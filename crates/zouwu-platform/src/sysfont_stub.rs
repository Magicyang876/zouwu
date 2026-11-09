//! `sysfont` 的非 Windows 占位：v1 只发 Windows（ADR-001），但 `zouwu-ui` 的测试
//! 要在别的平台上编得过，所以这里给同名同签名的空实现。

use super::{Chrome, Entry, Snapshot, Target};

pub fn current() -> Option<Snapshot> {
    None
}

pub fn apply(_entries: &[(Target, Entry)]) -> Result<(), String> {
    Err("sysfont: only supported on Windows".to_string())
}

pub fn restore(_snapshot: &Snapshot) -> Result<(), String> {
    Err("sysfont: only supported on Windows".to_string())
}

pub fn apply_with(_entries: &[(Target, Entry)], _chrome: Option<Chrome>) -> Result<(), String> {
    Err("sysfont: only supported on Windows".to_string())
}

pub fn chrome_needs_repair() -> bool {
    false
}

pub fn restore_chrome() -> Result<Chrome, String> {
    Err("sysfont: only supported on Windows".to_string())
}

pub fn os_build() -> u32 {
    0
}

pub fn caption_supported() -> bool {
    false
}

pub fn installed_faces() -> Vec<String> {
    Vec::new()
}

pub fn substitute_face() -> Option<String> {
    None
}

pub fn set_substitute(_face: Option<&str>) -> Result<(), String> {
    Err("sysfont: only supported on Windows".to_string())
}

pub fn is_admin() -> bool {
    false
}

pub fn caption_metrics() -> Option<(i32, i32)> {
    None
}

/// `sysfont::metrics_dump` 的替身：非 Windows 没有这套字面量。
pub(super) fn dump() -> Option<String> {
    None
}
