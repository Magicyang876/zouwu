//! 非 Windows 占位：Alt 拖动窗口未实现（ADR-001 只做 Windows，接口先占位）。

/// 恒失败：调用方按"功能不可用"处理，而不是以为开着。
pub fn apply(_enabled: bool) -> Result<(), String> {
    Err("alt drag is only implemented on Windows".to_string())
}

pub fn stop() {}

pub fn shutdown() {}

pub fn is_active() -> bool {
    false
}
