//! 控件层：在 [`tiny_skia::Pixmap`] 上画"表格状"界面的最小积木。
//!
//! 为什么不用 egui / iced / slint：整个项目只有这一个面板需要控件，
//! 引一个即时模式 UI 反而会带来第二套字体、第二套主题、第二套事件循环
//! （egui 要 `winit` 的输入转译，slint 要自己的渲染后端）。这里的控件
//! 只有五种（开关、步进、选择、按钮、文本），一百多行足够，且完全长在
//! 我们自己的 `Theme` 与 `TextEngine` 上 —— 换肤对面板同样生效。
//!
//! 坐标约定：**逻辑像素**。`Painter` 内部乘 `scale` 转物理像素，
//! 控件代码不需要知道 DPI。

use tiny_skia::{Color, FillRule, Mask, Paint, PathBuilder, Pixmap, Stroke, Transform};
use zouwu_core::geom::Point;
use zouwu_core::theme::Rgba;

use crate::text::TextEngine;

/// 逻辑矩形。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// 向内收 `dx`（水平）/ `dy`（垂直）。
    pub fn inset(&self, dx: f32, dy: f32) -> Self {
        Self::new(
            self.x + dx,
            self.y + dy,
            self.w - dx * 2.0,
            self.h - dy * 2.0,
        )
    }

    pub fn translated(&self, dy: f32) -> Self {
        Self::new(self.x, self.y + dy, self.w, self.h)
    }

    /// 是否与另一个矩形相交（用于"这一行要不要画/能不能点"）。
    pub fn intersects(&self, other: &Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    fn phys(&self, s: f32) -> (f32, f32, f32, f32) {
        (self.x * s, self.y * s, self.w * s, self.h * s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Bold,
}

pub struct Painter<'a> {
    pixmap: &'a mut Pixmap,
    scale: f32,
    /// 裁剪区（逻辑坐标）。控件滚动需要把内容挡在窄带里。
    clip: Option<Rect>,
    mask: Option<Mask>,
    /// 裁剪区的物理像素边界，给文本混合用（它绕过 skia 的 mask）。
    clip_px: Option<(f32, f32, f32, f32)>,
}

impl<'a> Painter<'a> {
    pub fn new(pixmap: &'a mut Pixmap, scale: f32) -> Self {
        Self {
            pixmap,
            scale,
            clip: None,
            mask: None,
            clip_px: None,
        }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// 设定裁剪区。掩膜按需重建 —— 面板只在滚动/悬停时重绘，这点成本可忽略。
    pub fn set_clip(&mut self, clip: Rect) {
        if self.clip == Some(clip) {
            return;
        }
        self.clip = Some(clip);
        let (x, y, w, h) = clip.phys(self.scale);
        self.clip_px = Some((x, y, x + w, y + h));
        let mut mask = match Mask::new(self.pixmap.width(), self.pixmap.height()) {
            Some(mask) => mask,
            None => return,
        };
        let mut pb = PathBuilder::new();
        pb.push_rect(
            tiny_skia::Rect::from_xywh(x, y, w, h).unwrap_or_else(|| {
                tiny_skia::Rect::from_xywh(0.0, 0.0, 1.0, 1.0).expect("unit rect")
            }),
        );
        if let Some(path) = pb.finish() {
            mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        }
        self.mask = Some(mask);
    }

    /// 取消裁剪（画完滚动内容、要画页脚时用）。
    pub fn clear_clip(&mut self) {
        self.clip = None;
        self.mask = None;
        self.clip_px = None;
    }

    fn px(&self, v: f32) -> f32 {
        v * self.scale
    }

    fn border_px(&self) -> f32 {
        // 1 物理像素的线在 1.25 DPI 上会"半亮"，这里至少保证一个物理像素。
        (1.0 / self.scale).max(0.5)
    }

    pub fn fill(&mut self, rect: Rect, radius: f32, color: Rgba) {
        if rect.is_empty() || color.a == 0 {
            return;
        }
        let Some(path) = rounded_path(rect, radius, self.scale) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(to_color(color));
        paint.anti_alias = true;
        let mask = self.mask.as_ref();
        self.pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            mask,
        );
    }

    pub fn stroke(&mut self, rect: Rect, radius: f32, width: f32, color: Rgba) {
        let Some(path) = rounded_path(rect, radius, self.scale) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(to_color(color));
        paint.anti_alias = true;
        let stroke = Stroke {
            width: self.px(width.max(self.border_px())),
            ..Stroke::default()
        };
        let mask = self.mask.as_ref();
        self.pixmap
            .stroke_path(&path, &paint, &stroke, Transform::identity(), mask);
    }

    pub fn circle(&mut self, center: Point, radius: f32, color: Rgba) {
        let r = self.px(radius);
        if r <= 0.0 {
            return;
        }
        let Some(path) = PathBuilder::from_circle(self.px(center.x), self.px(center.y), r) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(to_color(color));
        paint.anti_alias = true;
        let mask = self.mask.as_ref();
        self.pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            mask,
        );
    }

    /// 水平细线（分隔线、进度条）。
    pub fn hline(&mut self, x0: f32, x1: f32, y: f32, color: Rgba) {
        let t = self.border_px();
        self.fill(Rect::new(x0, y, (x1 - x0).max(0.0), t), 0.0, color);
    }

    /// 画一行文本，`baseline` 为基线（逻辑像素）。
    pub fn text(
        &mut self,
        text: &str,
        x: f32,
        baseline: f32,
        size: f32,
        color: Rgba,
        weight: Weight,
    ) {
        if text.is_empty() || color.a == 0 {
            return;
        }
        let px = self.px(size);
        // 亚像素尺寸会让 hinting 发抖，四舍五入到 1/4 物理像素。
        let px = (px * 4.0).round() / 4.0;
        let bold = weight == Weight::Bold;
        TextEngine::with(|engine| {
            engine.draw(
                self.pixmap,
                text,
                self.px(x),
                self.px(baseline),
                px,
                color,
                bold,
                self.clip_px,
            );
        });
    }

    /// 在 `rect` 内垂直居中、左对齐画文本。
    pub fn text_in(&mut self, text: &str, rect: Rect, size: f32, color: Rgba, weight: Weight) {
        let baseline = self.centered_baseline(rect, size);
        self.text(text, rect.x, baseline, size, color, weight);
    }

    /// 在 `rect` 内水平居中 + 垂直居中。
    pub fn text_center(&mut self, text: &str, rect: Rect, size: f32, color: Rgba, weight: Weight) {
        let width = self.measure(text, size, weight);
        let baseline = self.centered_baseline(rect, size);
        self.text(
            text,
            rect.x + (rect.w - width) * 0.5,
            baseline,
            size,
            color,
            weight,
        );
    }

    /// 在 `rect` 内右对齐 + 垂直居中。
    pub fn text_right(&mut self, text: &str, rect: Rect, size: f32, color: Rgba, weight: Weight) {
        let width = self.measure(text, size, weight);
        let baseline = self.centered_baseline(rect, size);
        self.text(text, rect.right() - width, baseline, size, color, weight);
    }

    fn centered_baseline(&self, rect: Rect, size: f32) -> f32 {
        let ascent = TextEngine::with(|engine| engine.ascent(size * self.scale)) / self.scale;
        rect.y + (rect.h + ascent) * 0.5 - size * 0.5 * 0.16
    }

    pub fn measure(&mut self, text: &str, size: f32, weight: Weight) -> f32 {
        let px = self.px(size);
        let width = TextEngine::with(|engine| engine.measure(text, px));
        let width = if weight == Weight::Bold {
            // 粗体是"偏移 0.7px 再画一遍"，宽度也要留出这份余量。
            width + self.px(0.7)
        } else {
            width
        };
        width / self.scale
    }

    pub fn line_height(&mut self, size: f32) -> f32 {
        let px = self.px(size);
        TextEngine::with(|engine| engine.line_height(px)) / self.scale
    }

    pub fn truncate(&mut self, text: &str, size: f32, max_width: f32) -> String {
        let px = self.px(size);
        let max = self.px(max_width.max(0.0));
        TextEngine::with(|engine| engine.truncate(text, px, max))
    }
}

fn to_color(c: Rgba) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

fn rounded_path(rect: Rect, radius: f32, s: f32) -> Option<tiny_skia::Path> {
    let (x, y, w, h) = rect.phys(s);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = (radius * s).clamp(0.0, w.min(h) * 0.5);
    if r <= 0.5 {
        return Some(PathBuilder::from_rect(tiny_skia::Rect::from_xywh(
            x, y, w, h,
        )?));
    }
    let (l, t, right, bottom) = (x, y, x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(l + r, t);
    pb.line_to(right - r, t);
    pb.quad_to(right, t, right, t + r);
    pb.line_to(right, bottom - r);
    pb.quad_to(right, bottom, right - r, bottom);
    pb.line_to(l + r, bottom);
    pb.quad_to(l, bottom, l, bottom - r);
    pb.line_to(l, t + r);
    pb.quad_to(l, t, l + r, t);
    pb.close();
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_uses_half_open_interval() {
        let r = Rect::new(10.0, 10.0, 20.0, 10.0);
        assert!(r.contains(Point::new(10.0, 10.0)));
        assert!(r.contains(Point::new(29.9, 19.9)));
        assert!(!r.contains(Point::new(30.0, 15.0)));
        assert!(!r.contains(Point::new(15.0, 20.0)));
    }

    #[test]
    fn intersects_detects_overlap() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        let c = Rect::new(10.0, 0.0, 5.0, 5.0);
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
    }
}
