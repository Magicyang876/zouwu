//! 非 Windows 占位：护眼遮罩未实现（ADR-001 只做 Windows，接口先占位）。

/// 恒失败：调用方按"功能不可用"处理，而不是以为屏幕已经变暖。
pub fn apply(_enabled: bool, _alpha: u8) -> Result<(), String> {
    Err("eye care overlay is only implemented on Windows".to_string())
}

pub fn shutdown() {}

pub fn is_active() -> bool {
    false
}

pub fn alpha() -> u8 {
    0
}
