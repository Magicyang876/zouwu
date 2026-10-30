//! 用 tiny-skia 把一"帧场景"光栅化到 Pixmap。
//!
//! tiny-skia 的 API 与 Skia 高度一致，后续若换成 `skia-safe`（GPU 后端），
//! 这里的绘制代码可以基本平移，因此 M0 先用纯 Rust 光栅器：
//! * 零 C++ 依赖，编译/分发简单；
//! * 340×340 的画面 CPU 光栅成本极低（< 0.3ms）；
//! * 空闲时根本不调用它。

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};
use zouwu_core::action::Glyph;
use zouwu_core::geom::Point;
use zouwu_core::skin::LayerRole;
use zouwu_core::theme::{Rgba, Theme};

use crate::skin::Skin;
use crate::widget::{Painter, Weight};

/// 代码绘制小兽的呼吸幅度（±3%）。位图皮肤用它自己的 `geometry.breath_amount`。
pub const DEFAULT_BREATH_AMOUNT: f32 = 0.03;

pub struct BubblePaint<'a> {
    pub center: Point,
    pub radius: f32,
    /// 弹簧值：0 = 收起，1 = 完全展开（可能略大于 1，即过冲）
    pub scale: f32,
    pub alpha: f32,
    pub glyph: Glyph,
    /// 自定义图标（PNG 位图）；有它就不画字形。
    pub icon: Option<&'a Pixmap>,
    pub selected: bool,
}

/// 一条子项激光条（`open_children` 展开时才有）。
pub struct BeamPaint<'a> {
    /// 起点：父球外沿
    pub base: Point,
    /// 终点：文字牌中心
    pub tip: Point,
    /// 生长进度 0..1（弹簧值，用来做"射出去"的动画）
    pub progress: f32,
    /// 文字牌（逻辑坐标）
    pub chip: zouwu_core::geom::Rect,
    pub label: &'a str,
    /// 鼠标停在这条牌子上
    pub hovered: bool,
}

pub struct Scene<'a> {
    /// 逻辑宽高
    pub width: f32,
    pub height: f32,
    /// DPI 缩放（物理 = 逻辑 × scale）
    pub scale: f32,
    pub buddy_center: Point,
    pub buddy_radius: f32,
    /// 悬停时的轻微放大（弹簧值）
    pub buddy_scale: f32,
    pub buddy_alpha: f32,
    /// 眨眼量：0 = 睁眼，1 = 闭眼
    pub blink: f32,
    /// 呼吸相位：0 = 常态，1 = 吸气顶点。**幅度**由皮肤决定，见 [`Scene::body_radius`]。
    pub breath: f32,
    pub theme: &'a Theme,
    pub bubbles: &'a [BubblePaint<'a>],
    /// 位图皮肤；`None` 或 `procedural` → 走代码绘制。
    pub skin: Option<&'a Skin>,
    /// 子项激光条（画在气泡之上）
    pub beams: &'a [BeamPaint<'a>],
}

impl Scene<'_> {
    /// 本体当前半径 = 基础半径 × 悬停弹簧 × (1 + 呼吸幅度 × 呼吸相位)。
    ///
    /// 呼吸幅度交给皮肤决定：手绘形象与代码绘制小兽的体积感不一样，
    /// 但"什么时候呼吸、呼多久"仍然是 App 统一管的（用户设置优先于皮肤）。
    pub fn body_radius(&self) -> f32 {
        let amp = match self.skin {
            Some(skin) => skin.breath_amount(),
            None => DEFAULT_BREATH_AMOUNT,
        };
        self.buddy_radius * self.buddy_scale * (1.0 + amp * self.breath)
    }
}

struct Ctx<'a> {
    pixmap: &'a mut Pixmap,
    s: f32,
}

fn col(c: Rgba, alpha_mul: f32) -> Color {
    let c = c.with_alpha(alpha_mul);
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

impl Ctx<'_> {
    fn px(&self, v: f32) -> f32 {
        v * self.s
    }

    fn point(&self, p: Point) -> (f32, f32) {
        (p.x * self.s, p.y * self.s)
    }

    fn fill_circle(&mut self, center: Point, radius: f32, color: Color) {
        let (cx, cy) = self.point(center);
        let r = self.px(radius);
        if r <= 0.0 {
            return;
        }
        if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.anti_alias = true;
            self.pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    /// 支持椭圆的圆（缩放 + 平移），用于眯眼等变形。
    fn fill_ellipse(&mut self, center: Point, radius: f32, sx: f32, sy: f32, color: Color) {
        let (cx, cy) = self.point(center);
        let r = self.px(radius);
        if r <= 0.0 || sx <= 0.0 || sy <= 0.0 {
            return;
        }
        if let Some(path) = PathBuilder::from_circle(0.0, 0.0, r) {
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.anti_alias = true;
            let ts = Transform::from_scale(sx, sy).post_translate(cx, cy);
            self.pixmap
                .fill_path(&path, &paint, FillRule::Winding, ts, None);
        }
    }

    /// 把一张位图画进图标区：等比缩放后居中，占 `size` 见方。
    ///
    /// 图不一定小，这里是**整个程序唯一会把用户 PNG 贴到屏上的地方**：
    /// 缩放里那个 `self.s` 是 DPI 缩放，漏乘就会在高 DPI 下缩成一小块。
    fn draw_icon(&mut self, icon: &Pixmap, center: Point, size: f32, alpha: f32) {
        let (pw, ph) = (icon.width() as f32, icon.height() as f32);
        let target = self.px(size);
        if pw <= 0.0 || ph <= 0.0 || target <= 0.0 {
            return;
        }
        let k = (target / pw).min(target / ph);
        let (cx, cy) = self.point(center);
        let ts = Transform::from_scale(k, k).post_translate(cx - pw * k * 0.5, cy - ph * k * 0.5);
        let paint = PixmapPaint {
            opacity: alpha.clamp(0.0, 1.0),
            blend_mode: tiny_skia::BlendMode::SourceOver,
            quality: tiny_skia::FilterQuality::Bilinear,
        };
        self.pixmap
            .as_mut()
            .draw_pixmap(0, 0, icon.as_ref(), &paint, ts, None);
    }

    fn stroke_circle(&mut self, center: Point, radius: f32, width: f32, color: Color) {
        let (cx, cy) = self.point(center);
        let r = self.px(radius);
        if r <= 0.0 {
            return;
        }
        if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.anti_alias = true;
            let stroke = Stroke {
                width: self.px(width).max(1.0),
                ..Stroke::default()
            };
            self.pixmap
                .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    fn line(&mut self, a: Point, b: Point, width: f32, color: Color) {
        let mut pb = PathBuilder::new();
        let (ax, ay) = self.point(a);
        let (bx, by) = self.point(b);
        pb.move_to(ax, ay);
        pb.line_to(bx, by);
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.anti_alias = true;
            let stroke = Stroke {
                width: self.px(width).max(1.0),
                ..Stroke::default()
            };
            self.pixmap
                .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    fn polyline(&mut self, pts: &[Point], width: f32, color: Color) {
        if pts.len() < 2 {
            return;
        }
        let mut pb = PathBuilder::new();
        let (x0, y0) = self.point(pts[0]);
        pb.move_to(x0, y0);
        for p in &pts[1..] {
            let (x, y) = self.point(*p);
            pb.line_to(x, y);
        }
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.anti_alias = true;
            let stroke = Stroke {
                width: self.px(width).max(1.0),
                ..Stroke::default()
            };
            self.pixmap
                .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    fn fill_triangle(&mut self, a: Point, b: Point, c: Point, color: Color) {
        let mut pb = PathBuilder::new();
        let (ax, ay) = self.point(a);
        let (bx, by) = self.point(b);
        let (cx, cy) = self.point(c);
        pb.move_to(ax, ay);
        pb.line_to(bx, by);
        pb.line_to(cx, cy);
        pb.close();
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.anti_alias = true;
            self.pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn fill_round_rect(&mut self, center: Point, w: f32, h: f32, radius: f32, color: Color) {
        let mut pb = PathBuilder::new();
        let (cx, cy) = self.point(center);
        let hw = self.px(w) * 0.5;
        let hh = self.px(h) * 0.5;
        let r = self.px(radius).min(hw).min(hh);
        let (l, t, rr, b) = (cx - hw, cy - hh, cx + hw, cy + hh);
        pb.move_to(l + r, t);
        pb.line_to(rr - r, t);
        pb.quad_to(rr, t, rr, t + r);
        pb.line_to(rr, b - r);
        pb.quad_to(rr, b, rr - r, b);
        pb.line_to(l + r, b);
        pb.quad_to(l, b, l, b - r);
        pb.line_to(l, t + r);
        pb.quad_to(l, t, l + r, t);
        pb.close();
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.anti_alias = true;
            self.pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

pub fn render(scene: &Scene<'_>, pixmap: &mut Pixmap) {
    pixmap.fill(Color::TRANSPARENT);
    let theme = scene.theme;
    let mut ctx = Ctx {
        pixmap,
        s: scene.scale,
    };

    draw_skin_or_buddy(&mut ctx, scene);
    draw_bubbles(&mut ctx, scene, theme);
    draw_beams(&mut ctx, scene, theme);
}

/// 子项激光条 + 文字牌（ADR-045）。
///
/// 光条画三层（外晕 / 中亮 / 芯），一层比一层细、一层比一层亮，看起来才像
/// "射出去的光"而不是一条直线。文字牌最后画，盖住光条的尾巴。
fn draw_beams(ctx: &mut Ctx<'_>, scene: &Scene<'_>, theme: &Theme) {
    if scene.beams.is_empty() {
        return;
    }
    let scale = ctx.s;
    for beam in scene.beams {
        let p = beam.progress.clamp(0.0, 1.2);
        if p <= 0.01 {
            continue;
        }
        // 光条长度跟着进度长出去，略微过冲（弹簧给的值常常 >1）。
        let tip = Point::new(
            beam.base.x + (beam.tip.x - beam.base.x) * p,
            beam.base.y + (beam.tip.y - beam.base.y) * p,
        );
        let dx = tip.x - beam.base.x;
        let dy = tip.y - beam.base.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.5 {
            continue;
        }
        // 垂直方向单位向量：光条是从 `base` 到 `tip` 的锥形四边形。
        let (nx, ny) = (-dy / len, dx / len);
        let (w0, w1) = (2.4, 0.9);
        for (w_mul, alpha, color) in [
            (3.2, 0.10, theme.glow),
            (1.7, 0.34, theme.glow),
            (1.0, 0.92, theme.accent),
        ] {
            let (a0, a1) = (w0 * w_mul, w1 * w_mul);
            let mut pb = PathBuilder::new();
            pb.move_to(
                beam.base.x * scale + nx * a0 * scale,
                beam.base.y * scale + ny * a0 * scale,
            );
            pb.line_to(
                tip.x * scale + nx * a1 * scale,
                tip.y * scale + ny * a1 * scale,
            );
            pb.line_to(
                tip.x * scale - nx * a1 * scale,
                tip.y * scale - ny * a1 * scale,
            );
            pb.line_to(
                beam.base.x * scale - nx * a0 * scale,
                beam.base.y * scale - ny * a0 * scale,
            );
            pb.close();
            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color(col(color, alpha));
                paint.anti_alias = true;
                ctx.pixmap.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }

        // 牌子在光条长到一半之后淡入：先看到光射出去，再读到字。
        let fade = ((p - 0.5) / 0.5).clamp(0.0, 1.0);
        if fade <= 0.01 {
            continue;
        }
        let chip = crate::widget::Rect::new(beam.chip.x, beam.chip.y, beam.chip.w, beam.chip.h);
        let bg = if beam.hovered {
            theme.bubble_bg_active
        } else {
            theme.bubble_bg
        };
        let edge = if beam.hovered {
            theme.bubble_edge_active
        } else {
            theme.bubble_edge
        };
        let mut painter = Painter::new(ctx.pixmap, scale);
        painter.fill(chip, 7.0, bg.with_alpha(0.94 * fade));
        painter.stroke(chip, 7.0, 1.0, edge.with_alpha(fade));
        let text_color = if beam.hovered {
            theme.glyph_active
        } else {
            theme.glyph
        };
        let textc = text_color.with_alpha(fade);
        let label = painter.truncate(beam.label, CHIP_FONT, chip.w - 12.0);
        painter.text_in(
            &label,
            crate::widget::Rect::new(chip.x, chip.y, chip.w, chip.h),
            CHIP_FONT,
            textc,
            Weight::Regular,
        );
    }
}

/// 文字牌字号（逻辑像素）。与 `app.rs` 里量文字用的字号必须一致。
pub const CHIP_FONT: f32 = 11.0;

/// 本体：有位图皮肤就贴图层，否则代码绘制。
fn draw_skin_or_buddy(ctx: &mut Ctx<'_>, scene: &Scene<'_>) {
    match scene.skin {
        Some(skin) if !skin.is_procedural() => draw_skin_buddy(ctx, scene, skin),
        _ => draw_buddy(ctx, scene),
    }
}

/// 投影的形状参数。位图皮肤要更"贴地"一点：
///
/// 代码绘制的小兽自带一圈宽身体，影子拉开看很自然；
/// 位图皮肤常常是**黑/深色**的角色（比如水墨驺吾），
/// 影子一拉开就变成"角色脚下飘着一条红带子"——所以收窄、贴紧、减淡。
#[derive(Clone, Copy)]
struct ShadowStyle {
    /// 相对半径的垂直偏移
    offset: f32,
    /// 椭圆的横向半宽（× 半径）
    spread: f32,
    /// 椭圆的纵向半高（× 半径）
    height: f32,
    /// 整体浓度
    strength: f32,
}

impl ShadowStyle {
    const PROCEDURAL: Self = Self {
        offset: 0.94,
        spread: 1.04,
        height: 0.14,
        strength: 1.0,
    };
    const BITMAP: Self = Self {
        offset: 0.80,
        spread: 0.92,
        height: 0.09,
        strength: 0.70,
    };
}

/// 地面投影：多层同心扁椭圆叠出柔和渐变（比模糊成本低一个数量级）。
fn draw_shadow(
    ctx: &mut Ctx<'_>,
    center: Point,
    radius: f32,
    alpha: f32,
    theme: &Theme,
    style: ShadowStyle,
) {
    for i in (1..=12).rev() {
        let k = i as f32 / 12.0;
        ctx.fill_ellipse(
            center,
            radius * (style.spread * k),
            1.0,
            style.height,
            col(theme.shadow, 0.055 * alpha * style.strength),
        );
    }
}

/// 位图皮肤：按清单顺序从下往上叠图层。
///
/// 三层变换的关系（顺序不能乱）：
/// 1. **整体**：呼吸 × 悬停弹簧 × 透明度（`Scene::body_radius`）；
/// 2. **图层**：各自 `scale` / `offset` / `opacity`；
/// 3. **眨眼**：只影响 `role = "blink"` 的图层，透明度随 `scene.blink` 淡入。
fn draw_skin_buddy(ctx: &mut Ctx<'_>, scene: &Scene<'_>, skin: &Skin) {
    let a = scene.buddy_alpha;
    if a <= 0.001 {
        return;
    }
    let c = scene.buddy_center;
    let r = scene.body_radius();
    // 偏移量是按设计尺寸写的，整体缩放时偏移也要跟着缩，否则放大时五官会散开。
    let k = if scene.buddy_radius > 0.0 {
        r / scene.buddy_radius
    } else {
        1.0
    };

    draw_shadow(
        ctx,
        Point::new(c.x, c.y + r * ShadowStyle::BITMAP.offset),
        r,
        a,
        scene.theme,
        ShadowStyle::BITMAP,
    );

    for layer in &skin.layers {
        let alpha = a
            * layer.opacity
            * match layer.role {
                LayerRole::Blink => scene.blink.clamp(0.0, 1.0),
                _ => 1.0,
            };
        if alpha <= 0.001 {
            continue;
        }
        let (w, h) = layer.draw_size(r);
        if w <= 0.5 || h <= 0.5 {
            continue;
        }

        let (pw, ph) = (layer.pixmap.width() as f32, layer.pixmap.height() as f32);
        let tx = c.x + layer.offset_x * k - w * 0.5;
        let ty = c.y + layer.offset_y * k - h * 0.5;
        // 贴图是唯一绕过 `Ctx` 画笔的地方：`draw_pixmap` 收的是**物理像素**，
        // 而上面所有坐标都是逻辑像素（`c`、`w`、`h` 全是逻辑）。
        // 忘记乘 `ctx.s` 的后果：125% 缩放下整只兽被画到 1/1.25 的位置，
        // 于是"影子在环中心、本体偏左上"——一个只在非 100% DPI 上才会出现的错位。
        let s = ctx.s;
        let transform =
            Transform::from_scale(w * s / pw, h * s / ph).post_translate(tx * s, ty * s);
        let paint = PixmapPaint {
            opacity: alpha,
            blend_mode: tiny_skia::BlendMode::SourceOver,
            quality: tiny_skia::FilterQuality::Bilinear,
        };
        ctx.pixmap
            .as_mut()
            .draw_pixmap(0, 0, layer.pixmap.as_ref(), &paint, transform, None);
    }
}

/// 生成托盘图标用的**非预乘** RGBA 像素（Windows `NOTIFYICONDATA` 要的就是这个）。
///
/// 复用桌面上的同一套画笔，所以托盘里那一小格和屏幕上的小兽是同一只，
/// 不需要另做一份图标资源（皮肤换掉时调色板变了，托盘也跟着变）。
///
/// `alpha` 会就地做预乘 → 直通转换：tiny-skia 输出的是预乘 alpha，
/// 直接拿去当图标会让边缘发黑（预乘值被当成直通值再乘一次）。
pub fn tray_icon(theme: &Theme, skin: Option<&Skin>, size: u32) -> (Vec<u8>, u32, u32) {
    let size = size.clamp(16, 256);
    let mut pixmap = Pixmap::new(size, size).expect("tray icon pixmap");
    let logical = size as f32;
    let radius = logical * 0.40;
    let scene = Scene {
        width: logical,
        height: logical,
        scale: 1.0,
        buddy_center: Point::new(logical * 0.5, logical * 0.52),
        // 托盘格子很小：位图皮肤按同一套几何缩小到 0.4 格，与代码绘制保持一致。
        buddy_radius: skin
            .filter(|s| !s.is_procedural())
            .map(|s| radius * s.manifest.geometry.radius / 56.0)
            .unwrap_or(radius),
        buddy_scale: 1.0,
        buddy_alpha: 1.0,
        blink: 0.0,
        breath: 0.0,
        theme,
        bubbles: &[],
        beams: &[],
        skin,
    };
    render(&scene, &mut pixmap);

    let mut rgba = pixmap.data().to_vec();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            continue;
        }
        for c in &mut px[..3] {
            *c = (((*c as u32) * 255 + a / 2) / a).min(255) as u8;
        }
    }
    (rgba, size, size)
}

fn draw_buddy(ctx: &mut Ctx<'_>, scene: &Scene<'_>) {
    let theme = scene.theme;
    let a = scene.buddy_alpha;
    if a <= 0.001 {
        return;
    }
    let c = scene.buddy_center;
    let r = scene.body_radius();
    // 局部坐标：以本体中心为原点、以半径 r 为单位
    let p = |x: f32, y: f32| Point::new(c.x + r * x, c.y + r * y);

    draw_shadow(
        ctx,
        p(0.0, ShadowStyle::PROCEDURAL.offset),
        r,
        a,
        theme,
        ShadowStyle::PROCEDURAL,
    );

    // ---- 尾巴（在身体后面），带三道虎纹
    ctx.polyline(
        &[
            p(-0.58, 0.34),
            p(-1.04, 0.22),
            p(-1.26, -0.10),
            p(-1.10, -0.48),
        ],
        r * 0.21,
        col(theme.body, a),
    );
    for (x1, y1, x2, y2) in [
        (-0.84, 0.40, -0.78, 0.16),
        (-1.06, 0.26, -1.02, 0.02),
        (-1.22, -0.02, -1.24, -0.26),
    ] {
        ctx.line(
            p(x1, y1),
            p(x2, y2),
            r * 0.085,
            col(theme.body_shade, 0.75 * a),
        );
    }

    // ---- 后脚掌
    for k in [-1.0f32, 1.0] {
        ctx.fill_ellipse(p(0.40 * k, 0.92), r * 0.19, 1.15, 0.72, col(theme.body, a));
    }

    // ---- 耳朵：外耳三角 + 内耳浅色三角
    for k in [-1.0f32, 1.0] {
        ctx.fill_triangle(
            p(0.28 * k, -0.72),
            p(0.70 * k, -1.28),
            p(0.04 * k, -1.00),
            col(theme.body, a),
        );
        ctx.fill_triangle(
            p(0.34 * k, -0.80),
            p(0.58 * k, -1.13),
            p(0.20 * k, -0.97),
            col(theme.mane, 0.85 * a),
        );
    }

    // ---- 鬃毛：头周围一圈绒球，只露出外缘，形成蓬松轮廓
    let head = Point::new(0.0, -0.34);
    for i in 0..10 {
        let ang = i as f32 / 10.0 * std::f32::consts::TAU + 0.31;
        ctx.fill_circle(
            p(head.x + ang.cos() * 0.58, head.y + ang.sin() * 0.58),
            r * 0.27,
            col(theme.mane, 0.95 * a),
        );
    }

    // ---- 身体
    ctx.fill_circle(p(0.0, 0.22), r * 0.92, col(theme.body, a));
    // 胸口浅色（肚皮）
    ctx.fill_ellipse(
        p(0.0, 0.42),
        r * 0.50,
        1.0,
        0.80,
        col(theme.muzzle, 0.92 * a),
    );
    // 身体右侧暗部，给一点体积
    ctx.fill_ellipse(
        p(0.34, 0.50),
        r * 0.40,
        1.0,
        0.85,
        col(theme.body_shade, 0.18 * a),
    );
    // 前爪
    for k in [-1.0f32, 1.0] {
        ctx.fill_ellipse(
            p(0.32 * k, 0.84),
            r * 0.20,
            1.1,
            0.78,
            col(theme.muzzle, 0.95 * a),
        );
        ctx.line(
            p(0.32 * k, 0.80),
            p(0.32 * k, 0.90),
            r * 0.04,
            col(theme.body_shade, 0.45 * a),
        );
    }

    // ---- 头
    ctx.fill_circle(p(head.x, head.y), r * 0.68, col(theme.body, a));
    // 头顶柔光
    ctx.fill_ellipse(
        p(head.x - 0.20, head.y - 0.28),
        r * 0.34,
        1.0,
        0.62,
        col(Rgba::rgb(255, 255, 255), 0.10 * a),
    );
    // 虎纹（额侧两道）
    for k in [-1.0f32, 1.0] {
        ctx.line(
            p(0.44 * k, -0.74),
            p(0.56 * k, -0.52),
            r * 0.055,
            col(theme.body_shade, 0.50 * a),
        );
        ctx.line(
            p(0.50 * k, -0.56),
            p(0.60 * k, -0.42),
            r * 0.048,
            col(theme.body_shade, 0.42 * a),
        );
    }

    // ---- 口鼻区
    ctx.fill_ellipse(
        p(0.0, -0.18),
        r * 0.32,
        1.18,
        0.86,
        col(theme.muzzle, 0.97 * a),
    );

    // ---- 腮红
    for k in [-1.0f32, 1.0] {
        ctx.fill_ellipse(
            p(0.47 * k, -0.28),
            r * 0.10,
            1.0,
            0.72,
            col(theme.accent, 0.26 * a),
        );
    }

    // ---- 眼睛（眨眼 = 纵向压扁）
    let eye_open = (1.0 - scene.blink).clamp(0.05, 1.0);
    for k in [-1.0f32, 1.0] {
        let eye = p(0.27 * k, -0.47);
        ctx.fill_ellipse(eye, r * 0.105, 1.0, eye_open, col(theme.eye, a));
        if eye_open > 0.6 {
            ctx.fill_circle(
                Point::new(eye.x + r * 0.035, eye.y - r * 0.035),
                r * 0.032,
                col(Rgba::rgb(255, 255, 255), 0.88 * a),
            );
        }
    }

    // ---- 鼻子
    ctx.fill_triangle(
        p(-0.065, -0.265),
        p(0.065, -0.265),
        p(0.0, -0.205),
        col(theme.eye, 0.92 * a),
    );
    // ---- 嘴（一个小小的微笑）
    ctx.polyline(
        &[p(-0.09, -0.165), p(0.0, -0.115), p(0.09, -0.165)],
        r * 0.045,
        col(theme.eye, 0.55 * a),
    );

    // ---- 胡须
    for k in [-1.0f32, 1.0] {
        ctx.line(
            p(0.30 * k, -0.16),
            p(0.58 * k, -0.24),
            r * 0.030,
            col(theme.body_shade, 0.55 * a),
        );
        ctx.line(
            p(0.30 * k, -0.09),
            p(0.60 * k, -0.06),
            r * 0.030,
            col(theme.body_shade, 0.55 * a),
        );
    }
}

fn draw_bubbles(ctx: &mut Ctx<'_>, scene: &Scene<'_>, theme: &Theme) {
    for b in scene.bubbles {
        let a = b.alpha;
        if a <= 0.005 || b.scale <= 0.01 {
            continue;
        }
        let r = b.radius * b.scale;

        // 选中时的外发光（两层）
        if b.selected {
            ctx.stroke_circle(b.center, r + 5.0, 5.0, col(theme.glow, 0.16 * a));
            ctx.stroke_circle(b.center, r + 1.8, 2.2, col(theme.glow, 0.55 * a));
        }

        let bg = if b.selected {
            theme.bubble_bg_active
        } else {
            theme.bubble_bg
        };
        let edge = if b.selected {
            theme.bubble_edge_active
        } else {
            theme.bubble_edge
        };
        // 气泡底（略微半透明，让桌面透出来一点，像玻璃）
        ctx.fill_circle(b.center, r, col(bg, 0.90 * a));
        ctx.stroke_circle(b.center, r - 0.6, 1.4, col(edge, 0.9 * a));

        let g = if b.selected {
            theme.glyph_active
        } else {
            theme.glyph
        };
        match b.icon {
            Some(icon) => ctx.draw_icon(icon, b.center, r * 0.92, a),
            None => draw_glyph(ctx, b.glyph, b.center, r, col(g, a)),
        };
    }
}

fn draw_glyph(ctx: &mut Ctx<'_>, glyph: Glyph, c: Point, r: f32, color: Color) {
    let s = r * 0.52; // 图标半径
    match glyph {
        Glyph::Frame => {
            // 四角取景框
            let corner = s * 0.45;
            for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let p = Point::new(c.x + s * 0.72 * sx, c.y + s * 0.72 * sy);
                let px = Point::new(p.x - corner * sx, p.y);
                let py = Point::new(p.x, p.y - corner * sy);
                ctx.line(p, px, s * 0.16, color);
                ctx.line(p, py, s * 0.16, color);
            }
        }
        Glyph::Lines => {
            for (i, w) in [1.0f32, 0.66, 0.86].iter().enumerate() {
                let y = c.y + s * (-0.42 + 0.42 * i as f32);
                let half = s * 0.72 * w;
                ctx.line(
                    Point::new(c.x - half, y),
                    Point::new(c.x + half, y),
                    s * 0.16,
                    color,
                );
            }
        }
        Glyph::Grid => {
            let cell = s * 0.46;
            for i in -1..=1 {
                for j in -1..=1 {
                    let filled = (i + j) % 2 == 0;
                    let p = Point::new(c.x + cell * i as f32, c.y + cell * j as f32);
                    if filled {
                        ctx.fill_round_rect(p, s * 0.34, s * 0.34, s * 0.06, color);
                    } else {
                        ctx.stroke_circle(p, s * 0.1, s * 0.1, color);
                    }
                }
            }
        }
        Glyph::Clip => {
            ctx.fill_round_rect(c, s * 1.18, s * 1.5, s * 0.24, color);
            ctx.fill_round_rect(
                Point::new(c.x, c.y - s * 0.62),
                s * 0.52,
                s * 0.3,
                s * 0.1,
                color,
            );
            // 挖一个洞的效果：用主题色再画一次内部矩形不可行（颜色未知），
            // 因此改为在夹子上画两条横线表示"内容"。
            ctx.line(
                Point::new(c.x - s * 0.34, c.y + s * 0.06),
                Point::new(c.x + s * 0.34, c.y + s * 0.06),
                s * 0.12,
                col(Rgba::rgb(0, 0, 0), 0.45),
            );
        }
        Glyph::Drop => {
            // 水滴：圆 + 三角
            ctx.fill_circle(Point::new(c.x, c.y + s * 0.24), s * 0.56, color);
            ctx.fill_triangle(
                Point::new(c.x - s * 0.36, c.y + s * 0.18),
                Point::new(c.x + s * 0.36, c.y + s * 0.18),
                Point::new(c.x, c.y - s * 0.86),
                color,
            );
        }
        Glyph::Bolt => {
            ctx.fill_triangle(
                Point::new(c.x + s * 0.30, c.y - s * 0.80),
                Point::new(c.x - s * 0.42, c.y + s * 0.12),
                Point::new(c.x + s * 0.02, c.y + s * 0.12),
                color,
            );
            ctx.fill_triangle(
                Point::new(c.x - s * 0.30, c.y + s * 0.80),
                Point::new(c.x + s * 0.42, c.y - s * 0.12),
                Point::new(c.x - s * 0.02, c.y - s * 0.12),
                color,
            );
        }
        Glyph::Clock => {
            ctx.stroke_circle(c, s * 0.82, s * 0.16, color);
            ctx.line(c, Point::new(c.x, c.y - s * 0.50), s * 0.14, color);
            ctx.line(
                c,
                Point::new(c.x + s * 0.40, c.y + s * 0.18),
                s * 0.14,
                color,
            );
        }
        Glyph::Dots => {
            for i in -1..=1 {
                ctx.fill_circle(Point::new(c.x + s * 0.52 * i as f32, c.y), s * 0.17, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::Skin;
    use zouwu_core::theme::Theme;
    use zouwu_platform::assets::SkinPackage;

    fn solid_png(w: u32, h: u32) -> Vec<u8> {
        let mut pm = Pixmap::new(w, h).expect("pixmap");
        pm.fill(tiny_skia::Color::from_rgba8(220, 120, 80, 255));
        pm.encode_png().expect("png")
    }

    fn bitmap_skin() -> Skin {
        let pkg = SkinPackage {
            manifest: zouwu_core::skin::SkinManifest::parse(
                "id = \"t\"\n[geometry]\nradius = 56.0\n[[layers]]\nimage = \"body.png\"",
            )
            .expect("manifest"),
            dir: std::path::PathBuf::from("/nowhere"),
            images: vec![("body.png".to_string(), solid_png(64, 64))],
        };
        Skin::decode(&pkg).expect("skin")
    }

    fn visible_bbox(pm: &Pixmap) -> (f32, f32, f32, f32) {
        let (w, h) = (pm.width() as f32, pm.height() as f32);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0.0f32, 0.0f32);
        for (i, px) in pm.data().chunks_exact(4).enumerate() {
            if px[3] <= 8 {
                continue;
            }
            let x = (i % w as usize) as f32;
            let y = (i / w as usize) as f32;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        (x0, y0, x1, y1)
    }

    /// 位图皮肤在非 100% DPI 下必须和代码绘制的小兽**落在同一个点**上。
    ///
    /// 回归的是这个 bug：贴图走 `draw_pixmap`（物理像素），而其他画笔吃逻辑像素，
    /// 忘了乘 `scale` 时，125%/150% 缩放下整只兽会缩到画布左上，
    /// 表现为「影子在环中心、本体偏左上」——只在用户的机器上复现，离屏预览全对。
    #[test]
    fn bitmap_skin_centers_on_canvas_at_any_dpi_scale() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let skin = bitmap_skin();
            let logical = 100.0f32;
            let size = (logical * scale) as u32;
            let mut pm = Pixmap::new(size, size).expect("pixmap");
            let scene = Scene {
                width: logical,
                height: logical,
                scale,
                buddy_center: Point::new(logical * 0.5, logical * 0.5),
                buddy_radius: 20.0,
                buddy_scale: 1.0,
                buddy_alpha: 1.0,
                blink: 0.0,
                breath: 0.5,
                theme: &Theme::fallback(),
                bubbles: &[],
                beams: &[],
                skin: Some(&skin),
            };
            render(&scene, &mut pm);
            let (x0, y0, x1, y1) = visible_bbox(&pm);
            let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
            let want = size as f32 * 0.5;
            assert!(
                (cx - want).abs() <= 2.0 && (cy - want).abs() <= 2.0,
                "scale {scale}: 本体中心 ({cx:.1}, {cy:.1}) 偏离画布中心 ({want:.1}); \
                 bbox = ({x0:.1}, {y0:.1})..({x1:.1}, {y1:.1})"
            );
            // 直径还要对得上：`radius` 是逻辑像素，乘 scale 后才是物理像素。
            let want_side = 2.0 * 20.0 * scale;
            let got_side = (x1 - x0 + 1.0).max(y1 - y0 + 1.0);
            assert!(
                (got_side - want_side).abs() <= 2.0,
                "scale {scale}: 本体边长 {got_side:.1} 应为 {want_side:.1}"
            );
        }
    }
}
