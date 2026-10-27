//! 文本栅格化（fontdue）。
//!
//! 为什么是 fontdue 而不是 cosmic-text / skia：
//! - 我们要画的文本形态非常有限（标签、数值、按钮），不需要排版引擎的
//!   bidi / 复杂 shaping / 富文本；
//! - fontdue 是纯 Rust、零系统依赖，和 tiny-skia 的"CPU 光栅"路线一致；
//! - **字体不内嵌**：直接读系统里的 `msyh.ttc` / `segoeui.ttf`。内嵌一份 CJK
//!   字体要多背 10MB+ 的体积和授权问题，而这两个文件在 Windows 上必然存在
//!   （ADR-001：v1 只发 Windows）。
//!
//! 缺失字形不会 panic：`measure` 用近似宽度、`draw` 跳过并只 warn 一次。

use std::collections::HashMap;
use std::sync::OnceLock;

use fontdue::{Font, FontSettings};
use tiny_skia::Pixmap;
use zouwu_core::theme::Rgba;

/// 系统字体候选，按优先级排列。
///
/// 放在 `OnceLock` 里：字体是进程级的只读资源，重新加载它（20MB 解析）
/// 比画面本身贵几个数量级，而设置面板每次重绘都会用到文本。
const FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",    // 微软雅黑：中英俱佳，Win7+ 默认
    r"C:\Windows\Fonts\msyhl.ttc",   // 雅黑 Light（msyh 缺失时）
    r"C:\Windows\Fonts\Deng.ttf",    // 等线
    r"C:\Windows\Fonts\simhei.ttf",  // 黑体
    r"C:\Windows\Fonts\simsun.ttc",  // 宋体
    r"C:\Windows\Fonts\segoeui.ttf", // 英文兜底
];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct GlyphKey {
    font: u8,
    ch: u32,
    /// 物理像素大小（四舍五入到 1/4 px，避免缩放变化把缓存打爆）
    px: u16,
}

struct Cached {
    metrics: fontdue::Metrics,
    bitmap: Option<Vec<u8>>,
}

pub struct TextEngine {
    fonts: Vec<Font>,
    cache: HashMap<GlyphKey, Cached>,
    missing: std::collections::HashSet<char>,
}

impl TextEngine {
    fn global() -> &'static std::sync::Mutex<TextEngine> {
        static ENGINE: OnceLock<std::sync::Mutex<TextEngine>> = OnceLock::new();
        ENGINE.get_or_init(|| std::sync::Mutex::new(TextEngine::load()))
    }

    /// 借用全局文本引擎（字体只解析一次）。
    pub fn with<R>(f: impl FnOnce(&mut TextEngine) -> R) -> R {
        let mut guard = Self::global()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut guard)
    }

    fn load() -> Self {
        let mut fonts = Vec::new();
        for path in FONT_CANDIDATES {
            match std::fs::read(path) {
                Ok(bytes) => {
                    let settings = FontSettings {
                        // .ttc 是集合，0 号字面通常就是 Regular。
                        collection_index: 0,
                        scale: 32.0,
                        load_substitutions: false,
                    };
                    match Font::from_bytes(bytes, settings) {
                        Ok(font) => {
                            log::debug!("text: loaded font {}", path);
                            fonts.push(font);
                        }
                        Err(err) => log::warn!("text: font {path} rejected: {err}"),
                    }
                }
                Err(err) => log::debug!("text: no font at {path} ({err})"),
            }
        }
        if fonts.is_empty() {
            log::error!("text: no usable system font; settings panel labels will be blank");
        }
        Self {
            fonts,
            cache: HashMap::new(),
            missing: Default::default(),
        }
    }

    /// 某个字符用第几号字体；都没有则 `None`。
    fn pick(&self, ch: char) -> Option<usize> {
        self.fonts
            .iter()
            .position(|f| f.lookup_glyph_index(ch) != 0)
    }

    pub fn has_fonts(&self) -> bool {
        !self.fonts.is_empty()
    }

    /// 文本宽度（物理像素）。
    pub fn measure(&mut self, text: &str, px: f32) -> f32 {
        if self.fonts.is_empty() {
            return text.chars().count() as f32 * px * 0.55;
        }
        let key_px = quantize(px);
        let mut width = 0.0;
        let mut prev: Option<(usize, char)> = None;
        for ch in text.chars() {
            let Some(fi) = self.pick(ch) else {
                self.note_missing(ch);
                width += px * 0.5;
                prev = None;
                continue;
            };
            if let Some((pf, pch)) = prev {
                if pf == fi {
                    if let Some(kern) = self.fonts[fi].horizontal_kern(pch, ch, dequantize(key_px))
                    {
                        width += kern;
                    }
                }
            }
            width += self.metrics(fi, ch, key_px).advance_width;
            prev = Some((fi, ch));
        }
        width
    }

    fn metrics(&mut self, font: usize, ch: char, key_px: u16) -> fontdue::Metrics {
        let key = GlyphKey {
            font: font as u8,
            ch: ch as u32,
            px: key_px,
        };
        if let Some(cached) = self.cache.get(&key) {
            return cached.metrics;
        }
        let metrics = self.fonts[font].metrics(ch, dequantize(key_px));
        self.cache.insert(
            key,
            Cached {
                metrics,
                bitmap: None,
            },
        );
        metrics
    }

    fn note_missing(&mut self, ch: char) {
        if self.missing.insert(ch) {
            log::warn!("text: no font has glyph U+{:04X} (`{ch}`)", ch as u32);
        }
    }

    /// 画一行文本。`baseline` 是基线（物理像素）。
    ///
    /// `clip` 是物理像素的裁剪框 `(x0, y0, x1, y1)`：字形是逐像素混合的，
    /// 绕过了 skia 的 mask，所以裁剪必须在这里自己做（滚动内容需要）。
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
        if self.fonts.is_empty() || px < 1.0 {
            return;
        }
        let key_px = quantize(px);
        let mut cursor = x;
        for ch in text.chars() {
            if ch == ' ' {
                cursor += self.space_advance(key_px);
                continue;
            }
            let Some(fi) = self.pick(ch) else {
                self.note_missing(ch);
                cursor += px * 0.5;
                continue;
            };
            let key = GlyphKey {
                font: fi as u8,
                ch: ch as u32,
                px: key_px,
            };
            // 惰性光栅：只画到才栅格化，纯布局的 measure 不会付这份钱。
            if self
                .cache
                .get(&key)
                .map(|c| c.bitmap.is_none())
                .unwrap_or(true)
            {
                let (metrics, bitmap) = self.fonts[fi].rasterize(ch, dequantize(key_px));
                self.cache.insert(
                    key,
                    Cached {
                        metrics,
                        bitmap: Some(bitmap),
                    },
                );
            }
            let cached = self.cache.get(&key).expect("just inserted");
            let metrics = cached.metrics;
            if let Some(bitmap) = cached.bitmap.as_deref() {
                let top = baseline - (metrics.ymin + metrics.height as i32) as f32;
                let left = cursor + metrics.xmin as f32;
                blit_glyph(
                    pixmap,
                    bitmap,
                    metrics.width,
                    metrics.height,
                    left,
                    top,
                    color,
                    clip,
                );
                if bold {
                    blit_glyph(
                        pixmap,
                        bitmap,
                        metrics.width,
                        metrics.height,
                        left + 0.7,
                        top,
                        color,
                        clip,
                    );
                }
            }
            cursor += metrics.advance_width;
        }
    }

    fn space_advance(&mut self, key_px: u16) -> f32 {
        self.fonts
            .first()
            .map(|f| f.metrics(' ', dequantize(key_px)).advance_width)
            .unwrap_or(dequantize(key_px) * 0.3)
    }

    /// 单个字符的推进宽度（含缺字形兜底）。
    fn advance(&mut self, ch: char, px: f32) -> f32 {
        if self.fonts.is_empty() {
            return px * 0.55;
        }
        let key_px = quantize(px);
        match self.pick(ch) {
            Some(fi) => self.metrics(fi, ch, key_px).advance_width,
            None => {
                self.note_missing(ch);
                px * 0.5
            }
        }
    }

    /// 单行最大可用宽度内折行。
    ///
    /// CJK 可以在任意字符间断开，拉丁文优先在空格断 —— 这正是两种语言混排时
    /// 唯一都说得过去的行为。不追求 Knuth 那种最优断行：面板标签都很短。
    pub fn wrap(&mut self, text: &str, px: f32, max_width: f32) -> Vec<String> {
        if max_width <= 0.0 {
            return vec![text.to_string()];
        }
        let mut lines = Vec::new();
        let mut line = String::new();
        let mut width = 0.0f32;
        // 最近一次可以断行的位置：行内字节下标 + 断点之前的宽度。
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
                        // 在空格处断开：剩下的部分挪到下一行（吞掉前导空格）。
                        let rest: String = line[idx..].trim_start().to_string();
                        line.truncate(idx);
                        lines.push(line.trim_end().to_string());
                        width = self.measure(&rest, px);
                        line = rest;
                    }
                    None => {
                        // 没有空格（纯 CJK）：就在当前位置硬断。
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
        if self.measure(text, px) <= max_width {
            return text.to_string();
        }
        let ellipsis = "\u{2026}";
        let budget = (max_width - self.measure(ellipsis, px)).max(0.0);
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
        self.fonts
            .first()
            .and_then(|f| f.horizontal_line_metrics(px))
            .map(|m| m.new_line_size)
            .unwrap_or(px * 1.35)
    }

    /// 从基线到顶部的距离（物理像素），用于垂直居中对齐。
    pub fn ascent(&self, px: f32) -> f32 {
        self.fonts
            .first()
            .and_then(|f| f.horizontal_line_metrics(px))
            .map(|m| m.ascent)
            .unwrap_or(px * 0.8)
    }
}

fn quantize(px: f32) -> u16 {
    // 0.25px 粒度：DPR 1.0/1.25/1.5/1.75/2.0 各占一档，缓存不会碎成渣。
    (px * 4.0).round().clamp(4.0, 4096.0) as u16
}

/// 缓存键里的 1/4px 还原成真实像素尺寸。
///
/// **别直接拿 key 当 px 用**：它被放大过 4 倍，忘了除会让字号变成四倍
/// （第一版就是这么把标题画成巨无霸的）。
fn dequantize(key: u16) -> f32 {
    key as f32 / 4.0
}

/// 把 fontdue 的 8bit 覆盖率位图混合到预乘 RGBA 画布上。
///
/// 参数确实多，但每一个都是"一次 blit 需要的原始量"，拆成结构体只会多一层
/// 不必要的分配（这个函数每帧被调用几百次）。
#[allow(clippy::too_many_arguments)]
fn blit_glyph(
    pixmap: &mut Pixmap,
    bitmap: &[u8],
    width: usize,
    height: usize,
    left: f32,
    top: f32,
    color: Rgba,
    clip: Option<(f32, f32, f32, f32)>,
) {
    let ox = left.round() as i32;
    let oy = top.round() as i32;
    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let (cx0, cy0, cx1, cy1) = match clip {
        Some((x0, y0, x1, y1)) => (
            (x0.floor() as i32).max(0),
            (y0.floor() as i32).max(0),
            (x1.ceil() as i32).min(pw),
            (y1.ceil() as i32).min(ph),
        ),
        None => (0, 0, pw, ph),
    };
    let data = pixmap.data_mut();
    let src_a = color.a as u32;
    // 预乘的源色（未乘覆盖率）
    let sr = color.r as u32 * src_a / 255;
    let sg = color.g as u32 * src_a / 255;
    let sb = color.b as u32 * src_a / 255;

    for gy in 0..height {
        let y = oy + gy as i32;
        if y < cy0 || y >= cy1 {
            continue;
        }
        for gx in 0..width {
            let x = ox + gx as i32;
            if x < cx0 || x >= cx1 {
                continue;
            }
            let cov = bitmap[gy * width + gx] as u32;
            if cov == 0 {
                continue;
            }
            let sa = src_a * cov / 255;
            let inv = 255 - sa;
            let i = ((y * pw + x) * 4) as usize;
            let r = sr * cov / 255 + data[i] as u32 * inv / 255;
            let g = sg * cov / 255 + data[i + 1] as u32 * inv / 255;
            let b = sb * cov / 255 + data[i + 2] as u32 * inv / 255;
            let a = sa + data[i + 3] as u32 * inv / 255;
            data[i] = r.min(255) as u8;
            data[i + 1] = g.min(255) as u8;
            data[i + 2] = b.min(255) as u8;
            data[i + 3] = a.min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_is_monotonic() {
        TextEngine::with(|t| {
            if !t.has_fonts() {
                return;
            }
            let short = t.measure("设置", 14.0);
            let long = t.measure("设置面板", 14.0);
            assert!(short > 0.0);
            assert!(long > short, "{long} <= {short}");
            // 大小翻倍，宽度大致翻倍
            let big = t.measure("设置", 28.0);
            assert!((big / short - 2.0).abs() < 0.2, "{big} vs {short}");
        });
    }

    #[test]
    fn quantize_keeps_common_dprs_distinct() {
        assert_ne!(quantize(13.0), quantize(16.25));
        assert_eq!(quantize(14.0), quantize(14.0));
    }

    #[test]
    fn wrap_respects_width() {
        TextEngine::with(|t| {
            if !t.has_fonts() {
                return;
            }
            let text = "鼠标停在驺吾身上可以展开一圈气泡";
            let lines = t.wrap(text, 12.0, 60.0);
            assert!(lines.len() > 1, "expected wrapping, got {lines:?}");
            for line in &lines {
                assert!(t.measure(line, 12.0) <= 61.0, "line too wide: {line:?}");
            }
            assert_eq!(lines.concat(), text);
        });
    }

    #[test]
    fn truncate_adds_ellipsis() {
        TextEngine::with(|t| {
            if !t.has_fonts() {
                return;
            }
            let out = t.truncate("abcdefghijklmnop", 12.0, 40.0);
            assert!(out.ends_with('…'));
            assert!(out.chars().count() < 16);
        });
    }
}
