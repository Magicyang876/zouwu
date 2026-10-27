//! 安装 / 卸载：把「一个 exe」变成「一个装好的程序」。
//!
//! ## 为什么不做 MSI / Inno Setup
//!
//! 三种人用得到"安装"这件事：想要开始菜单里的人、想要控制面板里能卸载的人、
//! 想要脚本里能静默部署的人。这三件事**都不需要**一个安装器框架：
//!
//! | 需要的东西 | 不装框架的做法 |
//! |---|---|
//! | 开始菜单入口 | 一个 `.lnk`（用系统的 `WScript.Shell` 生成） |
//! | 程序与功能里能卸载 | `HKCU\...\Uninstall\zouwu` 键（`DisplayName`/`UninstallString`） |
//! | 静默部署 | `zouwu.exe --install --silent` |
//!
//! 代价是要自己写几十行注册表和一个快捷方式生成，收益是**没有安装器、没有签名证书
//! 依赖、没有"下一步下一步"的六屏向导**，而且 zip 解压即用的便携形态依然成立。
//!
//! ## 装到哪
//!
//! `%LOCALAPPDATA%\Programs\zouwu`：不需要管理员权限（`Program Files` 要），
//! 又不像 temp 那样会被清理。这是 VS Code / GitHub Desktop 们的共同选择。
//!
//! ## 卸载时删自己
//!
//! 正在运行的 exe 删不掉。这里**不申请管理员权限、不写 reboot 标记**，而是
//! 让 `cmd` 在 2 秒后替我们删目录（那会儿进程已经退了）。用户看到的是
//! "卸载完成，文件夹随即消失"，而不是"已标记为下次启动时删除"。

use std::path::{Path, PathBuf};

use crate::reg;

pub const APP_NAME: &str = "zouwu";

/// 随二进制内嵌的图标字节（`assets/zouwu.ico`，由 `examples/make_icon.rs` 生成）。
///
/// 为什么安装时要落一份到安装目录：gnu 工具链下 `.res` 没法链进 exe
/// （见 `build.rs` 的说明），exe 自己就没有图标资源。于是**快捷方式的图标
/// 指向这个 ico 文件**——用户看到的图标是对的，代价只是多一个 110 KB 的文件。
pub const ICON_BYTES: &[u8] = include_bytes!("../../../assets/zouwu.ico");

/// 安装目录里的图标文件路径。
pub fn icon_path() -> PathBuf {
    install_dir().join(format!("{APP_NAME}.ico"))
}
const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\zouwu";
const START_MENU: &str = "Microsoft\\Windows\\Start Menu\\Programs";

/// 这次安装实际做成了什么，供命令行打印与日志核对。
#[derive(Debug, Default)]
pub struct InstallReport {
    pub exe: PathBuf,
    pub dir: PathBuf,
    /// 是否覆盖了已有的一份（升级路径）。
    pub replaced: bool,
    pub autostart: bool,
    pub shortcut: Option<PathBuf>,
    pub registry: bool,
}

/// `%LOCALAPPDATA%\Programs\zouwu`
///
/// 读环境变量而不是调 `SHGetKnownFolderPath`：少一次 COM 初始化，
/// 而且 `LOCALAPPDATA` 在进程里本来就有（万一没有，退回 `%APPDATA%`）。
pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
        });
    base.join("Programs").join(APP_NAME)
}

pub fn installed_exe() -> PathBuf {
    install_dir().join(format!("{APP_NAME}.exe"))
}

/// 安装目录里是否有一份 exe。**不看注册表**：文件在就算装了，
/// 这样"手工删了文件夹但注册表还在"的脏状态不会被误判成已安装。
pub fn is_installed() -> bool {
    installed_exe().is_file()
}

/// 当前正在跑的是不是"装好的那份"。
///
/// 便携运行 / 从 zip 里直接跑 / 安装后从开始菜单跑，三种情况都会走到这里，
/// 用它决定"要不要提示重新安装"或者"卸载时能不能立刻删目录"。
pub fn running_installed_copy() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    // 规范化后再比：同一个文件可能经过 8.3 短名、符号链接等不同写法。
    match (exe.canonicalize(), installed_exe().canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => exe == installed_exe(),
    }
}

fn start_menu_shortcut() -> Option<PathBuf> {
    let programs = std::env::var_os("APPDATA").map(PathBuf::from)?;
    Some(programs.join(START_MENU).join(format!("{APP_NAME}.lnk")))
}

/// 安装（幂等）：复制 exe、写卸载信息、设自启、建开始菜单快捷方式。
pub fn install(version: &str) -> Result<InstallReport, String> {
    let dir = install_dir();
    let target = installed_exe();
    std::fs::create_dir_all(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;

    let mut report = InstallReport {
        exe: target.clone(),
        dir: dir.clone(),
        ..InstallReport::default()
    };

    // 1) 复制自己。先写临时名再 rename：中途断电不会留下半个 exe。
    let source = std::env::current_exe().map_err(|err| format!("locate self: {err}"))?;
    let same = source
        .canonicalize()
        .ok()
        .zip(target.canonicalize().ok())
        .is_some_and(|(a, b)| a == b);
    if !same {
        report.replaced = target.exists();
        let staged = dir.join(format!("{APP_NAME}.new"));
        std::fs::copy(&source, &staged)
            .map_err(|err| format!("copy to {}: {err}", staged.display()))?;
        // Windows 上 `rename` 会覆盖目标（内部是 MOVEFILE_REPLACE_EXISTING）。
        std::fs::rename(&staged, &target)
            .map_err(|err| format!("install to {}: {err}", target.display()))?;
    }

    // 2) 图标落地（快捷方式和「应用和功能」的图标都靠它）。
    let icon = icon_path();
    if let Err(err) = std::fs::write(&icon, ICON_BYTES) {
        log::warn!("cannot write {}: {err}", icon.display());
    }

    // 3) 「应用和功能」里的卸载入口。
    let exe_arg = format!("\"{}\"", target.display());
    let mut ok = true;
    for (name, value) in [
        ("DisplayName", APP_NAME.to_string()),
        ("DisplayVersion", version.to_string()),
        ("Publisher", APP_NAME.to_string()),
        ("DisplayIcon", icon.display().to_string()),
        ("InstallLocation", dir.display().to_string()),
        ("UninstallString", format!("{exe_arg} --uninstall")),
        (
            "QuietUninstallString",
            format!("{exe_arg} --uninstall --silent"),
        ),
    ] {
        ok &= reg::set_string(UNINSTALL_KEY, name, &value);
    }
    report.registry = ok;

    // 4) 自启指向**安装后的**位置（不是 zip 里那个临时路径）。
    report.autostart = crate::autostart::set_command(&target);

    // 5) 开始菜单快捷方式。
    report.shortcut = create_shortcut(&target, &icon);
    log::info!(
        "installed {APP_NAME} {version} -> {} (replaced={}, autostart={}, shortcut={:?})",
        target.display(),
        report.replaced,
        report.autostart,
        report.shortcut
    );
    Ok(report)
}

/// 卸载：清注册表/自启/快捷方式，然后把自己这份删掉。
///
/// 返回后进程**应该立刻退出**——删目录的计划任务还在等我们放手。
pub fn uninstall() -> Result<(), String> {
    crate::autostart::set_enabled(false);
    reg::delete_tree(UNINSTALL_KEY);

    if let Some(lnk) = start_menu_shortcut() {
        if lnk.is_file() {
            match std::fs::remove_file(&lnk) {
                Ok(()) => log::info!("removed shortcut {}", lnk.display()),
                Err(err) => log::warn!("cannot remove {}: {err}", lnk.display()),
            }
        }
    }

    let dir = install_dir();
    if !running_installed_copy() {
        // 从别处跑卸载（便携副本 / 已损坏的安装）：可以直接删。
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        }
        log::info!("uninstalled (removed {})", dir.display());
        return Ok(());
    }

    // 删自己：交给 cmd 延时执行。
    schedule_self_delete(&dir)?;
    log::info!("uninstalled; {} will disappear in ~2s", dir.display());
    Ok(())
}

/// 让一个外部进程**等我们退出后**删掉安装目录。
///
/// 这行代码前后踩了两个坑，都值得记下来：
///
/// 1. 最初写 `cmd /c "timeout /t 2 & rmdir ..."`：从 Git Bash 启动时子进程继承
///    MSYS 的 `PATH`，`timeout` 命中的是 **coreutils 的 timeout**（`invalid time interval`）。
///    → 改成 `%SystemRoot%\System32\timeout.exe` 绝对路径，且命令行用 `raw_arg`
///    原样传递（`Command::arg` 会给参数加引号并转义内层 `"`，cmd.exe 不认）。
/// 2. 即使这样，`timeout` 在没有控制台/输入被重定向时会直接报错，而
///    `&` 后面的 `rmdir` 立刻执行——那时 exe 还锁着，等于什么都没做。
///    → 干脆不用"睡一会儿"，改用 `Wait-Process -Id <我们的 pid>`：**等的是事件
///    而不是时间**，慢机器上也不会失效。
///
/// 都没有时退回 `ping` 计时（不依赖控制台），再不行就让用户自己删目录。
fn schedule_self_delete(dir: &Path) -> Result<(), String> {
    let literal = dir.display().to_string().replace('\'', "''");
    let script = format!(
        "Wait-Process -Id {} -ErrorAction SilentlyContinue;          Remove-Item -LiteralPath '{}' -Recurse -Force -ErrorAction SilentlyContinue",
        std::process::id(),
        literal
    );
    if spawn_hidden(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    ) {
        return Ok(());
    }

    let system32 = std::env::var("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32"))
        .unwrap_or_else(|_| PathBuf::from(r"C:\Windows\System32"));
    let ping = system32.join("ping.exe");
    if ping.is_file() {
        let cmd = format!(
            "\"{}\" -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"",
            ping.display(),
            dir.display().to_string().replace('"', "")
        );
        let shell = std::env::var("ComSpec").unwrap_or_else(|_| String::from("cmd.exe"));
        let mut child = std::process::Command::new(shell);
        child.arg("/c");
        // 必须 `raw_arg`：`Command::arg` 会按 MSVCRT 规则给参数加引号并转义内层 `"`，
        // 而 cmd.exe 不认这种转义。这条命令行只含我们自己拼的路径，没有用户输入。
        use std::os::windows::process::CommandExt;
        child.raw_arg(cmd);
        if child.spawn().is_ok() {
            return Ok(());
        }
    }
    Err(format!(
        "cannot schedule removal of {}; please delete it manually",
        dir.display()
    ))
}

/// 起一个不带窗口的子进程（powershell 默认会闪一下黑框）。
fn spawn_hidden(program: &str, args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .is_ok()
}

/// 删掉用户数据（配置/皮肤/日志）。`--uninstall --purge` 用。
///
/// 单独成函数而不是塞进 [`uninstall`]：**装回去还想要我的配置**是常态。
pub fn purge_data() -> Result<(), String> {
    let dir = crate::config_dir();
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    log::info!("purged {}", dir.display());
    Ok(())
}

/// 用系统的 `WScript.Shell` 生成 `.lnk`。
///
/// 为什么不上 `IShellLinkW`：那要 COM 初始化 + 手写三张 vtable 的函数指针，
/// 为了一个快捷方式不值得。`WScript.Shell` 在所有受支持的 Windows 上都在，
/// 失败也只是少一个开始菜单入口（下面的返回值会如实告诉调用方）。
fn create_shortcut(target: &Path, icon: &Path) -> Option<PathBuf> {
    let lnk = start_menu_shortcut()?;
    if let Some(parent) = lnk.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let q = |p: &Path| p.display().to_string().replace('\'', "''");
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');\
         $s.TargetPath='{}';$s.WorkingDirectory='{}';$s.IconLocation='{}';$s.Description='{APP_NAME}';$s.Save()",
        q(&lnk),
        q(target),
        q(target.parent().unwrap_or(target)),
        // 图标单独指文件：exe 里可能没有图标资源（gnu 工具链）
        q(icon),
    );
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output();
    match output {
        Ok(out) if out.status.success() && lnk.is_file() => Some(lnk),
        Ok(out) => {
            log::warn!(
                "shortcut failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
            None
        }
        Err(err) => {
            log::warn!("cannot run powershell: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 安装目录必须在用户目录下，不能落到 `Program Files`（那需要管理员权限）。
    #[test]
    fn install_dir_is_per_user() {
        let dir = install_dir();
        let text = dir.to_string_lossy().to_ascii_lowercase();
        assert!(text.contains("programs"), "{text}");
        assert!(text.ends_with(APP_NAME), "{text}");
        assert!(!text.contains("program files"), "{text}");
        assert_eq!(installed_exe().file_name().unwrap(), "zouwu.exe");
    }

    #[test]
    fn uninstall_key_is_under_our_own_name() {
        assert!(UNINSTALL_KEY.ends_with(&format!("\\{}", APP_NAME)));
        assert!(
            UNINSTALL_KEY.starts_with("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall")
        );
    }
}
