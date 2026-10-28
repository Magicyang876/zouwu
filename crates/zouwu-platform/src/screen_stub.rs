//! 非 Windows 占位：截图未实现（ADR-001 只做 Windows，接口先占位）。

/// 一张整桌面截图（占位版恒不产生）。
pub struct Capture {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Capture {
    pub fn to_desktop(&self, px: u32, py: u32) -> (i32, i32) {
        (self.x + px as i32, self.y + py as i32)
    }
}

pub fn capture_virtual_screen() -> Result<Capture, String> {
    Err("screenshot capture is only implemented on Windows".to_string())
}
