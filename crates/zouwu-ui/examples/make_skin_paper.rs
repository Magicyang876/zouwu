//! 生成官方第三套皮肤「折纸驺吾」的图层 PNG。
//!
//! ```bash
//! cargo run --release -p zouwu-ui --example make_skin_paper
//! ```
//!
//! 输出到**仓库内**的 `assets/skins/zouwu-paper/`（打包进程序，首次启动落到
//! `%APPDATA%/zouwu/skins/`），生成物要提交进 git。
//!
//! ## 这套形象的立意
//!
//! 代码绘制那只的质感是"毛茸茸"（径向渐变的柔光），水墨驺吾是"墨会晕开"。
//! 折纸是第三种质感：**没有渐变，只有面**——同一块颜色靠亮面和暗面折出体积，
//! 边界是直线。所以这里刻意不用任何渐变或模糊：全部是纯色多边形 + 一条折痕线。
//!
//! 配色取自传统颜料名：藤黄（主体）、雌黄（亮面）、赭石（暗面）、松烟（眼与折痕）。

use std::path::{Path, PathBuf};

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

const SIDE: u32 = 256;
/// 主体（不含耳朵）在画布里的半径占比。`geometry.radius` 的口径就是它，
/// 所以图层 scale 必须由它反推（见 main 里的注释），不能手调。
const BLOB_RATIO: f32 = 0.30;

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("skins")
        .join("zouwu-paper");
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

/// 藤黄：主体纸面。
fn gamboge() -> Color {
    Color::from_rgba8(232, 163, 61, 255)
}
/// 雌黄：迎光的那一面。
fn orpiment() -> Color {
    Color::from_rgba8(247, 202, 104, 255)
}
/// 赭石：背光的那一面。
fn ochre() -> Color {
    Color::from_rgba8(196, 124, 42, 255)
}
/// 松烟：眼、折痕。
fn soot() -> Color {
    Color::from_rgba8(42, 28, 20, 255)
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

/// 多边形：顶点按顺序给，自动闭合。
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

/// 圆点（眼睛）。用 8 边形近似而不是真圆——折纸没有曲线，眼也切成小方块状。
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

/// 主体：一张八边形的纸，沿一条折痕分成迎光/背光两半，再加两片折耳。
///
/// 关键不是"画几个好看的多边形"，而是**面必须首尾相接**：折痕两侧的三角共用
/// 同一条边，眼睛才会把它读成"一块纸折了一下"。第一版画了两块互不相邻的菱形，
/// 结果像贴了两张糖纸。
fn body() -> Pixmap {
    let mut pm = canvas();
    let s = SIDE as f32;
    let (cx, cy) = (s * 0.5, s * 0.53);
    let r = s * BLOB_RATIO;

    // 八边形顶点：手工剪的纸不会是正八边形，半径与角度都给一点不规则。
    let jitter = [1.00f32, 0.97, 1.03, 0.98, 1.02, 0.96, 1.04, 0.99];
    let mut v = Vec::with_capacity(8);
    for (i, k) in jitter.iter().enumerate() {
        let a = std::f32::consts::TAU * i as f32 / 8.0 + 0.20;
        v.push((cx + r * k * a.cos(), cy + r * k * a.sin()));
    }

    // 折痕从左下顶点连到右上顶点：这两点是"最高/最低"的两个折角。
    // v[5] ≈ 左下方、v[1] ≈ 右上方（角度递增、y 向下）。
    let crease_a = v[5];
    let crease_b = v[1];

    // 背光半（折痕下方）：由外轮廓的下半顶点组成，整片赭石。
    for i in 5..=8 {
        let p = v[i % 8];
        let q = v[(i + 1) % 8];
        fill(&mut pm, &[crease_a, p, q], ochre());
    }
    fill(&mut pm, &[crease_a, v[0], crease_b], ochre());
    // 迎光半（折痕上方）：雌黄。
    fill(&mut pm, &[crease_a, v[4], v[3], crease_b], orpiment());
    fill(&mut pm, &[crease_a, v[3], v[2], crease_b], gamboge());
    fill(&mut pm, &[crease_a, v[2], crease_b], orpiment());

    // 背光半里再折一道：从底部顶点到折痕，给下半一个更暗的三角。
    fill(&mut pm, &[v[6], v[7], crease_b], tint(soot(), 0.16));

    // 折痕本身：纸折过的地方是灰的，不是黑的，所以用低透明度的松烟。
    stroke(&mut pm, &[crease_a, crease_b], tint(soot(), 0.30), 2.2);

    // 耳朵：有宽度的折片（底边宽 → 耳尖），靠里的半边是迎光面。
    for side in [-1.0f32, 1.0] {
        let inner = (cx + side * 0.10 * r, cy - 0.86 * r);
        let outer = (cx + side * 1.02 * r, cy - 0.40 * r);
        let tip = (cx + side * 0.52 * r, cy - 1.30 * r);
        fill(&mut pm, &[inner, outer, tip], gamboge());
        // 折面：底边中点 → 耳尖，把耳朵对折成亮/暗两半。
        let mid = ((inner.0 + outer.0) * 0.5, (inner.1 + outer.1) * 0.5);
        if side < 0.0 {
            fill(&mut pm, &[inner, mid, tip], orpiment());
        } else {
            fill(&mut pm, &[mid, outer, tip], ochre());
        }
        stroke(&mut pm, &[mid, tip], tint(soot(), 0.25), 1.6);
    }

    // 口鼻：一个朝下的亮三角（受光的折面），尖端一枚小鼻。
    let (mx, my) = (cx, cy + 0.20 * r);
    fill(
        &mut pm,
        &[
            (mx - 0.21 * r, my),
            (mx + 0.21 * r, my),
            (mx, my + 0.34 * r),
        ],
        orpiment(),
    );
    fill(
        &mut pm,
        &[
            (mx - 0.075 * r, my + 0.30 * r),
            (mx + 0.075 * r, my + 0.30 * r),
            (mx, my + 0.40 * r),
        ],
        soot(),
    );

    // 眼：小八边形点。折纸形象用"点"而不是"眼白+瞳孔"，多一点细节就脏了。
    for side in [-1.0f32, 1.0] {
        dot(
            &mut pm,
            cx + side * 0.34 * r,
            cy - 0.12 * r,
            0.12 * r,
            soot(),
        );
    }

    pm
}

/// 闭眼层：只画两条向下弯的折线（安静、不抢戏）。
fn blink() -> Pixmap {
    let mut pm = canvas();
    let s = SIDE as f32;
    let (cx, cy) = (s * 0.5, s * 0.53);
    let r = s * BLOB_RATIO;
    for side in [-1.0f32, 1.0] {
        let x = cx + side * 0.36 * r;
        let y = cy - 0.08 * r;
        stroke(
            &mut pm,
            &[(x - 0.11 * r, y), (x, y + 0.075 * r), (x + 0.11 * r, y)],
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

/// 外轮廓最大半径（相对画布边长的一半），用来检查装饰别伸太远。
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

/// 对照图：同一只小兽在深色/浅色底上、三种尺寸里各画一遍。
fn preview(body: &Pixmap, dir: &Path) {
    let scales = [(256u32, 1.0f32), (64, 0.25), (32, 0.125)];
    let (pad, gap) = (16u32, 12u32);
    let cell = 256 + pad * 2;
    let width = cell * scales.len() as u32 + gap * (scales.len() as u32 - 1);
    let height = cell * 2 + gap;

    let mut sheet = Pixmap::new(width, height).expect("sheet");
    // 深色底 / 浅色底两行
    let mut bg = paint(Color::from_rgba8(28, 24, 22, 255));
    sheet.fill_rect(
        tiny_skia::Rect::from_xywh(0.0, 0.0, width as f32, (cell) as f32).unwrap(),
        &bg,
        Transform::identity(),
        None,
    );
    bg.set_color(Color::from_rgba8(244, 240, 234, 255));
    sheet.fill_rect(
        tiny_skia::Rect::from_xywh(0.0, (cell + gap) as f32, width as f32, cell as f32).unwrap(),
        &bg,
        Transform::identity(),
        None,
    );

    for (row, _) in [0, 1].iter().enumerate() {
        for (i, (size, _)) in scales.iter().enumerate() {
            let dst_x = (i as u32 * (cell + gap) + pad + (256 - size) / 2) as f32;
            let dst_y = (row as u32 * (cell + gap) + pad + (256 - size) / 2) as f32;
            // 这里手动放大缩小：tiny-skia 的 draw_pixmap 参数组合容易踩坑（见 make_icon）。
            let scaled = resize(body, *size);
            for (idx, px) in scaled.data().chunks_exact(4).enumerate() {
                if px[3] == 0 {
                    continue;
                }
                let x = dst_x as u32 + (idx as u32 % size);
                let y = dst_y as u32 + (idx as u32 / size);
                blit(&mut sheet, x, y, px);
            }
        }
    }
    let path = dir.join("paper-skin.png");
    sheet.save_png(&path).expect("save preview");
    println!("preview -> {}", path.display());
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

/// tiny-skia 全是**预乘**：source-over 必须按 `dst = src + dst*(255-a)/255` 算，
/// 直接相加会在浅底上整片发白（make_icon 踩过一次）。
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

fn main() {
    let dir = out_dir();
    let body_pm = body();
    let (gx, gy) = centroid(&body_pm);
    assert!(
        (gx - 0.5).abs() < 0.04 && (gy - 0.5).abs() < 0.04,
        "body is off-center: centroid ({gx:.3}, {gy:.3})"
    );

    // 与水墨驺吾同一套口径：`geometry.radius` 表示"主体半径"（不含耳朵），
    // 所以 scale 由主体在画布里占多大反推 —— 手调会让环菜单贴到身上。
    let scale = 1.0 / (2.0 * BLOB_RATIO);
    let sil = silhouette(&body_pm);
    assert!(
        sil > BLOB_RATIO && sil < BLOB_RATIO * 1.45,
        "装饰伸得太远：外轮廓 {sil:.3} vs 主体 {BLOB_RATIO:.3}"
    );
    println!(
        "paper: blob {:.3} of side, silhouette {:.3} -> layer scale {:.3}",
        BLOB_RATIO, sil, scale
    );

    write_png(&dir, "body.png", body_pm, 12_000);
    write_png(&dir, "blink.png", blink(), 200);

    let manifest = format!(
        r#"# 折纸驺吾 —— 官方第三套皮肤
id = "zouwu-paper"
name = "折纸驺吾"
author = "zouwu"
version = "1.0.0"

[geometry]
radius = 56.0
breath_amount = 0.03

# 主体在 256 画布里占 0.6（半径 0.30），所以放大 1/0.6 ≈ 1.67 倍，`radius` 才
# 仍然表示"桌面上的主体半径"。这个数由 make_skin_paper.rs 按实测占比反推。
[[layers]]
image = "body.png"
scale = {scale:.2}

[[layers]]
image = "blink.png"
role = "blink"
scale = {scale:.2}
"#
    );
    std::fs::write(dir.join("skin.toml"), manifest).expect("write skin.toml");
    println!("wrote skin.toml");

    let preview_pm = body();
    preview(&preview_pm, &preview_dir());
}
