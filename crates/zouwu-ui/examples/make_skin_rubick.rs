//! 生成官方第五套皮肤「大魔导」的图层 PNG。
//!
//! ```bash
//! cargo run --release -p zouwu-ui --example make_skin_rubick
//! ```
//!
//! 输出到**仓库内**的 `assets/skins/zouwu-rubick/`（打包进程序，首次启动落到
//! `%APPDATA%/zouwu/skins/`），生成物要提交进 git。
//!
//! ## 和前四套的关系
//!
//! 规则没变：纯色面 + 折痕线，没有渐变、没有模糊、没有曲线。换的是题材——
//! 一位**飘着的长袍法师**：兜帽、一对向上外弯的骨角、尖领、握在手里的法杖、
//! 袍角下面的浮空光。多出来的是两样东西：
//!
//! 1. **发光**：眼睛、宝石、浮着的碎晶用同一支鬼火绿，用同心八边形堆出柔光
//!    （没有高斯模糊可用）。发光物在缩到 32 px 之后必须比周围亮一档，
//!    否则整只兽会糊成一块深绿——这是这套皮肤唯一"高科技"的地方。
//! 2. **第三层**：`overlay.png` 装那几片隔空浮着的碎晶。它不参与主体的质心与
//!    外轮廓自检（那两项只管 `body`），也不随眨眼变。
//!
//! 配色：墨绿长袍 + 铜金镶边 + 灰白脸 + 鬼火绿。没有曲线、没有圆角，连手都切六边形。

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
        .join("zouwu-rubick");
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

/// 墨绿：长袍的主面、兜帽的受光侧。
fn robe() -> Color {
    Color::from_rgba8(36, 82, 62, 255)
}
/// 苔绿：迎光的那一面。
fn robe_lit() -> Color {
    Color::from_rgba8(52, 112, 80, 255)
}
/// 深墨绿：兜帽里子、尖领、背光面。
fn robe_dark() -> Color {
    Color::from_rgba8(24, 52, 41, 255)
}
/// 铜金：镶边、尖领的边、法杖上的箍。
fn gold() -> Color {
    Color::from_rgba8(201, 162, 39, 255)
}
/// 亮金：金面的受光侧、宝石的高光面。
fn gold_lit() -> Color {
    Color::from_rgba8(232, 200, 103, 255)
}
/// 灰白：脸、手掌。
fn pale() -> Color {
    Color::from_rgba8(203, 216, 206, 255)
}
/// 灰白暗面：脸的下半、骨角、杖杆。
fn pale_dark() -> Color {
    Color::from_rgba8(158, 177, 165, 255)
}
/// 鬼火绿：眼睛、宝石、浮着的碎晶。全图只有它发光。
fn arcane() -> Color {
    Color::from_rgba8(123, 240, 176, 255)
}
/// 松烟：最暗的一档，只用在折痕、眼窝、杖杆。
fn soot() -> Color {
    Color::from_rgba8(15, 26, 22, 255)
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
/// 骨角、袖子、下摆的碎布都是这样的纸带。**不画曲线、不做圆头**——纸折出来的东西没有圆角，
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

/// 开路径描边：`poly()` 会 `close()`，描折线时不能再用它（会多画一条回到起点的线）。
fn line(pm: &mut Pixmap, points: &[(f32, f32)], color: Color, width: f32) {
    let Some((first, rest)) = points.split_first() else {
        return;
    };
    let mut pb = PathBuilder::new();
    pb.move_to(first.0, first.1);
    for (x, y) in rest {
        pb.line_to(*x, *y);
    }
    if let Some(path) = pb.finish() {
        let s = Stroke {
            width,
            ..Default::default()
        };
        pm.stroke_path(&path, &paint(color), &s, Transform::identity(), None);
    }
}

/// 两个不透明颜色按 `t` 混（`tint()` 只改 alpha，压暗要靠这个）。
fn mix(a: Color, b: Color, t: f32) -> Color {
    let f = |x: f32, y: f32| (x + (y - x) * t).clamp(0.0, 1.0);
    let ch = |v: f32| (v * 255.0).round() as u8;
    Color::from_rgba8(
        ch(f(a.red(), b.red())),
        ch(f(a.green(), b.green())),
        ch(f(a.blue(), b.blue())),
        255,
    )
}

/// 同心八边形堆出来的"柔光"。没有高斯模糊可用，四层递减的 alpha 在缩到 32 px
/// 之后和真模糊没有区别。
fn glow(pm: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    for (k, alpha) in [(1.0f32, 0.06f32), (0.74, 0.09), (0.50, 0.14), (0.30, 0.20)] {
        dot(pm, cx, cy, r * k, tint(arcane(), alpha));
    }
}

/// 法杖：一根细杆 + 两道金箍 + 顶上一颗菱形宝石。
///
/// 杖身画在**长袍之前**——它要能被袖子压住一半，否则"握着"这件事不成立。
fn staff(pm: &mut Pixmap) {
    let (bx, by) = (166.0f32, 216.0f32);
    let (tx, ty) = (192.0f32, 112.0f32);
    let (dx, dy) = (tx - bx, ty - by);
    let len = (dx * dx + dy * dy).sqrt();
    let (nx, ny) = (-dy / len * 3.4, dx / len * 3.4);

    // 杆：暗面整体 + 左半的受光面（光从左上来，与整只兽一致）。
    fill(
        pm,
        &[
            (bx + nx, by + ny),
            (tx + nx, ty + ny),
            (tx - nx, ty - ny),
            (bx - nx, by - ny),
        ],
        soot(),
    );
    fill(
        pm,
        &[(bx + nx, by + ny), (tx + nx, ty + ny), (tx, ty), (bx, by)],
        mix(pale_dark(), soot(), 0.55),
    );

    // 金箍：横跨杖身的短粗线，位置按竿的 30% 与 74% 取。
    for t in [0.30f32, 0.74] {
        let (cx, cy) = (bx + dx * t, by + dy * t);
        line(
            pm,
            &[
                (cx + nx * 1.3, cy + ny * 1.3),
                (cx - nx * 1.3, cy - ny * 1.3),
            ],
            gold(),
            4.0,
        );
    }

    // 宝石：金托 + 鬼火绿核心 + 一道折面 + 竖折痕。
    let (gx, gy) = (194.0f32, 96.0f32);
    fill(
        pm,
        &[
            (gx, gy - 18.0),
            (gx + 13.0, gy),
            (gx, gy + 18.0),
            (gx - 13.0, gy),
        ],
        gold(),
    );
    fill(
        pm,
        &[
            (gx, gy - 13.0),
            (gx + 9.5, gy),
            (gx, gy + 13.0),
            (gx - 9.5, gy),
        ],
        arcane(),
    );
    fill(
        pm,
        &[(gx, gy - 13.0), (gx, gy + 13.0), (gx - 9.5, gy)],
        mix(gold_lit(), arcane(), 0.45),
    );
    line(
        pm,
        &[(gx, gy - 18.0), (gx, gy + 18.0)],
        tint(soot(), 0.28),
        1.2,
    );
}

/// 长袍：小、A 字、收到一个尖上。
///
/// **比例是这套皮肤的第一原则**：头大、身小才是 Q 版法师；上一版头 56 宽、身体
/// 88 宽，缩到 32 px 直接读成甲虫——壳大、头小、六条腿。
///
/// 金边只画**前摆**这一小段，不绕整圈：绕满一圈，整件袍子就变成一块镶了金边的盾牌。
fn drape(pm: &mut Pixmap) {
    let (dark1, dark2) = (mix(robe(), soot(), 0.16), mix(robe(), soot(), 0.36));

    fill(
        pm,
        &[
            (104.0, 150.0),
            (126.0, 150.0),
            (122.0, 214.0),
            (106.0, 212.0),
            (96.0, 182.0),
        ],
        robe_lit(),
    );
    fill(
        pm,
        &[
            (126.0, 150.0),
            (136.0, 150.0),
            (132.0, 226.0),
            (122.0, 214.0),
        ],
        robe(),
    );
    fill(
        pm,
        &[
            (136.0, 150.0),
            (152.0, 150.0),
            (150.0, 212.0),
            (132.0, 226.0),
        ],
        dark1,
    );
    fill(pm, &[(152.0, 150.0), (160.0, 182.0), (150.0, 212.0)], dark2);

    for (a, b) in [
        ((126.0, 150.0), (122.0, 214.0)),
        ((136.0, 150.0), (132.0, 226.0)),
    ] {
        line(pm, &[a, b], tint(soot(), 0.28), 1.3);
    }

    // 腰带：一道横过腰的金箍 + 中间一颗扣。横向的一道能立刻把"甲壳"打断成"衣服"。
    fill(
        pm,
        &[(98.0, 180.0), (158.0, 180.0), (157.0, 192.0), (99.0, 192.0)],
        gold(),
    );
    fill(
        pm,
        &[(98.0, 180.0), (158.0, 180.0), (157.0, 186.0), (99.0, 186.0)],
        gold_lit(),
    );
    fill(
        pm,
        &[
            (128.0, 177.0),
            (133.5, 186.0),
            (128.0, 195.0),
            (122.5, 186.0),
        ],
        arcane(),
    );

    // 前摆金边。
    line(
        pm,
        &[(106.0, 212.0), (128.0, 226.0), (150.0, 212.0)],
        gold(),
        2.6,
    );
}

/// 头：一顶大兜帽 + 眉带 + 脸 + 面具 + 一对弯角。**占了整只兽的一半高**。
fn head(pm: &mut Pixmap) {
    // 兜帽顶上不是圆的，是一个小尖——圆顶 + 两根向上的角就是兔子，尖顶就不会。
    let hood = [
        (96.0, 150.0),
        (86.0, 116.0),
        (92.0, 86.0),
        (104.0, 66.0),
        (118.0, 58.0),
        (128.0, 52.0),
        (138.0, 58.0),
        (152.0, 66.0),
        (164.0, 86.0),
        (170.0, 116.0),
        (160.0, 150.0),
    ];
    fill(pm, &hood, robe_dark());
    fill(
        pm,
        &[
            (86.0, 116.0),
            (92.0, 86.0),
            (104.0, 66.0),
            (118.0, 58.0),
            (128.0, 52.0),
            (128.0, 148.0),
            (96.0, 150.0),
        ],
        robe_lit(),
    );
    line(
        pm,
        &[(128.0, 52.0), (128.0, 148.0)],
        tint(soot(), 0.28),
        1.6,
    );

    // 脸：**兜帽里是暗的**。
    //
    // 这一块返工了三次：先是又宽又白的脸（读成机器人面罩），再是加眼窝加面具
    // （读成骑士头盔）。最后放弃了"把脸画出来"——兜帽的阴影里只有两团鬼火，
    // 亮的是下巴上那块金属面罩。32 px 下这个反而最清楚：暗底上两个亮点。
    let hollow = [
        (110.0, 96.0),
        (128.0, 92.0),
        (146.0, 96.0),
        (148.0, 118.0),
        (140.0, 140.0),
        (116.0, 140.0),
        (108.0, 118.0),
    ];
    fill(pm, &hollow, mix(soot(), robe_dark(), 0.20));
    stroke(pm, &hollow, tint(soot(), 0.35), 2.5);

    // 眼睛：暗处两团鬼火，向鼻梁内侧倾斜。
    // 圆眼在这里会变成猫；缝眼才是"兜帽阴影里发光的法师"。
    for (x, side) in [(115.0f32, 1.0f32), (141.0, -1.0)] {
        let (ox, ix) = (x - 6.0 * side, x + 6.0 * side);
        fill(
            pm,
            &[(ox, 106.0), (ix, 109.5), (ix, 113.5), (ox, 110.0)],
            tint(arcane(), 0.24),
        );
        fill(
            pm,
            &[(ox, 107.4), (ix, 110.6), (ix, 112.8), (ox, 109.6)],
            arcane(),
        );
    }

    // 面罩：下巴上一块亮着的金属板，顶边一道金线，中间一道中脊。
    // 这张面具不给你看嘴——兜帽的阴影里只有眼睛在发光。
    fill(
        pm,
        &[
            (114.0, 124.0),
            (142.0, 124.0),
            (139.0, 139.0),
            (117.0, 139.0),
        ],
        pale_dark(),
    );
    fill(
        pm,
        &[
            (128.0, 124.0),
            (142.0, 124.0),
            (139.0, 139.0),
            (128.0, 139.0),
        ],
        mix(pale_dark(), soot(), 0.20),
    );
    line(pm, &[(114.0, 124.0), (142.0, 124.0)], gold(), 1.8);
    // 千万别在面罩上开一排竖缝：三道竖条在 32 px 下就是一张咧嘴笑的牙。
    // 只留一道中脊 + 下巴的边。
    line(
        pm,
        &[(128.0, 125.0), (128.0, 138.0)],
        tint(soot(), 0.30),
        1.3,
    );
    line(
        pm,
        &[(118.0, 138.0), (138.0, 138.0)],
        tint(soot(), 0.34),
        1.3,
    );

    // 眉带：一道金箍绕过兜帽、从额上过去（**不压眼睛**）+ 正中一颗宝石。
    line(
        pm,
        &[
            (96.0, 98.0),
            (110.0, 83.0),
            (128.0, 79.0),
            (146.0, 83.0),
            (160.0, 98.0),
        ],
        gold(),
        4.0,
    );
    fill(
        pm,
        &[(128.0, 73.0), (133.0, 79.0), (128.0, 85.0), (123.0, 79.0)],
        arcane(),
    );

    // 角从**太阳穴**长出来、向上外弯——从头顶长就是兔耳。颜色用暗金属 + 一道亮边，
    // 顶上再套一枚金箍：金箍是"这是戴在头盔上的零件，不是身体"的唯一暗示。
    let horn_lit = mix(pale_dark(), pale(), 0.35);
    let horn = mix(pale_dark(), soot(), 0.45);
    let horn_dark = mix(soot(), robe_dark(), 0.45);
    for nodes in [
        [(92.0f32, 96.0f32), (78.0, 80.0), (66.0, 64.0), (60.0, 52.0)],
        [(164.0, 96.0), (178.0, 80.0), (190.0, 64.0), (196.0, 52.0)],
    ] {
        ribbon(
            pm,
            &nodes,
            &[11.0, 8.0, 5.0, 1.8],
            &[
                (horn_lit, horn_dark),
                (horn_lit, horn_dark),
                (horn, horn_dark),
            ],
        );
        // 角根的金箍。
        let (bx, by) = nodes[0];
        let (tx, ty) = nodes[1];
        let (dx, dy) = (tx - bx, ty - by);
        let len = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = (-dy / len * 5.6, dx / len * 5.6);
        line(pm, &[(bx + nx, by + ny), (bx - nx, by - ny)], gold(), 4.0);
    }

    // 领口：兜帽下沿的一道金边——高领的替身。这一版没有肩甲/尖领：
    // 头一大，肩上的尖角就全被头挡住，露出来的那点只会像虫子的腿。
    line(pm, &[(96.0, 149.0), (160.0, 149.0)], gold(), 2.4);
}

/// 两只袖子和手：右手扣在法杖上，左手向外抬着——他是"隔空取物"的，手不该抓到任何东西。
fn sleeves(pm: &mut Pixmap) {
    ribbon(
        pm,
        &[(104.0, 158.0), (88.0, 170.0), (74.0, 180.0)],
        &[13.0, 10.0, 8.0],
        &[(robe(), robe_lit()), (robe_lit(), robe())],
    );
    ribbon(
        pm,
        &[(152.0, 158.0), (166.0, 168.0), (176.0, 174.0)],
        &[13.0, 10.0, 8.0],
        &[
            (mix(robe(), soot(), 0.18), robe()),
            (robe(), mix(robe(), soot(), 0.18)),
        ],
    );

    for (cx, cy, r) in [(71.0f32, 182.0f32, 6.4f32), (178.0, 176.0, 6.0)] {
        dot(pm, cx, cy, r, pale());
        dot(
            pm,
            cx + 1.5,
            cy + 1.6,
            r * 0.60,
            mix(pale(), pale_dark(), 0.75),
        );
    }
}

/// 把覆盖层叠到主体上：`preview()` 只接受一张图，尺寸对照图要看得到碎晶才算数。
fn composite(base: &Pixmap, top: &Pixmap) -> Pixmap {
    let mut out = base.clone();
    for y in 0..top.height() {
        for x in 0..top.width() {
            let i = ((y * top.width() + x) as usize) * 4;
            let px = &top.data()[i..i + 4];
            if px[3] == 0 {
                continue;
            }
            let px = [px[0], px[1], px[2], px[3]];
            blit(&mut out, x, y, &px);
        }
    }
    out
}

fn body() -> Pixmap {
    let mut pm = canvas();
    // 光晕先画：它们要在所有形体**下面**，只从缝里透出来。
    // （曾经把眼睛的光晕画在最后，结果整张脸被糊成一个发光的圆。）
    glow(&mut pm, 194.0, 96.0, 22.0); // 法杖顶端
    glow(&mut pm, 128.0, 220.0, 12.0); // 袍角下的浮空光
    staff(&mut pm);
    drape(&mut pm);
    sleeves(&mut pm);
    head(&mut pm);
    pm
}

/// 闭眼层：两片盖住眼缝的眼皮 + 一道折痕。
///
/// 这一层是**盖在眼睛上**淡入的（`paint::draw_skin_buddy` 里的 `LayerRole::Blink`），
/// 所以必须是实心的；只画线条的话睁着的鬼火会一直透出来。
fn blink() -> Pixmap {
    let mut pm = canvas();
    for (x, s) in [(117.0f32, 1.0f32), (139.0, -1.0)] {
        let (ox, ix) = (x - 6.5 * s, x + 6.5 * s);
        fill(
            &mut pm,
            &[(ox, 108.0), (ix, 111.5), (ix, 117.5), (ox, 114.0)],
            mix(pale(), soot(), 0.06),
        );
        line(
            &mut pm,
            &[(ox, 111.4), (ix, 114.9)],
            tint(soot(), 0.55),
            1.8,
        );
    }
    pm
}

/// 覆盖层：隔空浮着的几片碎晶，全落在身体之外的空白处。
///
/// 放在覆盖层而不是画进 body，是因为它们**不随眨眼变**、也不该参与主体的
/// 质心与外轮廓自检。碎片本身很小，但"有东西在飘"这件事是这套皮肤的一半。
fn overlay() -> Pixmap {
    let mut pm = canvas();
    for (x, y, r, a) in [
        (66.0f32, 120.0f32, 2.6f32, 0.85f32),
        (198.0, 164.0, 2.2, 0.78),
        (206.0, 208.0, 1.8, 0.60),
        (58.0, 182.0, 2.0, 0.62),
        (168.0, 108.0, 1.6, 0.50),
    ] {
        dot(&mut pm, x, y, r * 1.9, tint(arcane(), a * 0.16));
        fill(
            &mut pm,
            &[(x, y - r * 1.7), (x + r, y), (x, y + r * 1.7), (x - r, y)],
            tint(arcane(), a),
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
    let path = dir.join("rubick-sizes.png");
    sheet.save_png(&path).expect("save preview");
    println!("sizes -> {}", path.display());
}

/// 走**真正的加载路径**（`SkinPackage` → `Skin::decode` → `paint::render`）出两张图：
/// 单独一只、以及张开气泡环的样子。皮肤是数据，只有经过解码器才证明它真的能用。
fn in_app(dir: &Path, manifest: &str, body_png: &[u8], blink_png: &[u8], overlay_png: &[u8]) {
    let package = SkinPackage {
        manifest: SkinManifest::parse(manifest).expect("manifest parses"),
        dir: dir.to_path_buf(),
        images: vec![
            ("body.png".to_string(), body_png.to_vec()),
            ("blink.png".to_string(), blink_png.to_vec()),
            ("overlay.png".to_string(), overlay_png.to_vec()),
        ],
    };
    let skin = Skin::decode(&package).expect("skin decodes");
    let theme = Theme::by_name("zouwu-warm");

    for (name, openness) in [("rubick-alone.png", 0.0f32), ("rubick-ring.png", 1.0)] {
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
    println!("rubick: blob {BLOB_RATIO:.3} of side, silhouette {sil:.3} -> layer scale {scale:.3}");

    write_png(&dir, "body.png", body(), 12_000);
    write_png(&dir, "blink.png", blink(), 80);
    write_png(&dir, "overlay.png", overlay(), 40);

    let manifest = format!(
        r#"# 大魔导 —— 官方第五套皮肤
id = "zouwu-rubick"
name = "大魔导"
author = "zouwu"
version = "1.0.0"

[geometry]
radius = 56.0
breath_amount = 0.03

# 主体在 256 画布里占 0.6（半径 0.30），所以放大 1/0.6 ≈ 1.67 倍，`radius` 才
# 仍然表示"桌面上的主体半径"。这个数由 make_skin_rubick.rs 按实测占比反推。
[[layers]]
image = "body.png"
scale = {scale:.2}

[[layers]]
image = "blink.png"
role = "blink"
scale = {scale:.2}

# 隔空浮着的碎晶。覆盖层的 `opacity` 是常驻的，不跟眨眼走。
[[layers]]
image = "overlay.png"
role = "overlay"
scale = {scale:.2}
"#
    );
    std::fs::write(dir.join("skin.toml"), &manifest).expect("write skin.toml");
    println!("wrote skin.toml");

    let body_png = std::fs::read(dir.join("body.png")).expect("read body");
    let blink_png = std::fs::read(dir.join("blink.png")).expect("read blink");
    let overlay_png = std::fs::read(dir.join("overlay.png")).expect("read overlay");
    let sheet = composite(&body_pm, &overlay());
    preview(&sheet, &preview_dir());
    in_app(
        &preview_dir(),
        &manifest,
        &body_png,
        &blink_png,
        &overlay_png,
    );
}
