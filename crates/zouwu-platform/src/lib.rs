//! zouwu-platform —— 平台能力层。
//!
//! v1 只实现 Windows（见 `docs/decisions.md` ADR-001），但所有平台相关逻辑
//! 都必须待在本 crate 里，UI 与领域层通过这里的函数/ trait 使用。

pub mod assets;
#[cfg(target_os = "windows")]
pub mod autostart;
#[cfg(target_os = "windows")]
pub mod clipboard;
#[cfg(not(target_os = "windows"))]
#[path = "clipboard_stub.rs"]
pub mod clipboard;
/// 字体文件的 `name` 表解析（纯 Rust，可测）。
pub mod fontfile;
pub mod fontpack;
#[cfg(target_os = "windows")]
pub mod hotkey;
#[cfg(target_os = "windows")]
pub mod install;
#[cfg(target_os = "windows")]
pub mod layer;
pub mod logging;
#[cfg(target_os = "windows")]
pub(crate) mod reg;
#[cfg(target_os = "windows")]
pub mod single_instance;

/// 文本栅格化：Windows 走 GDI（系统共享字体，常驻内存≈0）。
#[cfg(target_os = "windows")]
pub mod text_gdi;
#[cfg(not(target_os = "windows"))]
#[path = "text_gdi_stub.rs"]
pub mod text_gdi;
#[cfg(target_os = "windows")]
pub mod tray;

/// Alt 拖动窗口（AltSnap 的最小集：拖动 / 双击最大化 / 滚轮最大最小化）。
#[cfg(target_os = "windows")]
pub mod altdrag;
#[cfg(not(target_os = "windows"))]
#[path = "altdrag_stub.rs"]
pub mod altdrag;
pub mod dialogs;
/// 护眼遮罩（蓝光过滤）：一层点击穿透的暖色窗口。
#[cfg(target_os = "windows")]
pub mod eyecare;
#[cfg(not(target_os = "windows"))]
#[path = "eyecare_stub.rs"]
pub mod eyecare;
/// 跑一条用户写的命令 / 打开一个程序（自定义气泡球的执行端）。
pub mod launch;
pub mod reload;
#[cfg(target_os = "windows")]
pub mod screen;
#[cfg(not(target_os = "windows"))]
#[path = "screen_stub.rs"]
pub mod screen;
pub mod skinpack;

use std::fs;
use std::path::{Path, PathBuf};

use zouwu_core::config::{Config, WindowState};

/// 打开配置目录（"配置文件…"入口）。
pub fn open_config_dir() -> bool {
    open_dir(&config_dir())
}

/// 用系统文件管理器打开一个目录。
///
/// 故意用 `explorer` 而不是 `ShellExecuteW`：前者对"路径里有空格/中文"的
/// 处理是系统自己做的，而且不需要我们再声明一堆 COM 相关的东西。
pub fn open_dir(dir: &Path) -> bool {
    let _ = fs::create_dir_all(dir);
    #[cfg(target_os = "windows")]
    {
        match std::process::Command::new("explorer").arg(dir).spawn() {
            Ok(_) => true,
            Err(err) => {
                log::warn!("explorer failed for {}: {err}", dir.display());
                false
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        matches!(std::process::Command::new("open").arg(dir).spawn(), Ok(_))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        matches!(
            std::process::Command::new("xdg-open").arg(dir).spawn(),
            Ok(_)
        )
    }
}

/// 配置根目录：Windows `%APPDATA%\zouwu`，其它平台 `$XDG_CONFIG_HOME/zouwu`。
pub fn config_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("zouwu");
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("zouwu");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join("zouwu");
    }
    PathBuf::from(".zouwu")
}

fn read_to_string(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// 原子写：先写临时文件再 rename，避免半个文件。
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn state_path() -> PathBuf {
    config_dir().join("state.toml")
}

pub fn history_path() -> PathBuf {
    config_dir().join("history.toml")
}

/// 读剪贴板历史。文件不存在就是空历史（第一次启动的常态）。
pub fn load_history() -> zouwu_core::history::History {
    match read_to_string(&history_path()) {
        Some(text) => zouwu_core::history::History::from_toml(&text),
        None => zouwu_core::history::History::new(zouwu_core::history::DEFAULT_LIMIT),
    }
}

pub fn save_history(history: &zouwu_core::history::History) {
    // 写失败只记日志：历史丢了是小事，让复制这个动作本身失败才是大事。
    if let Err(err) = write_atomic(&history_path(), &history.to_toml()) {
        log::warn!("failed to save clipboard history: {err}");
    }
}

/// 读取配置；文件不存在或解析失败时写回默认配置并返回默认值。
pub fn load_config() -> Config {
    let path = config_path();
    match read_to_string(&path) {
        Some(text) => match Config::from_toml(&text) {
            Ok(cfg) => cfg.migrated(),
            Err(err) => {
                log::warn!("config parse failed ({err}); backing up and using defaults");
                let _ = fs::rename(&path, path.with_extension("bad.toml"));
                let cfg = Config::default();
                let _ = write_atomic(&path, &cfg.to_toml());
                cfg
            }
        },
        None => {
            let cfg = Config::default();
            if let Err(err) = write_atomic(&path, &cfg.to_toml()) {
                log::warn!("failed to write default config: {err}");
            }
            cfg
        }
    }
}

pub fn save_config(cfg: &Config) {
    if let Err(err) = write_atomic(&config_path(), &cfg.to_toml()) {
        log::warn!("failed to save config: {err}");
    }
}

pub fn load_window_state() -> Option<WindowState> {
    let text = read_to_string(&state_path())?;
    toml::from_str::<WindowState>(&text).ok()
}

pub fn save_window_state(state: WindowState) {
    let text = toml::to_string(&state).unwrap_or_default();
    if let Err(err) = write_atomic(&state_path(), &text) {
        log::warn!("failed to save window state: {err}");
    }
}

/// 当前平台名，写进日志便于排查。
pub fn platform_name() -> &'static str {
    #[cfg(windows)]
    {
        "windows"
    }
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "linux"
    }
    #[cfg(not(any(windows, unix)))]
    {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_is_not_empty() {
        assert!(!config_dir().as_os_str().is_empty());
    }

    #[test]
    fn atomic_roundtrip() {
        let dir = std::env::temp_dir().join("zouwu-test-atomic");
        let path = dir.join("x.toml");
        write_atomic(&path, "a = 1\n").unwrap();
        assert_eq!(read_to_string(&path).as_deref(), Some("a = 1\n"));
        let _ = fs::remove_dir_all(&dir);
    }
}
