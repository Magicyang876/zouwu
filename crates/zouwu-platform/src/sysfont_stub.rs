//! `sysfont` 的非 Windows 占位：v1 只发 Windows（ADR-001），但 `zouwu-ui` 的测试
//! 要在别的平台上编得过，所以这里给同名同签名的空实现。
//!
//! 签名必须和 `sysfont_sys.rs` 一一对应：`sysfont.rs` 用 `pub use sys::{...}`
//! 把两边的实现都当同一套 API 导出去，少一个就编不过。

use super::{Chrome, Entry, Snapshot, Target};

const UNSUPPORTED: &str = "sysfont: only supported on Windows";

pub fn current() -> Option<Snapshot> {
    None
}

pub fn apply(_entries: &[(Target, Entry)]) -> Result<(), String> {
    Err(UNSUPPORTED.to_string())
}

pub fn restore(_snapshot: &Snapshot) -> Result<(), String> {
    Err(UNSUPPORTED.to_string())
}

pub fn apply_with(_entries: &[(Target, Entry)], _chrome: Option<Chrome>) -> Result<(), String> {
    Err(UNSUPPORTED.to_string())
}

pub fn chrome_needs_repair() -> bool {
    false
}

pub fn restore_chrome() -> Result<Chrome, String> {
    Err(UNSUPPORTED.to_string())
}

pub fn os_build() -> u32 {
    0
}

pub fn caption_supported() -> bool {
    false
}

pub fn caption_metrics() -> Option<(i32, i32)> {
    None
}

pub fn installed_faces() -> Vec<String> {
    Vec::new()
}

pub fn substitute_face() -> Option<String> {
    None
}

pub fn set_substitute(_face: Option<&str>) -> Result<(), String> {
    Err(UNSUPPORTED.to_string())
}

pub fn is_admin() -> bool {
    false
}

pub fn system_dpi() -> u32 {
    96
}

/// 某个 DPI 下的文字度量（`(height, ascent, internal_leading)`）。
pub fn probe_for_dpi(_dpi: u32) -> Option<(i32, i32, i32)> {
    None
}

/// `sysfont::metrics_dump` 的替身：非 Windows 没有这套字面量。
pub(super) fn dump() -> Option<String> {
    None
}