//! 非 Windows 平台的文本后端占位。
//!
//! 只做"估算度量 + 不画字"：mac/Linux 接 CoreText/Pango 时不改变
//! `zouwu_ui::text` 与 `settings` 的任何调用点。

pub struct Metrics {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
}

pub fn measure(text: &str, px: f32, _bold: bool) -> f32 {
    text.chars().count() as f32 * px * 0.55
}

pub fn metrics(px: f32, _bold: bool) -> Metrics {
    Metrics {
        width: 0.0,
        ascent: px * 0.8,
        descent: px * 0.2,
        line_height: px * 1.2,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    _pixmap: &[u8],
    _pm_width: usize,
    _pm_height: usize,
    _text: &str,
    _x: f32,
    _baseline: f32,
    _px: f32,
    _color: [u8; 4],
    _bold: bool,
    _clip: Option<(f32, f32, f32, f32)>,
) -> Option<Vec<u8>> {
    None
}
