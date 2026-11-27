//! 开机自启（Linux：XDG autostart，即 `~/.config/autostart/zouwu.desktop`）。
//!
//! 和 Windows 的 `HKCU\...\Run` 是同一个语义层的两套实现：都是"用户登录后
//! 由桌面环境替我们起一个进程"，都不需要 root，都能被用户在系统设置里看见并否掉。
//!
//! 为什么不用 systemd user unit：autostart 目录是桌面环境的既有约定
//! （KDE/GNOME/XFCE 都在「自启动」面板里读它），用户能看见、能取消，
//! 而且我们能直接读回状态来给菜单打勾。systemd 那条路要额外判断"这个会话
//! 到底是不是有图形"，反而更绕。
//!
//! 坑：`.desktop` 里的 `Exec=` 不是 shell，路径要按 Desktop Entry Spec 转义
//! （带空格/引号的路径必须加双引号并转义 `"` `` ` `` `$` `\`），否则改天程序
//! 装到带空格的目录下，自启就静默失效了。

use std::fs;
use std::path::{Path, PathBuf};

/// 桌面项文件名（固定，方便判断"是不是我们写的"）。
const FILE_NAME: &str = "zouwu.desktop";

fn autostart_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("autostart"));
        }
    }
    let home = std::env::var_os("HOME")?;
    (!home.is_empty()).then(|| PathBuf::from(home).join(".config").join("autostart"))
}

fn desktop_path() -> Option<PathBuf> {
    Some(autostart_dir()?.join(FILE_NAME))
}

/// 按 Desktop Entry Spec 的 `Exec=` 规则转义一个参数。
fn quote(arg: &str) -> String {
    let needs_quotes = arg.is_empty()
        || arg
            .chars()
            .any(|c| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c));
    if !needs_quotes {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        if c == '"' || c == '`' || c == '$' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    // Desktop Entry Spec 还要求整体再转义一层 `\`：字符串值里的反斜杠要写成 `\\`。
    out.replace('\\', "\\\\").replace("\"\"", "\"")
}

/// 当前自启项指向的可执行文件（没有自启项时是 None）。
pub fn command_path() -> Option<String> {
    let text = fs::read_to_string(desktop_path()?).ok()?;
    for line in text.lines() {
        let Some(exec) = line.strip_prefix("Exec=") else {
            continue;
        };
        let exec = exec.trim();
        // 我们只写一个裸路径（必要时带引号），所以逆操作很简单。
        let unquoted = exec.trim_matches('"').replace("\\\\", "\\").replace("\\\"", "\"");
        return Some(unquoted);
    }
    None
}

pub fn is_enabled() -> bool {
    let Some(path) = desktop_path() else {
        return false;
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return false;
    };
    // 只看 Exec 指向的那个文件还在不在：用户把程序挪走之后，
    // 留着一个指向空处的 .desktop 只会让登录时静默失败。
    match command_path() {
        Some(cmd) => Path::new(&cmd).exists() && !text.contains("Hidden=true"),
        None => false,
    }
}

/// 把自启项指到 `exe`。返回值 = 文件写成功。
pub fn set_command(exe: &Path) -> bool {
    let Some(dir) = autostart_dir() else {
        log::warn!("autostart: no XDG_CONFIG_HOME/HOME");
        return false;
    };
    if let Err(err) = fs::create_dir_all(&dir) {
        log::warn!("autostart: create {} failed: {err}", dir.display());
        return false;
    }
    let exe = match fs::canonicalize(exe) {
        Ok(path) => path,
        Err(_) => exe.to_path_buf(),
    };
    // 已经是这条命令就什么都不做（避免每次启动都重写文件、刷新 mtime）。
    if command_path().as_deref() == Some(exe.to_string_lossy().as_ref()) {
        return true;
    }
    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=驺吾\n\
         Name[en]=zouwu\n\
         Comment=桌面上溜达的小兽\n\
         Comment[en]=A little beast that wanders your desktop\n\
         Exec={}\n\
         Icon={}\n\
         Terminal=false\n\
         StartupNotify=false\n\
         X-GNOME-Autostart-enabled=true\n\
         X-KDE-autostart-after=panel\n",
        quote(&exe.to_string_lossy()),
        // 图标写**主题名**（`--install` 已把 PNG 落到 hicolor 里），桌面环境自己找得到；
        // 没装过（直接跑 target/debug 那种）就留空，环境会退回默认图标。
        // 这里曾经去 `assets::resolve_icon("zouwu")` 找，那找的是**球的自定义图标**目录
        // （`~/.config/zouwu/icons/`），和程序自己的图标不是一回事，所以一直找不到。
        if crate::install::icon_path().is_some_and(|path| path.is_file()) {
            "zouwu"
        } else {
            ""
        },
    );
    let path = dir.join(FILE_NAME);
    match crate::write_atomic(&path, &body) {
        Ok(()) => {
            log::info!("autostart: wrote {}", path.display());
            true
        }
        Err(err) => {
            log::warn!("autostart: write {} failed: {err}", path.display());
            false
        }
    }
}

/// 开/关自启。`true` 时把 Exec 指向当前可执行文件。
pub fn set_enabled(enabled: bool) -> bool {
    if !enabled {
        let Some(path) = desktop_path() else {
            return false;
        };
        if !path.exists() {
            return true;
        }
        return match fs::remove_file(&path) {
            Ok(()) => {
                log::info!("autostart: removed {}", path.display());
                true
            }
            Err(err) => {
                log::warn!("autostart: remove {} failed: {err}", path.display());
                false
            }
        };
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    set_command(&exe)
}

pub fn open_config_dir() -> bool {
    crate::open_dir(&crate::config_dir())
}

pub fn open_path(path: &Path) -> bool {
    crate::open_dir(path)
}