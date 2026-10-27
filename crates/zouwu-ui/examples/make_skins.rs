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
    // 比"正红"暗一点、亮一点：压在纯墨上要看得见，又不能抢走整只兽的注意力。
    Color::from_rgba8(204, 52, 44, 255)
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

/// 一串互相重叠的墨点，半径从 `r0` 收到 `r1`。
///
/// 为什么不用 [`brush`] 画尾巴：三角笔触在急转弯的两段之间必然留缝，
/// 视觉上就是一串尖刺（试过三版，每版都像蜈蚣腿）。圆点连珠没有接缝，
/// 而且和水墨的晕染感同源。
fn blob_chain(pm: &mut Pixmap, pts: &[(f32, f32)], r0: f32, r1: f32, color: Color) {
    let n = pts.len().max(2) as f32;
    for (i, &(x, y)) in pts.iter().enumerate() {
        let t = i as f32 / (n - 1.0);
        // 每个点之间再补一个中间点，转弯处才不会出现"珠子"感
        if i > 0 {
            let (px, py) = pts[i - 1];
            let tm = (i as f32 - 0.5) / (n - 1.0);
            blot(
                pm,
                (x + px) * 0.5,
                (y + py) * 0.5,
                r0 + (r1 - r0) * tm,
                color,
                1.0,
            );
        }
        blot(pm, x, y, r0 + (r1 - r0) * t, color, 1.0);
    }
}

/// 所有部件都用**同一个坐标系**：身体圆心 `(cx, cy)` + 身体半径 `R`，
/// 比例以 `R` 为单位（`0.5R` = 半个身体半径）。
///
/// 第一版是混着写的——身体用 `cy = 0.55 * s`，胡须却写死 `0.60 * s`，
/// 于是"把身体挪正"只挪了一半，整只兽看上去永远往右下坠。
/// 现在身体圆心固定 `(0.5, 0.5)`，其他部件只准用 `R` 表达偏移：
/// 想调构图只改中心那一行，不会再有部件掉队。
fn frame(size: u32) -> (f32, f32, f32) {
    let s = size as f32;
    (s * 0.5, s * 0.5, s * 0.30)
}

fn body(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let (cx, cy, r) = frame(size);

    // ── 尾巴：从身体左下沿甩出去、末端向内卷成一个钩 ─────────────────
    // 先画，压在身体下层。半径 0.13r → 0.07r：比第一版细，否则像个"袋子"。
    blob_chain(
        &mut pm,
        &[
            (cx - 0.66 * r, cy + 0.46 * r),
            (cx - 0.92 * r, cy + 0.40 * r),
            (cx - 1.06 * r, cy + 0.22 * r),
            (cx - 1.04 * r, cy + 0.02 * r),
            (cx - 0.92 * r, cy - 0.08 * r),
        ],
        0.13 * r,
        0.07 * r,
        ink(),
    );

    // ── 耳朵：两只带尖的三角，左右对称 ───────────────────────────────
    // 用路径画而不是墨点：墨点画出来的"耳朵"永远是圆的，看不出是猫科。
    for side in [-1.0f32, 1.0] {
        let mut pb = PathBuilder::new();
        pb.move_to(cx + side * 0.20 * r, cy - 0.60 * r);
        pb.quad_to(
            cx + side * 0.48 * r,
            cy - 0.86 * r,
            cx + side * 0.66 * r,
            cy - 1.00 * r,
        );
        pb.quad_to(
            cx + side * 0.74 * r,
            cy - 0.66 * r,
            cx + side * 0.66 * r,
            cy - 0.40 * r,
        );
        pb.close();
        if let Some(path) = pb.finish() {
            let mut paint = Paint {
                anti_alias: true,
                ..Paint::default()
            };
            paint.set_color(ink());
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    // ── 身体：一团稍扁的墨，下缘再补一笔当"坐下去"的重量 ───────────────
    blot(&mut pm, cx, cy - 0.02 * r, r, ink(), 1.0);
    blot(&mut pm, cx, cy + 0.24 * r, 0.80 * r, ink(), 1.0);

    // ── 鬃毛：左右各三簇，**对称** ─────────────────────────────────
    // 第一版只长在左上（想表现"被风吹"，结果像多了三条腿），
    // 而且整只兽因此显得向右坠——不对称的装饰会直接变成"位置偏"的观感。
    // 角度按屏幕坐标算（y 向下）：右侧 -30°..30°，左侧 150°..210°。
    for side in [-1.0f32, 1.0] {
        for (i, deg) in [26.0f32, 0.0, -26.0].iter().enumerate() {
            let rad = deg.to_radians();
            let (sin, cos) = rad.sin_cos();
            let k = 1.0 - i as f32 * 0.06;
            blot(
                &mut pm,
                cx + side * cos * 1.04 * r,
                cy + sin * 1.04 * r,
                0.13 * r * k,
                ink(),
                0.95,
            );
        }
    }

    // ── 胡须：两侧各三笔，长度对称 ────────────────────────────────
    let mut thin = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    thin.set_color(tint(paper(), 0.8));
    for (dy, len) in [(0.10f32, 0.40f32), (0.24, 0.46), (0.38, 0.38)] {
        for side in [-1.0f32, 1.0] {
            let mut pb = PathBuilder::new();
            pb.move_to(cx + side * 0.34 * r, cy + (dy + 0.05) * r);
            pb.quad_to(
                cx + side * (0.34 + len * 0.55) * r,
                cy + (dy + 0.09) * r,
                cx + side * (0.34 + len) * r,
                cy + (dy - 0.02) * r,
            );
            if let Some(path) = pb.finish() {
                pm.stroke_path(
                    &path,
                    &thin,
                    &Stroke {
                        width: 0.035 * r,
                        ..Default::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    // ── 眼睛：纸色眼白 + 墨点瞳孔 ─────────────────────────────────
    // 只有眼白的时候像个"空壳"；加上瞳孔才有表情，也才读得出是"看过来"。
    for side in [-1.0f32, 1.0] {
        let ex = cx + side * 0.28 * r;
        let ey = cy - 0.10 * r;
        blot(&mut pm, ex, ey, 0.135 * r, paper(), 0.95);
        blot(
            &mut pm,
            ex + side * 0.012 * r,
            ey + 0.028 * r,
            0.058 * r,
            ink(),
            1.0,
        );
        // 高光：一个更小的纸色点，压在瞳孔左上
        blot(
            &mut pm,
            ex - 0.018 * r,
            ey - 0.010 * r,
            0.020 * r,
            paper(),
            0.9,
        );
    }

    // ── 鼻子与嘴：一个纸色小三角 + 两笔向下的弧 ─────────────────────
    let mut pb = PathBuilder::new();
    pb.move_to(cx - 0.055 * r, cy + 0.10 * r);
    pb.line_to(cx + 0.055 * r, cy + 0.10 * r);
    pb.line_to(cx, cy + 0.175 * r);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        paint.set_color(tint(paper(), 0.85));
        pm.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    for side in [-1.0f32, 1.0] {
        let mut pb = PathBuilder::new();
        pb.move_to(cx, cy + 0.17 * r);
        pb.quad_to(
            cx + side * 0.05 * r,
            cy + 0.26 * r,
            cx + side * 0.16 * r,
            cy + 0.24 * r,
        );
        if let Some(path) = pb.finish() {
            pm.stroke_path(
                &path,
                &thin,
                &Stroke {
                    width: 0.03 * r,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
    pm
}

fn blink(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let (cx, cy, r) = frame(size);
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(tint(paper(), 0.95));
    for side in [-1.0f32, 1.0] {
        let mut pb = PathBuilder::new();
        let x = cx + side * 0.28 * r;
        pb.move_to(x - 0.14 * r, cy - 0.10 * r);
        pb.quad_to(x, cy + 0.02 * r, x + 0.14 * r, cy - 0.10 * r);
        if let Some(path) = pb.finish() {
            pm.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width: 0.045 * r,
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
    let (cx, cy, r) = frame(size);
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(seal());

    // 贴住身体右下沿（球面外一点点），压在留白上，不糊在脸上
    let w = 0.26 * r;
    let (x, y) = (cx + 0.50 * r, cy + 0.54 * r);
    // 歪一点（-6°）：手盖的印章不会正。绕自己的中心旋转，位置不受影响。
    let turn = Transform::from_rotate_at(-6.0, x + w * 0.5, y + w * 0.5);
    if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, w) {
        let path = PathBuilder::from_rect(rect);
        pm.fill_path(&path, &paint, FillRule::Winding, turn, None);
    }
    // 印面的留白：两条横线，远看像字
    let mut cut = Paint {
        anti_alias: true,
        ..Default::default()
    };
    cut.set_color(tint(paper(), 0.9));
    for (i, dy) in [0.30f32, 0.58].iter().enumerate() {
        let mut pb = PathBuilder::new();
        pb.move_to(x + w * 0.22, y + w * dy);
        pb.line_to(x + w * (0.78 - i as f32 * 0.12), y + w * dy);
        if let Some(path) = pb.finish() {
            pm.stroke_path(
                &path,
                &cut,
                &Stroke {
                    width: w * 0.11,
                    ..Default::default()
                },
                turn,
                None,
            );
        }
    }
    pm
}

/// alpha 加权重心（0..1 归一坐标）。
///
/// 用来拦住"构图跑偏"这类只能靠眼睛发现的错误：身体层的重心必须落在画布中心
/// 附近，否则挂到桌面上就会显得往一边坠。
fn centroid(pm: &Pixmap) -> (f32, f32) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let (mut sx, mut sy, mut total) = (0.0f64, 0.0f64, 0.0f64);
    for (i, px) in pm.data().chunks_exact(4).enumerate() {
        let a = px[3] as f64;
        if a == 0.0 {
            continue;
        }
        let x = (i as f32 % w) as f64;
        let y = (i as f32 / w) as f64;
        sx += x * a;
        sy += y * a;
        total += a;
    }
    if total == 0.0 {
        return (0.5, 0.5);
    }
    (
        (sx / total / w as f64) as f32,
        (sy / total / h as f64) as f32,
    )
}

fn write_png(dir: &Path, name: &str, pm: Pixmap, visible_min: usize) {
    let path = dir.join(name);
    pm.save_png(&path).expect("save png");
    let visible = pm.data().chunks_exact(4).filter(|px| px[3] > 8).count();
    assert!(
        visible >= visible_min,
        "{name}: only {visible} visible px (expected >= {visible_min}) — 图塌了"
    );
    let (gx, gy) = centroid(&pm);
    println!(
        "{name}: {visible} visible px, centroid ({gx:.3}, {gy:.3}) -> {}",
        path.display()
    );
}

fn main() {
    let dir = out_dir();
    let body_pm = body(256);
    // 主体重心允许 3% 的偏移：耳朵在上、尾巴在左、印章在右下，天然不对称。
    // 但要拦住的是"整体往一边坠"那种 8% 以上的错位。
    let (gx, gy) = centroid(&body_pm);
    assert!(
        (gx - 0.5).abs() < 0.03 && (gy - 0.5).abs() < 0.03,
        "body is off-center: centroid ({gx:.3}, {gy:.3}) — 检查 make_skins.rs 的 frame()"
    );
    write_png(&dir, "body.png", body_pm, 15_000);
    write_png(&dir, "blink.png", blink(256), 100);
    // 印章做小了（0.26r），可见像素跟着少——阈值按"至少能看出是个方块"给。
    write_png(&dir, "seal.png", seal_layer(256), 300);

    let manifest = r#"# 水墨驺吾 —— 官方第二套皮肤
id = "zouwu-ink"
name = "水墨驺吾"
author = "zouwu"
version = "1.0.0"
theme = "zouwu-dark"

[geometry]
radius = 56.0
breath_amount = 0.03

# 主体在 256 画布里只占 0.6（留了水墨的空白），
# 所以用图层 `scale` 把它放大 1/0.6 ≈ 1.67 倍补齐，再留一点余量取 1.80：
# `radius` 始终表示"桌面上的主体半径"，`scale` 只负责校正画面里的留白。
[[layers]]
image = "body.png"
scale = 1.80

[[layers]]
image = "blink.png"
role = "blink"
scale = 1.80

[[layers]]
image = "seal.png"
role = "overlay"
scale = 1.80
"#;
    std::fs::write(dir.join("skin.toml"), manifest).expect("write skin.toml");
    println!("wrote skin.toml");
}
