//! `sysfont` 的非 Windows 占位：v1 只发 Windows（ADR-001），但 `zouwu-ui` 的测试
//! 要在别的平台上编得过，所以这里给同名同签名的空实现。

use super::{Entry, Snapshot, Target};

pub fn current() -> Option<Snapshot> {
    None
}

pub fn apply(_entries: &[(Target, Entry)]) -> Result<(), String> {
    Err("sysfont: only supported on Windows".to_string())
}

pub fn restore(_snapshot: &Snapshot) -> Result<(), String> {
    Err("sysfont: only supported on Windows".to_string())
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
