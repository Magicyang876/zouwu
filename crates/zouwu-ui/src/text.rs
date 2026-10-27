//! 文本栅格化。
//!
//! 实现在平台层（Windows 走 GDI，见 `zouwu_platform::text_gdi`），这里只留
//! **与平台无关的排版逻辑**：折行、省略、度量封装。
//!
//! 为什么不用字体库自己解析：实测 `fontdue` 解析一个 CJK 字面常驻 175MB，
//! 六个候选字面一起加载把设置面板顶到 868MB。GDI 用系统共享字体，
//! 常驻增量≈0，中文/emoji 的 fallback 也是白送的。
//!
//! 非 Windows 平台暂时只有一个"估算度量 + 不画字"的占位实现，
//! 等 macOS/Linux 排期时再接 CoreText/Pango。

use tiny_skia::Pixmap;
use zouwu_core::theme::Rgba;

use zouwu_platform::text_gdi as backend;

/// 当前用的字体家族（`None` = 平台默认字体）。
pub fn family() -> Option<String> {
    backend::family()
}

/// 换字体家族。返回 `true` 表示真的换了 —— 调用方据此重排/重绘。
///
/// 只认**已经注册进本进程**的家族名（见 `zouwu_platform::fontpack::register_all`）。
/// 传一个系统里不存在的名字不会报错：GDI 会静默替换成别的字体，
/// 所以这里只能把"我们以为的设置"和"实际用的字体"对齐到家族名这一层。
pub fn set_family(family: Option<String>) -> bool {
    backend::set_family(family)
}

/// 文本引擎。字体资源在平台层是全局单例，这里只是一个零成本的门面。
pub struct TextEngine {
    /// 预留给将来的字形缓存/排版配置。
    _private: (),
}

impl TextEngine {
    fn new() -> Self {
        Self { _private: () }
    }

    /// 借用全局引擎执行一段排版操作。
    pub fn with<R>(f: impl FnOnce(&mut TextEngine) -> R) -> R {
        // 平台层自己带锁；这里只是为了保持"引擎实例"的调用形态。
        let mut engine = TextEngine::new();
        f(&mut engine)
    }

    pub fn has_fonts(&self) -> bool {
        cfg!(target_os = "windows")
    }

    /// 单行宽度（物理像素）。`bold` 会改变字形宽度，别忽略它。
    pub fn measure(&mut self, text: &str, px: f32, bold: bool) -> f32 {
        backend::measure(text, px, bold)
    }

    /// 单字符步进（折行/省略用）。
    fn advance(&mut self, ch: char, px: f32) -> f32 {
        let mut buf = [0u8; 4];
        self.measure(ch.encode_utf8(&mut buf), px, false)
    }

    /// 把文本混合进 pixmap。`clip` 是物理像素的裁剪框 `(x0, y0, x1, y1)`。
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        baseline: f32,
        px: f32,
        color: Rgba,
        bold: bool,
        clip: Option<(f32, f32, f32, f32)>,
    ) {
        if px < 1.0 || text.is_empty() {
            return;
        }
        let width = pixmap.width() as usize;
        let height = pixmap.height() as usize;
        let Some(out) = backend::draw(
            pixmap.data(),
            width,
            height,
            text,
            x,
            baseline,
            px,
            [color.r, color.g, color.b, color.a],
            bold,
            clip,
        ) else {
            return;
        };
        // 平台层返回的是"已混合"的整幅缓冲；拷回去即可。
        pixmap.data_mut().copy_from_slice(&out);
    }

    /// 单行最大可用宽度内折行。
    ///
    /// CJK 可以在任意字符间断开，拉丁文优先在空格断 —— 这正是两种语言混排时
    /// 唯一都说得过去的行为。
    pub fn wrap(&mut self, text: &str, px: f32, max_width: f32) -> Vec<String> {
        if max_width <= 0.0 {
            return vec![text.to_string()];
        }
        let mut lines = Vec::new();
        let mut line = String::new();
        let mut width = 0.0f32;
        let mut last_space: Option<(usize, f32)> = None;

        for ch in text.chars() {
            if ch == '\n' {
                lines.push(std::mem::take(&mut line));
                width = 0.0;
                last_space = None;
                continue;
            }
            let advance = self.advance(ch, px);
            if width + advance > max_width && !line.is_empty() {
                match last_space.take() {
                    Some((idx, _)) => {
                        let rest: String = line[idx..].trim_start().to_string();
                        line.truncate(idx);
                        lines.push(line.trim_end().to_string());
                        width = self.measure(&rest, px, false);
                        line = rest;
                    }
                    None => {
                        lines.push(std::mem::take(&mut line));
                        width = 0.0;
                    }
                }
            }
            if ch == ' ' {
                last_space = Some((line.len(), width));
            }
            line.push(ch);
            width += advance;
        }
        if !line.is_empty() {
            lines.push(line);
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        lines
    }

    /// 溢出的单行文本用省略号收尾。
    pub fn truncate(&mut self, text: &str, px: f32, max_width: f32) -> String {
        if self.measure(text, px, false) <= max_width {
            return text.to_string();
        }
        let ellipsis = "\u{2026}";
        let budget = (max_width - self.measure(ellipsis, px, false)).max(0.0);
        let mut out = String::new();
        let mut width = 0.0;
        for ch in text.chars() {
            let advance = self.advance(ch, px);
            if width + advance > budget {
                break;
            }
            out.push(ch);
            width += advance;
        }
        out.push_str(ellipsis);
        out
    }

    /// 行高（物理像素）。
    pub fn line_height(&self, px: f32) -> f32 {
        let m = backend::metrics(px, false);
        if m.line_height > 0.0 {
            m.line_height
        } else {
            px * 1.35
        }
    }

    /// 从基线到顶部的距离（物理像素），用于垂直居中对齐。
    pub fn ascent(&self, px: f32) -> f32 {
        let m = backend::metrics(px, false);
        if m.ascent > 0.0 {
            m.ascent
        } else {
            px * 0.8
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_cjk_anywhere() {
        let mut e = TextEngine::new();
        let lines = e.wrap("驺吾驺吾驺吾驺吾", 16.0, 40.0);
        assert!(lines.len() > 1, "long CJK run must wrap: {lines:?}");
    }

    #[test]
    fn truncate_returns_input_when_it_fits() {
        let mut e = TextEngine::new();
        assert_eq!(e.truncate("hi", 14.0, 1000.0), "hi");
    }
}
