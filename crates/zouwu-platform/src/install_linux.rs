//! 安装 / 卸载（Linux：XDG 目录，无系统包管理器介入）。
//!
//! Windows 版的做法（复制 exe + 写注册表 + 生成 `.lnk`）在这里换成对应的
//! 三件事，语义一一对齐：
//!
//! | Windows | Linux |
//! |---|---|
//! | `%LOCALAPPDATA%\Programs\zouwu\zouwu.exe` | `~/.local/bin/zouwu`（`$XDG_BIN_HOME` 优先） |
//! | 「应用和功能」注册表键 | `~/.local/share/applications/zouwu.desktop` |
//! | 开始菜单 `.lnk` | 同上那个 `.desktop`（应用菜单就是读它） |
//! | `HKCU\...\Run` 自启 | `~/.config/autostart/zouwu.desktop`（见 `autostart_linux.rs`） |
//!
//! **不碰 `/usr`、不要 sudo**：装到用户目录才能"解压即用 / 一键卸干净"。
//! 系统级安装交给发行版打包，那件事有它自己的规矩（FHS、`/usr/bin` 所有权）。
//!
//! 卸载时**可以立刻删掉正在运行的二进制**（Linux 的 inode 语义），
//! 所以这里没有 Windows 那套「延迟 2 秒删自己」的花活。

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "zouwu";

/// 桌面入口文件名
const DESKTOP_NAME: &str = "zouwu.desktop";

/// 应用菜单图标，内嵌在二进制里（和窗口图标是同一张 `assets/zouwu-icon.png`）。
///
/// 和 Windows 那个 `ICON_BYTES` 一个道理：发布版只有一个可执行文件，没有
/// `assets/` 目录可以读，所以图标必须跟着二进制走，安装时再落到图标主题里。
const ICON_PNG: &[u8] = include_bytes!("../../../assets/zouwu-icon.png");

/// 这次安装实际做成了什么（字段与 Windows 版同名，命令行打印因此共用一套）。
#[derive(Debug, Default)]
pub struct InstallReport {
    pub exe: PathBuf,
    pub dir: PathBuf,
    pub replaced: bool,
    pub autostart: bool,
    /// Linux 下指 `.desktop` 文件（对应 Windows 的 `.lnk`）。
    pub shortcut: Option<PathBuf>,
    /// Linux 没有注册表；留着这个字段让调用方不用按平台分支。
    pub registry: bool,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => home().map(|home| home.join(fallback)),
    }
}

/// `$XDG_BIN_HOME` → `~/.local/bin`
pub fn bin_dir() -> PathBuf {
    match std::env::var_os("XDG_BIN_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home()
            .map(|home| home.join(".local").join("bin"))
            .unwrap_or_else(std::env::temp_dir),
    }
}

/// 安装目录 = 可执行文件所在目录。
pub fn install_dir() -> PathBuf {
    bin_dir()
}

pub fn installed_exe() -> PathBuf {
    bin_dir().join(APP_NAME)
}

/// 桌面入口（应用菜单里那个图标）。
pub fn desktop_entry() -> Option<PathBuf> {
    Some(
        xdg_dir("XDG_DATA_HOME", ".local/share")?
            .join("applications")
            .join(DESKTOP_NAME),
    )
}

/// 安装后的图标文件：XDG 图标主题里的 `hicolor/128x128/apps/zouwu.png`。
///
/// 走**图标主题**而不是在 `.desktop` 里写绝对路径：这样 KDE/GNOME 按自己的
/// 尺寸需求缩放（菜单 32px、任务栏 24px），高 DPI 下也不会糊成一团。
/// 128 是仓库里那张 PNG 的原始尺寸。
pub fn icon_path() -> Option<PathBuf> {
    Some(
        xdg_dir("XDG_DATA_HOME", ".local/share")?
            .join("icons")
            .join("hicolor")
            .join("128x128")
            .join("apps")
            .join(format!("{APP_NAME}.png")),
    )
}

/// 有没有装过。**只看文件在不在**，和 Windows 版同一个判断口径。
pub fn is_installed() -> bool {
    installed_exe().is_file()
}

/// 当前跑的是不是装好的那份。
pub fn running_installed_copy() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let installed = installed_exe();
    match (exe.canonicalize(), installed.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => exe == installed,
    }
}

/// Desktop Entry 的字符串值：空格/引号这类字符按规范转义。
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ' ' => out.push_str("\\s"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

/// 生成 `.desktop` 内容。`Exec` 用绝对路径：应用菜单不会帮我们找 `PATH`。
fn desktop_contents(exe: &Path, icon: Option<&str>) -> String {
    let mut text = String::from(
        "[Desktop Entry]\n\
         Type=Application\n\
         Categories=Utility;\n\
         StartupNotify=false\n\
         Terminal=false\n",
    );
    text.push_str(&format!("Name={APP_NAME}\n"));
    text.push_str(&format!(
        "Comment={}\n",
        quote("A desktop companion that expands into a ring of shortcuts")
    ));
    text.push_str(&format!("Exec={}\n", quote(&exe.display().to_string())));
    if let Some(icon) = icon {
        text.push_str(&format!("Icon={}\n", quote(icon)));
    }
    text
}

/// 安装（幂等）：复制可执行文件、写自启、建桌面入口。
pub fn install(version: &str) -> Result<InstallReport, String> {
    let dir = install_dir();
    let target = installed_exe();
    std::fs::create_dir_all(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;

    let mut report = InstallReport {
        exe: target.clone(),
        dir,
        ..InstallReport::default()
    };

    // 1) 复制自己。先写临时名再 rename：中途断电不会留下半个可执行文件。
    let source = std::env::current_exe().map_err(|err| format!("locate self: {err}"))?;
    let same = source
        .canonicalize()
        .ok()
        .zip(target.canonicalize().ok())
        .is_some_and(|(a, b)| a == b);
    if !same {
        report.replaced = target.exists();
        let staged = target.with_extension("new");
        std::fs::copy(&source, &staged)
            .map_err(|err| format!("copy to {}: {err}", staged.display()))?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|err| format!("chmod {}: {err}", staged.display()))?;
        std::fs::rename(&staged, &target)
            .map_err(|err| format!("install to {}: {err}", target.display()))?;
    }

    // 2) 自启指向**安装后的**位置（不是构建目录里的那个 target/debug/zouwu）。
    report.autostart = crate::autostart::set_command(&target);

    // 3) 图标进主题 + 应用菜单入口。没图标的话菜单里就是个灰方块，
    //    而 `.desktop` 的 `Icon=` 只有在图标真的落了盘时写才对。
    let icon = install_icon().inspect_err(|err| log::warn!("cannot install icon: {err}"));
    if let Some(entry) = desktop_entry() {
        if let Some(parent) = entry.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match crate::write_atomic(
            &entry,
            &desktop_contents(&target, icon.is_ok().then_some(APP_NAME)),
        ) {
            Ok(()) => report.shortcut = Some(entry),
            Err(err) => log::warn!("cannot write desktop entry: {err}"),
        }
    }

    log::info!(
        "installed {APP_NAME} {version} -> {} (replaced={}, autostart={}, entry={:?})",
        target.display(),
        report.replaced,
        report.autostart,
        report.shortcut
    );
    Ok(report)
}

/// 把内嵌的 PNG 落到图标主题里。
///
/// 先写 `.tmp` 再 rename：图标目录一动，面板和文件管理器就会去重建缩略图缓存，
/// 半张 PNG 会被它记成一张坏图。
fn install_icon() -> Result<PathBuf, String> {
    let path = icon_path().ok_or("no HOME for the icon dir")?;
    let parent = path.parent().ok_or("icon path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
    let staged = path.with_extension("tmp");
    std::fs::write(&staged, ICON_PNG)
        .map_err(|err| format!("write {}: {err}", staged.display()))?;
    std::fs::rename(&staged, &path).map_err(|err| format!("install {}: {err}", path.display()))?;
    log::info!("icon installed: {}", path.display());
    Ok(path)
}

/// 卸载：关自启、删桌面入口、删图标、删可执行文件。
pub fn uninstall() -> Result<(), String> {
    crate::autostart::set_enabled(false);

    if let Some(entry) = desktop_entry() {
        if entry.is_file() {
            match std::fs::remove_file(&entry) {
                Ok(()) => log::info!("removed {}", entry.display()),
                Err(err) => log::warn!("cannot remove {}: {err}", entry.display()),
            }
        }
    }

    if let Some(icon) = icon_path() {
        if icon.is_file() {
            match std::fs::remove_file(&icon) {
                Ok(()) => log::info!("removed {}", icon.display()),
                Err(err) => log::warn!("cannot remove {}: {err}", icon.display()),
            }
        }
    }

    let target = installed_exe();
    if target.is_file() {
        // 正在跑的这份也能删（inode 语义），进程照常活到退出。
        std::fs::remove_file(&target)
            .map_err(|err| format!("cannot remove {}: {err}", target.display()))?;
    }
    log::info!("uninstalled (removed {})", target.display());
    Ok(())
}

/// 删掉用户数据（配置/皮肤/日志）。`--uninstall --purge` 用。
pub fn purge_data() -> Result<(), String> {
    // 先把改过的东西还回去：配置目录里有 `altdrag.state`，删了就再也不知道
    // 原来的 `CommandAllKey` 是什么，用户的 Meta 拖动会永远变成 Alt。
    crate::altdrag::stop();
    crate::eyecare::stop();
    crate::activate::shutdown();
    let dir = crate::config_dir();
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    log::info!("purged {}", dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_quotes_spaces() {
        let text = desktop_contents(Path::new("/home/a b/zouwu"), Some(APP_NAME));
        assert!(text.contains("Exec=/home/a\\sb/zouwu"), "{text}");
        // 图标走主题名，不是绝对路径：写错一位菜单里就是个灰方块。
        assert!(text.contains("Icon=zouwu\n"), "{text}");
    }

    /// 图标落在 hicolor 主题的 128x128 槽里——`Icon=zouwu` 能不能解析全看这条路径。
    #[test]
    fn icon_lands_in_the_hicolor_theme() {
        let path = icon_path().expect("HOME set in tests");
        let text = path.display().to_string();
        assert!(
            text.contains("icons/hicolor/128x128/apps/zouwu.png"),
            "{text}"
        );
    }

    #[test]
    fn install_paths_live_under_home() {
        // `HOME` 一定存在（否则 bin_dir 退到 temp），这里只断言路径形状。
        let bin = bin_dir();
        assert!(bin.ends_with("bin") || bin == std::env::temp_dir());
    }
}
