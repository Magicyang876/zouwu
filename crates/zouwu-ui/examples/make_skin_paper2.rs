//! 生成官方第四套皮肤「折纸驺吾 2」的图层 PNG。
//!
//! ```bash
//! cargo run --release -p zouwu-ui --example make_skin_paper2
//! ```
//!
//! 输出到**仓库内**的 `assets/skins/zouwu-paper-2/`（打包进程序，首次启动落到
//! `%APPDATA%/zouwu/skins/`），生成物要提交进 git。
//!
//! ## 与「折纸驺吾」的区别
//!
//! 那套是**一团纸**：一个八边形，靠一道折痕分亮暗两面，抽象到只剩耳朵和口鼻。
//! 这一套是**一只坐着的兽**：有头、有前爪、有一条卷起来的尾巴，靠十几块互不相邻
//! 却又首尾相接的三角面拼出体积。同样的规则没变——没有渐变、没有模糊、没有曲线，
//! 只有纯色面 + 折痕线；变化的是细节密度：32 px 下那团纸只剩轮廓，这一套还看得出
//! 耳朵、脸和爪子。
//!
//! 配色从传统颜料名换成更"年画"的一组：朱橘为主，杏黄为迎光面，赭为背光面，
//! 奶白做口鼻与胸腹，松烟只用在眼睛、鼻子和折痕上。

use std::path::{Path, PathBuf};

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
use zouwu_core::geom::{Point, Rect};
use zouwu_core::skin::SkinManifest;
use zouwu_core::theme::Theme;
use zouwu_platform::assets::SkinPackage;
use zouwu_ui::paint::{render, BubblePaint, Scene};
use zouwu_ui::skin::Skin;

const SIDE: u32 = 256;
/// 主体（不含耳朵与尾巴）在画布里的半径占比。`geometry.radius` 的口径就是它，
/// 图层的 scale 由它反推，不能手调。
const BLOB_RATIO: f32 = 0.30;

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("skins")
        .join("zouwu-paper-2");
    std::fs::create_dir_all(&dir).expect("create skin dir");
    dir
}

fn preview_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("zouwu-offscreen");
    std::fs::create_dir_all(&dir).expect("create preview dir");
    dir
}

/// 朱橘：主体纸面。
fn vermilion() -> Color {
    Color::from_rgba8(226, 120, 52, 255)
}
/// 杏黄：迎光的那一面。
fn apricot() -> Color {
    Color::from_rgba8(246, 196, 132, 255)
}
/// 赭：背光的那一面。
fn ochre() -> Color {
    Color::from_rgba8(178, 88, 33, 255)
}
/// 奶白：口鼻、胸腹、爪。
fn cream() -> Color {
    Color::from_rgba8(248, 238, 219, 255)
}
/// 松烟：眼、鼻、折痕。
fn soot() -> Color {
    Color::from_rgba8(53, 37, 28, 255)
}

fn tint(base: Color, alpha: f32) -> Color {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::from_rgba8(ch(base.red()), ch(base.green()), ch(base.blue()), ch(alpha))
}

fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

fn poly(points: &[(f32, f32)]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let (first, rest) = points.split_first()?;
    pb.move_to(first.0, first.1);
    for (x, y) in rest {
        pb.line_to(*x, *y);
    }
    pb.close();
    pb.finish()
}

fn fill(pm: &mut Pixmap, points: &[(f32, f32)], color: Color) {
    if let Some(path) = poly(points) {
        pm.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn stroke(pm: &mut Pixmap, points: &[(f32, f32)], color: Color, width: f32) {
    if let Some(path) = poly(points) {
        let s = Stroke {
            width,
            ..Default::default()
        };
        pm.stroke_path(&path, &paint(color), &s, Transform::identity(), None);
    }
}

/// 眼：折纸没有曲线，圆眼也切成八边形。
fn dot(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, color: Color) {
    let mut pts = Vec::with_capacity(8);
    for i in 0..8 {
        let a = std::f32::consts::TAU * i as f32 / 8.0 + 0.39;
        pts.push((cx + r * a.cos(), cy + r * a.sin()));
    }
    fill(pm, &pts, color);
}

fn canvas() -> Pixmap {
    Pixmap::new(SIDE, SIDE).expect("canvas")
}

/// 一条折起来的纸带：相邻两点之间是一段四边形，再沿对角线切成两个三角形。
///
/// 尾巴就是四条这样的纸带接起来。**不画曲线、不做圆头**——纸折出来的东西没有圆角，
/// 圆头一看就"是画的"。
fn ribbon(pm: &mut Pixmap, nodes: &[(f32, f32)], widths: &[f32], tones: &[(Color, Color)]) {
    for i in 0..nodes.len() - 1 {
        let (x0, y0) = nodes[i];
        let (x1, y1) = nodes[i + 1];
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt().max(1e-3);
        let (nx, ny) = (-dy / len, dx / len);
        let (w0, w1) = (widths[i], widths[i + 1]);
        let a = (x0 + nx * w0, y0 + ny * w0);
        let b = (x1 + nx * w1, y1 + ny * w1);
        let c = (x1 - nx * w1, y1 - ny * w1);
        let d = (x0 - nx * w0, y0 - ny * w0);
        let (t1, t2) = tones[i];
        fill(pm, &[a, b, c], t1);
        fill(pm, &[a, c, d], t2);
        // 折痕：两个三角形共用对角线 a–c，给一条极淡的线，面才有"折过"的意思。
        stroke(pm, &[a, c], tint(soot(), 0.14), 1.2);
    }
}

/// 头的外轮廓：左右不对称（手工剪的纸不会左右同形），上窄下圆。
fn head_outline() -> [(f32, f32); 12] {
    [
        (90.0, 72.0), // 左耳根
        (108.0, 62.0),
        (128.0, 59.0), // 顶中
        (150.0, 62.0),
        (166.0, 74.0), // 右耳根
        (184.0, 102.0),
        (176.0, 124.0),
        (166.0, 140.0),
        (128.0, 153.0), // 下巴
        (90.0, 140.0),
        (80.0, 124.0),
        (74.0, 102.0),
    ]
}

fn body() -> Pixmap {
    let mut pm = canvas();

    // ---- 尾巴（在最下层：根部压在身体下面，接缝才不会被看见）----
    let nodes = [
        (168.0, 200.0),
        (194.0, 195.0),
        (208.0, 172.0),
        (204.0, 146.0),
        (188.0, 128.0),
    ];
    let widths = [16.0, 16.0, 13.0, 9.0, 5.0];
    let tones = [
        (vermilion(), ochre()),
        (ochre(), vermilion()),
        (vermilion(), apricot()),
        (apricot(), vermilion()),
    ];
    ribbon(&mut pm, &nodes, &widths, &tones);

    // ---- 身体：肩宽下窄的六边形，从腹部的"折点"向轮廓扇出六个面 ----
    let b = [
        (102.0, 142.0),
        (154.0, 142.0),
        (173.0, 180.0),
        (168.0, 206.0),
        (88.0, 206.0),
        (83.0, 180.0),
    ];
    let belly_pole = (128.0, 176.0);
    let body_tones = [
        vermilion(),
        vermilion(),
        ochre(),
        vermilion(),
        apricot(),
        apricot(),
    ];
    for i in 0..b.len() {
        let p = b[i];
        let q = b[(i + 1) % b.len()];
        fill(&mut pm, &[belly_pole, p, q], body_tones[i]);
    }

    // 肩上的两道虎斑：越靠外越窄，收在轮廓里。
    fill(
        &mut pm,
        &[(102.0, 142.0), (118.0, 146.0), (103.0, 172.0)],
        ochre(),
    );
    fill(
        &mut pm,
        &[(154.0, 142.0), (140.0, 146.0), (153.0, 170.0)],
        ochre(),
    );

    // 胸腹：一块奶白的折面，中间一道竖折痕。别画太长，否则像条白围兜。
    fill(
        &mut pm,
        &[
            (110.0, 146.0),
            (146.0, 146.0),
            (134.0, 187.0),
            (122.0, 187.0),
        ],
        cream(),
    );
    fill(
        &mut pm,
        &[
            (110.0, 146.0),
            (128.0, 146.0),
            (128.0, 187.0),
            (122.0, 187.0),
        ],
        tint(soot(), 0.05),
    );
    stroke(
        &mut pm,
        &[(128.0, 148.0), (128.0, 186.0)],
        tint(soot(), 0.15),
        1.3,
    );

    // ---- 前爪：两块奶白的纸片，压在身体下缘 ----
    fill(
        &mut pm,
        &[(84.0, 192.0), (172.0, 192.0), (168.0, 206.0), (88.0, 206.0)],
        tint(soot(), 0.16),
    );
    for (x0, crease) in [(98.0f32, 112.0f32), (132.0, 146.0)] {
        let x1 = x0 + 26.0;
        fill(
            &mut pm,
            &[
                (x0, 195.0),
                (x1, 195.0),
                (x1 - 3.0, 216.0),
                (x0 + 3.0, 216.0),
            ],
            cream(),
        );
        fill(
            &mut pm,
            &[
                (crease, 195.0),
                (x1, 195.0),
                (x1 - 3.0, 216.0),
                (crease, 216.0),
            ],
            tint(soot(), 0.07),
        );
        stroke(
            &mut pm,
            &[(crease, 196.0), (crease, 215.0)],
            tint(soot(), 0.18),
            1.2,
        );
    }

    // ---- 耳朵：宽而短的三角，画在头下面，头的外轮廓会盖住耳根 ----
    for (base_a, base_b, tip, inner, outer) in [
        (
            (86.0f32, 82.0f32),
            (120.0f32, 64.0f32),
            (70.0f32, 40.0f32),
            apricot(),
            vermilion(),
        ),
        (
            (168.0, 82.0),
            (134.0, 64.0),
            (184.0, 40.0),
            vermilion(),
            ochre(),
        ),
    ] {
        fill(&mut pm, &[base_a, base_b, tip], inner);
        let mid = ((base_a.0 + base_b.0) * 0.5, (base_a.1 + base_b.1) * 0.5);
        fill(&mut pm, &[mid, base_b, tip], outer);
        stroke(&mut pm, &[mid, tip], tint(soot(), 0.18), 1.3);
    }

    // ---- 头：折痕从鼻梁根部向外辐射，这是纸面具的折法 ----
    let h = head_outline();
    let pole = (128.0, 124.0);
    let head_tones = [
        vermilion(),
        apricot(),
        apricot(),
        vermilion(),
        vermilion(),
        apricot(),
        vermilion(),
        vermilion(),
        apricot(),
        apricot(),
        apricot(),
        apricot(),
    ];
    for i in 0..h.len() {
        let p = h[i];
        let q = h[(i + 1) % h.len()];
        fill(&mut pm, &[pole, p, q], head_tones[i]);
    }

    // 耳根到眼下：两道虎斑，收在颞部，别压到眼睛上。
    fill(
        &mut pm,
        &[(90.0, 72.0), (104.0, 70.0), (97.0, 90.0)],
        ochre(),
    );
    fill(
        &mut pm,
        &[(166.0, 74.0), (152.0, 70.0), (159.0, 90.0)],
        ochre(),
    );

    // 额头正中的竖折痕：从额顶一路到鼻梁。
    stroke(
        &mut pm,
        &[(128.0, 59.0), (128.0, 122.0)],
        tint(soot(), 0.16),
        1.4,
    );

    // ---- 口鼻：一小块奶白的折面，鼻尖一个小三角（朝下）----
    // 只占脸的下三分之一。大了就变成"口罩"，整张脸会读成浣熊。
    fill(
        &mut pm,
        &[
            (110.0, 126.0),
            (146.0, 126.0),
            (136.0, 147.0),
            (120.0, 147.0),
        ],
        cream(),
    );
    fill(
        &mut pm,
        &[
            (128.0, 126.0),
            (146.0, 126.0),
            (136.0, 147.0),
            (128.0, 147.0),
        ],
        tint(soot(), 0.04),
    );
    stroke(
        &mut pm,
        &[(128.0, 127.0), (128.0, 147.0)],
        tint(soot(), 0.13),
        1.1,
    );
    fill(
        &mut pm,
        &[(122.0, 128.0), (134.0, 128.0), (128.0, 137.0)],
        soot(),
    );

    // ---- 眼睛：小、分得开、纯松烟 ----
    for x in [108.0f32, 148.0] {
        dot(&mut pm, x, 112.0, 6.0, soot());
    }

    pm
}

/// 闭眼层：两道向下弯的折线，位置与睁眼时一致。
fn blink() -> Pixmap {
    let mut pm = canvas();
    for x in [106.0f32, 150.0] {
        stroke(
            &mut pm,
            &[(x - 6.5, 110.0), (x, 116.0), (x + 6.5, 110.0)],
            soot(),
            3.4,
        );
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
    let (gx, gy) = centroid(&pm);
    println!(
        "{name}: {visible} visible px, centroid ({gx:.3}, {gy:.3}) -> {}",
        path.display()
    );
}

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

fn silhouette(pm: &Pixmap) -> f32 {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let (cx, cy) = (w * 0.5, h * 0.5);
    let mut far = 0.0f32;
    for (i, px) in pm.data().chunks_exact(4).enumerate() {
        if px[3] <= 8 {
            continue;
        }
        let x = (i as f32 % w) - cx;
        let y = (i as f32 / w) - cy;
        far = far.max((x * x + y * y).sqrt());
    }
    far / (w.min(h))
}

fn resize(src: &Pixmap, size: u32) -> Pixmap {
    let mut out = Pixmap::new(size, size).expect("resized");
    let ratio = src.width() as f32 / size as f32;
    for y in 0..size {
        for x in 0..size {
            let sx = ((x as f32 + 0.5) * ratio) as u32;
            let sy = ((y as f32 + 0.5) * ratio) as u32;
            let s = (sy.min(src.height() - 1) * src.width() + sx.min(src.width() - 1)) as usize;
            let px = &src.data()[s * 4..s * 4 + 4];
            if px[3] == 0 {
                continue;
            }
            let d = ((y * size + x) as usize) * 4;
            out.data_mut()[d..d + 4].copy_from_slice(px);
        }
    }
    out
}

/// tiny-skia 全是**预乘**：source-over 必须按 `dst = src + dst*(255-a)/255` 算。
fn blit(pm: &mut Pixmap, x: u32, y: u32, src: &[u8]) {
    if x >= pm.width() || y >= pm.height() {
        return;
    }
    let i = ((y * pm.width() + x) as usize) * 4;
    let a = src[3] as u32;
    if a == 255 {
        pm.data_mut()[i..i + 4].copy_from_slice(src);
        return;
    }
    for c in 0..4 {
        let s = src[c] as u32;
        let d = pm.data()[i + c] as u32;
        pm.data_mut()[i + c] = (s + d * (255 - a) / 255).min(255) as u8;
    }
}

/// 尺寸对照图：浅底/深底两行，256 / 128 / 64 / 32 四列。
fn preview(body_pm: &Pixmap, dir: &Path) {
    let scales = [256u32, 128, 64, 32];
    let (pad, gap) = (14u32, 10u32);
    let cell = 256 + pad * 2;
    let width = cell * scales.len() as u32 + gap * (scales.len() as u32 - 1);
    let height = cell * 2 + gap;

    let mut sheet = Pixmap::new(width, height).expect("sheet");
    let mut bg = paint(Color::from_rgba8(28, 24, 22, 255));
    sheet.fill_rect(
        tiny_skia::Rect::from_xywh(0.0, 0.0, width as f32, cell as f32).unwrap(),
        &bg,
        Transform::identity(),
        None,
    );
    bg.set_color(Color::from_rgba8(244, 240, 234, 255));
    sheet.fill_rect(
        tiny_skia::Rect::from_xywh(
            0.0,
            (cell + gap) as f32,
            width as f32,
            (height - cell - gap) as f32,
        )
        .unwrap(),
        &bg,
        Transform::identity(),
        None,
    );

    for row in 0..2u32 {
        for (i, size) in scales.iter().enumerate() {
            let dst_x = (i as u32 * (cell + gap) + pad + (256 - size) / 2) as f32;
            let dst_y = (row * (cell + gap) + pad + (256 - size) / 2) as f32;
            let scaled = resize(body_pm, *size);
            for (idx, px) in scaled.data().chunks_exact(4).enumerate() {
                if px[3] == 0 {
                    continue;
                }
                blit(
                    &mut sheet,
                    dst_x as u32 + (idx as u32 % size),
                    dst_y as u32 + (idx as u32 / size),
                    px,
                );
            }
        }
    }
    let path = dir.join("paper2-sizes.png");
    sheet.save_png(&path).expect("save preview");
    println!("sizes -> {}", path.display());
}

/// 走**真正的加载路径**（`SkinPackage` → `Skin::decode` → `paint::render`）出两张图：
/// 单独一只、以及张开气泡环的样子。皮肤是数据，只有经过解码器才证明它真的能用。
fn in_app(dir: &Path, manifest: &str, body_png: &[u8], blink_png: &[u8]) {
    let package = SkinPackage {
        manifest: SkinManifest::parse(manifest).expect("manifest parses"),
        dir: dir.to_path_buf(),
        images: vec![
            ("body.png".to_string(), body_png.to_vec()),
            ("blink.png".to_string(), blink_png.to_vec()),
        ],
    };
    let skin = Skin::decode(&package).expect("skin decodes");
    let theme = Theme::by_name("zouwu-warm");

    for (name, openness) in [("paper2-alone.png", 0.0f32), ("paper2-ring.png", 1.0)] {
        let logical = 360.0f32;
        let scale = 2.0f32;
        let px = (logical * scale).round() as u32;
        let mut pixmap = Pixmap::new(px, px).expect("pixmap");
        pixmap.fill(if openness > 0.5 {
            Color::from_rgba8(28, 24, 22, 255)
        } else {
            Color::from_rgba8(244, 240, 234, 255)
        });

        let center = Point::new(logical * 0.5, logical * 0.5);
        let view = Rect::new(0.0, 0.0, logical, logical);
        let items = zouwu_core::action::resolve(
            &zouwu_core::config::RingCfg::default().items,
            &zouwu_core::i18n::I18n::new("zh-CN"),
        );
        let ring = zouwu_core::ring::RingConfig::default();
        let rotation = zouwu_core::ring::solve_rotation(&ring, center, view, items.len());
        let bubbles: Vec<BubblePaint<'_>> = ring
            .layout(items.len(), center, rotation)
            .into_iter()
            .zip(&items)
            .map(|(bubble, item)| BubblePaint {
                center: bubble.center,
                radius: bubble.radius,
                scale: openness,
                alpha: openness,
                glyph: item.glyph,
                icon: None,
                selected: false,
            })
            .collect();

        let scene = Scene {
            width: logical,
            height: logical,
            scale,
            buddy_center: center,
            buddy_radius: 56.0,
            buddy_scale: 1.0 + 0.06 * openness,
            buddy_alpha: 1.0,
            blink: 0.0,
            breath: 0.0,
            theme: &theme,
            bubbles: &bubbles,
            skin: Some(&skin),
        };
        render(&scene, &mut pixmap);
        let path = dir.join(name);
        pixmap.save_png(&path).expect("save png");
        println!("{name} -> {}", path.display());
    }
}

fn main() {
    let dir = out_dir();
    let body_pm = body();
    let (gx, gy) = centroid(&body_pm);
    assert!(
        (gx - 0.5).abs() < 0.04 && (gy - 0.5).abs() < 0.04,
        "body is off-center: centroid ({gx:.3}, {gy:.3})"
    );

    let scale = 1.0 / (2.0 * BLOB_RATIO);
    let sil = silhouette(&body_pm);
    assert!(
        sil > BLOB_RATIO && sil < BLOB_RATIO * 1.45,
        "装饰伸得太远：外轮廓 {sil:.3} vs 主体 {BLOB_RATIO:.3}"
    );
    println!("paper2: blob {BLOB_RATIO:.3} of side, silhouette {sil:.3} -> layer scale {scale:.3}");

    write_png(&dir, "body.png", body_pm, 12_000);
    let blink_pm = blink();
    write_png(&dir, "blink.png", blink_pm, 200);

    let manifest = format!(
        r#"# 折纸驺吾 2 —— 官方第四套皮肤
id = "zouwu-paper-2"
name = "折纸驺吾 2"
author = "zouwu"
version = "1.0.0"

[geometry]
radius = 56.0
breath_amount = 0.03

# 主体在 256 画布里占 0.6（半径 0.30），所以放大 1/0.6 ≈ 1.67 倍，`radius` 才
# 仍然表示"桌面上的主体半径"。这个数由 make_skin_paper2.rs 按实测占比反推。
[[layers]]
image = "body.png"
scale = {scale:.2}

[[layers]]
image = "blink.png"
role = "blink"
scale = {scale:.2}
"#
    );
    std::fs::write(dir.join("skin.toml"), &manifest).expect("write skin.toml");
    println!("wrote skin.toml");

    let body_png = std::fs::read(dir.join("body.png")).expect("read body");
    let blink_png = std::fs::read(dir.join("blink.png")).expect("read blink");
    let preview_pm = body();
    preview(&preview_pm, &preview_dir());
    in_app(&preview_dir(), &manifest, &body_png, &blink_png);
}
