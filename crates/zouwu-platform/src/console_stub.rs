//! 非 Windows 占位：没有"控制台子系统"这回事（ADR-001 只做 Windows）。

/// 恒不接管：Linux/macOS 的可执行文件本来就从终端继承 stdout。
pub fn attach_parent() -> bool {
    false
}
