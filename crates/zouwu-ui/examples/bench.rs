//! 光栅化性能基准：把一帧场景反复画进 `Pixmap`，报每帧平均耗时。
//!
//! ```bash
//! cargo run -p zouwu-ui --example bench            # dev
//! cargo run --release -p zouwu-ui --example bench  # release（真实观感）
//! cargo run -p zouwu-ui --example bench -- 1000 1.25
//! ```
//!
//! 参数：`[帧数] [DPI 缩放]`。报告的是**纯 CPU 光栅**成本，
//! 不含 `UpdateLayeredWindow`（那一块由 app 的 prof 日志给出，约 0.3~0.4ms）。
//!
//! 为什么要单独测：渲染预算（见 docs/architecture.md）是"空闲时几乎不画、
//! 动起来时才画"，所以单帧成本 × 动画占比 = 常驻开销，两个数都要盯着。

use std::time::Instant;

use tiny_skia::{Color, Pixmap};
use zouwu_core::action::ActionItem;
use zouwu_core::geom::{Point, Rect};
use zouwu_core::ring::{solve_rotation, RingConfig};
use zouwu_core::theme::Theme;
use zouwu_ui::paint::{render, BubblePaint, Scene};

fn bubble_paints(
    items: &[ActionItem],
    ring: &RingConfig,
    center: Point,
    view: Rect,
    openness: f32,
) -> Vec<BubblePaint<'static>> {
    let rotation = solve_rotation(ring, center, view, items.len());
    ring.layout(items.len(), center, rotation)
        .into_iter()
        .zip(items)
        .map(|(bubble, item)| BubblePaint {
            center: bubble.center,
            radius: bubble.radius,
            scale: openness,
            alpha: openness.clamp(0.0, 1.0),
            glyph: item.glyph,
            icon: None,
            selected: false,
        })
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let frames: u32 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
        .max(1);
    let scale: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);

    let logical = 340.0f32;
    let px = (logical * scale).round() as u32;
    let mut pixmap = Pixmap::new(px, px).expect("pixmap");
    pixmap.fill(Color::TRANSPARENT);

    let items = zouwu_core::action::resolve(
        &zouwu_core::config::RingCfg::default().items,
        &zouwu_core::i18n::I18n::new("zh-CN"),
    );
    let ring = RingConfig::default();
    let center = Point::new(logical * 0.5, logical * 0.5);
    let view = Rect::new(0.0, 0.0, logical, logical);
    let theme = Theme::by_name("zouwu-warm");

    println!("canvas {px}x{px} (logical {logical} @ {scale}x), {frames} frames each\n");
    println!(
        "{:<12} {:>10} {:>10} {:>12}",
        "state", "us/frame", "fps cap", "Mpx/s"
    );

    for (name, openness, blink, breath) in [
        ("collapsed", 0.0f32, 0.0f32, 0.0f32),
        ("expanded", 1.0, 0.0, 0.0),
        ("blink", 0.0, 1.0, 0.0),
        ("breath", 0.0, 0.0, 0.025),
    ] {
        let bubbles = bubble_paints(&items, &ring, center, view, openness);
        let scene = Scene {
            width: logical,
            height: logical,
            scale,
            buddy_center: center,
            buddy_radius: 56.0,
            buddy_scale: 1.0,
            buddy_alpha: 1.0,
            blink,
            breath,
            theme: &theme,
            bubbles: &bubbles,
            beams: &[],
            skin: None,
        };

        // 预热：让 tiny-skia 的路径缓存与 CPU 频率先稳下来
        for _ in 0..20 {
            render(&scene, &mut pixmap);
        }
        let t0 = Instant::now();
        for _ in 0..frames {
            render(&scene, &mut pixmap);
        }
        let us = t0.elapsed().as_micros() as f64 / frames as f64;
        println!(
            "{name:<12} {us:>10.1} {:>10.0} {:>12.1}",
            1_000_000.0 / us,
            (px as f64 * px as f64) / us
        );
    }

    micro(&mut pixmap, frames, scale);
}

/// 成本模型：把画面拆成"同等形状的合成用例"，看时间到底花在哪儿。
///
/// 目的不是精确复刻素材，而是给出一条可比较的标尺：
/// 如果某个合成用例和整体帧成本数量级一致，优化就有明确靶子。
fn micro(pixmap: &mut Pixmap, frames: u32, scale: f32) {
    let s = scale;
    let mut bench = |label: &str, f: &mut dyn FnMut(&mut Pixmap)| {
        for _ in 0..20 {
            f(pixmap);
        }
        let t0 = Instant::now();
        for _ in 0..frames {
            f(pixmap);
        }
        let us = t0.elapsed().as_micros() as f64 / frames as f64;
        println!("{label:<12} {us:>10.1} us/frame");
    };

    let cx = 170.0f32;
    let cy = 176.0f32;
    bench("clear", &mut |pm| pm.fill(Color::TRANSPARENT));
    let mut shadow = |pm: &mut Pixmap| {
        for i in (1..=12).rev() {
            let k = i as f32 / 12.0;
            let r = 56.0 * (0.42 + 0.62 * k);
            let Some(path) = tiny_skia::PathBuilder::from_circle(0.0, 0.0, r * s) else {
                continue;
            };
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(Color::from_rgba8(0, 0, 0, 14));
            paint.anti_alias = true;
            let ts = tiny_skia::Transform::from_scale(1.0, 0.14)
                .post_translate(cx * s, (cy + 56.0 * 0.94) * s);
            pm.fill_path(&path, &paint, tiny_skia::FillRule::Winding, ts, None);
        }
    };
    bench("shadow x12", &mut shadow);
    let mut mane = |pm: &mut Pixmap| {
        for i in 0..10 {
            let ang = i as f32 / 10.0 * std::f32::consts::TAU + 0.31;
            paint_circle(
                pm,
                s,
                cx + ang.cos() * 32.5,
                cy - 19.0 + ang.sin() * 32.5,
                15.1,
                242,
            );
        }
    };
    bench("mane x10", &mut mane);
    bench("body r51", &mut |pm| {
        paint_circle(pm, s, cx, cy + 12.3, 51.5, 242)
    });
    bench("head r38", &mut |pm| {
        paint_circle(pm, s, cx, cy - 19.0, 38.1, 242)
    });
}

fn paint_circle(pm: &mut Pixmap, s: f32, cx: f32, cy: f32, r: f32, a: u8) {
    let Some(path) = tiny_skia::PathBuilder::from_circle(cx * s, cy * s, r * s) else {
        return;
    };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color(Color::from_rgba8(232, 122, 74, a));
    paint.anti_alias = true;
    pm.fill_path(
        &path,
        &paint,
        tiny_skia::FillRule::Winding,
        tiny_skia::Transform::identity(),
        None,
    );
}
