//! 生成官方第二套皮肤「水墨驺吾」的图层 PNG。
//!
//! ```bash
//! cargo run --release -p zouwu-ui --example make_skins
//! ```
//!
//! 输出到**仓库内**的 `assets/skins/zouwu-ink/`（会被打包进程序，
//! 首次启动时落到 `%APPDATA%/zouwu/skins/`）。生成物要提交进 git——
//! 这样别人 clone 下来就有两套皮肤，而不是先跑一遍生成器。
//!
//! 为什么要个生成器而不直接给 PNG：这套皮肤是**代码画的**，改一笔就得重出图。
//! 生成器留在仓库里，以后想调墨色/笔触，改数字重跑即可，不用求人开 Photoshop。

use std::path::{Path, PathBuf};

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("skins")
        .join("zouwu-ink");
    std::fs::create_dir_all(&dir).expect("create skin dir");
    dir
}

/// 给颜色换个不透明度。
///
/// tiny-skia 的 `Color` 没有 `with_alpha`（只有在 `ColorU8` 那边才有直通构造），
/// 所以这里自己按分量重组一次——`from_rgba8` 不做校验，不会返回 Option。
fn tint(base: Color, alpha: f32) -> Color {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::from_rgba8(ch(base.red()), ch(base.green()), ch(base.blue()), ch(alpha))
}

fn ink() -> Color {
    Color::from_rgba8(38, 34, 32, 255)
}

fn paper() -> Color {
    Color::from_rgba8(248, 244, 232, 255)
}

fn seal() -> Color {
    Color::from_rgba8(178, 42, 38, 255)
}

/// 一团"墨"：几个半透明圆叠起来，边缘不齐，比单个正圆更像笔触。
fn blot(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, color: Color, alpha: f32) {
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    // 三笔叠一起，略微错位 → 看起来是"洇开"的，而不是几何圆。
    for (dx, dy, k, a) in [
        (0.0, 0.0, 1.00, 1.0),
        (0.10, -0.06, 0.86, 0.55),
        (-0.08, 0.07, 0.92, 0.45),
    ] {
        paint.set_color(tint(color, alpha * a));
        if let Some(path) = PathBuilder::from_circle(cx + r * dx, cy + r * dy, r * k) {
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// 一笔：带粗细渐变的折线（用锥形三角近似，避免引入渐变对象）。
fn brush(pm: &mut Pixmap, pts: &[(f32, f32)], width: f32, color: Color) {
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(color);
    for (i, pair) in pts.windows(2).enumerate() {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        // 头重脚轻：起笔粗、收笔细。
        let k = 1.0 - i as f32 / pts.len().max(2) as f32 * 0.6;
        let (dx, dy) = (y1 - y0, x0 - x1);
        let len = (dx * dx + dy * dy).sqrt().max(0.001);
        let (nx, ny) = (dx / len * width * k * 0.5, dy / len * width * k * 0.5);
        let mut pb = PathBuilder::new();
        pb.move_to(x0 + nx, y0 + ny);
        pb.line_to(x1 + nx * 0.6, y1 + ny * 0.6);
        pb.line_to(x1 - nx * 0.6, y1 - ny * 0.6);
        pb.line_to(x0 - nx, y0 - ny);
        pb.close();
        if let Some(path) = pb.finish() {
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

fn body(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let s = size as f32;
    let (cx, cy) = (s * 0.5, s * 0.55);

    // 尾巴：先画在身体下面，两笔交叠
    brush(
        &mut pm,
        &[
            (s * 0.34, s * 0.70),
            (s * 0.19, s * 0.68),
            (s * 0.15, s * 0.52),
            (s * 0.19, s * 0.38),
        ],
        s * 0.075,
        ink(),
    );

    // 耳朵：两团墨
    blot(&mut pm, s * 0.35, s * 0.30, s * 0.085, ink(), 0.95);
    blot(&mut pm, s * 0.65, s * 0.30, s * 0.085, ink(), 0.95);

    // 身体：主墨团（半径 0.30 对齐 geometry.radius = 56 / 设计尺寸 112 的一半）
    blot(&mut pm, cx, cy, s * 0.30, ink(), 1.0);

    // 鬃毛：四根短笔触贴着身体左上缘往外扫（长了就像胳膊，第一版就是）
    for (i, (dx, dy, tilt)) in [
        (-0.06f32, -0.22f32, -0.10f32),
        (-0.10, -0.16, -0.14),
        (-0.13, -0.09, -0.16),
        (-0.14, -0.02, -0.12),
    ]
    .iter()
    .enumerate()
    {
        let k = 1.0 - i as f32 * 0.14;
        brush(
            &mut pm,
            &[
                (cx + s * (dx + 0.06), cy + s * (dy + 0.06)),
                (cx + s * (dx * 1.10), cy + s * (dy * 1.10 + tilt * 0.3)),
                (cx + s * (dx * 1.28), cy + s * (dy * 1.28 + tilt * 0.6)),
            ],
            s * 0.030 * k,
            ink(),
        );
    }

    // 胡须：两侧各两笔细线，留白感
    let mut thin = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    thin.set_color(tint(paper(), 0.75));
    for (dy, len) in [(0.03f32, 0.13f32), (0.09, 0.11)] {
        for side in [-1.0f32, 1.0] {
            let mut pb = PathBuilder::new();
            pb.move_to(cx + side * s * 0.12, s * (0.60 + dy));
            pb.quad_to(
                cx + side * s * (0.12 + len * 0.6),
                s * (0.62 + dy),
                cx + side * s * (0.12 + len),
                s * (0.58 + dy),
            );
            if let Some(path) = pb.finish() {
                pm.stroke_path(
                    &path,
                    &thin,
                    &Stroke {
                        width: s * 0.012,
                        ..Default::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    // 眼睛：留白点（水墨风格里"眼"是纸色）
    for side in [-1.0f32, 1.0] {
        blot(
            &mut pm,
            cx + side * s * 0.085,
            cy - s * 0.02,
            s * 0.028,
            paper(),
            0.95,
        );
    }
    pm
}

fn blink(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let s = size as f32;
    let (cx, cy) = (s * 0.5, s * 0.55);
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(tint(paper(), 0.95));
    for side in [-1.0f32, 1.0] {
        let mut pb = PathBuilder::new();
        let x = cx + side * s * 0.085;
        pb.move_to(x - s * 0.035, cy - s * 0.02);
        pb.quad_to(x, cy + s * 0.005, x + s * 0.035, cy - s * 0.02);
        if let Some(path) = pb.finish() {
            pm.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width: s * 0.014,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
    pm
}

/// 印章：右下角一枚小方印。用来验证 `overlay` 角色（永远在最上层）。
fn seal_layer(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let s = size as f32;
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(seal());

    // 位置在身体右下外侧（身体边缘 ≈ 0.80），压在留白上，不糊在脸上
    let (w, x, y) = (s * 0.105, s * 0.735, s * 0.755);
    let path = PathBuilder::from_rect(tiny_skia::Rect::from_xywh(x, y, w, w).unwrap());
    pm.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    // 印面的留白：两条横线，远看像字
    let mut cut = Paint {
        anti_alias: true,
        ..Default::default()
    };
    cut.set_color(tint(paper(), 0.9));
    for (i, dy) in [0.035f32, 0.075].iter().enumerate() {
        let mut pb = PathBuilder::new();
        pb.move_to(x + w * 0.22, y + w * dy);
        pb.line_to(x + w * (0.78 - i as f32 * 0.12), y + w * dy);
        if let Some(path) = pb.finish() {
            pm.stroke_path(
                &path,
                &cut,
                &Stroke {
                    width: s * 0.012,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
    pm
}

fn write_png(dir: &Path, name: &str, pm: Pixmap, visible_min: usize) {
    let path = dir.join(name);
    pm.save_png(&path).expect("save png");
    let visible = pm.data().chunks_exact(4).filter(|px| px[3] > 8).count();
    assert!(
        visible >= visible_min,
        "{name}: only {visible} visible px (expected >= {visible_min}) — 图塌了"
    );
    println!("{name}: {visible} visible px -> {}", path.display());
}

fn main() {
    let dir = out_dir();
    write_png(&dir, "body.png", body(256), 15_000);
    write_png(&dir, "blink.png", blink(256), 100);
    write_png(&dir, "seal.png", seal_layer(256), 600);

    let manifest = r#"# 水墨驺吾 —— 官方第二套皮肤
id = "zouwu-ink"
name = "水墨驺吾"
author = "zouwu"
version = "1.0.0"
theme = "zouwu-dark"

[geometry]
radius = 56.0
breath_amount = 0.03

[[layers]]
image = "body.png"

[[layers]]
image = "blink.png"
role = "blink"

[[layers]]
image = "seal.png"
role = "overlay"
"#;
    std::fs::write(dir.join("skin.toml"), manifest).expect("write skin.toml");
    println!("wrote skin.toml");
}
