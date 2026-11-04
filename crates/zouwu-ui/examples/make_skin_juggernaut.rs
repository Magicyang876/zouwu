//! 生成官方第六套皮肤「剑圣」（`assets/skins/zouwu-juggernaut/`）。
//!
//! ## 为什么是这几个形状
//!
//! 大魔导是**竖的**：尖帽 + 垂到脚的长袍 + 右手边一根直立法杖，绿光。
//! 剑圣要是也画成"一个人 + 一根杆"，缩到 32 px 两只就分不出来了。所以反过来：
//!
//! - **横**：双角从面具两侧往**外**平挑再上勾（不是从头顶竖着长），肩甲横向铺开，
//!   整只兽比大魔导宽一圈；
//! - **一条斜线**：长刀从右肩前方斜劈到左下角，刃口一道亮线。这是全图唯一的长直线，
//!   32 px 时最抓眼的就是它；
//! - **暖**：暗绯红布 + 象牙面具 + 余烬橙的眼睛，和大魔导的墨绿冷光完全相反。
//!
//! 面具里不画五官细节，只留两道眼缝——折纸/剪纸风格里，32 px 能读出来的只有
//! 「一大块亮面 + 两个亮点」。
//!
//! 运行：`cargo run --release --example make_skin_juggernaut -p zouwu-ui`
//! 产物：`assets/skins/zouwu-juggernaut/{body,blink,overlay}.png` + `skin.toml`，
//! 预览图在 `target/zouwu-offscreen/`。

use std::path::{Path, PathBuf};

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
use zouwu_core::geom::{Point, Rect};
use zouwu_core::skin::SkinManifest;
use zouwu_core::theme::Theme;
use zouwu_platform::assets::SkinPackage;
use zouwu_ui::paint::{render, BubblePaint, Scene};
use zouwu_ui::skin::Skin;

const SIDE: u32 = 256;
/// 主体（不含角与刀）在画布里的半径占比。`geometry.radius` 的口径就是它，
/// 图层的 scale 由它反推，不能手调。
const BLOB_RATIO: f32 = 0.30;

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("skins")
        .join("zouwu-juggernaut");
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

/// 暗绯红：外袍的主面。
fn cloak() -> Color {
    Color::from_rgba8(126, 42, 38, 255)
}
/// 迎光的一面（光从左上来，与整只兽一致）。
fn cloak_lit() -> Color {
    Color::from_rgba8(168, 60, 52, 255)
}
/// 里子、背光面、肩甲的暗面。
fn cloak_dark() -> Color {
    Color::from_rgba8(74, 24, 24, 255)
}
/// 象牙：面具的主面。
fn bone() -> Color {
    Color::from_rgba8(226, 214, 188, 255)
}
/// 象牙的暗面：面具右半边、角的背光面。
fn bone_dark() -> Color {
    Color::from_rgba8(176, 162, 136, 255)
}
/// 冷银：刀身。整套只有它是冷色，所以刀在暖红里特别跳。
fn steel() -> Color {
    Color::from_rgba8(212, 219, 224, 255)
}
/// 刀身的暗面。
fn steel_dark() -> Color {
    Color::from_rgba8(152, 162, 170, 255)
}
/// 铜金：镶边、护手、腰带的扣。
fn gold() -> Color {
    Color::from_rgba8(201, 162, 39, 255)
}
/// 亮金：金面的受光侧。
fn gold_lit() -> Color {
    Color::from_rgba8(232, 200, 103, 255)
}
/// 余烬橙：眼睛、浮着的火星。全图只有它发光。
fn ember() -> Color {
    Color::from_rgba8(255, 186, 92, 255)
}
/// 焦黑：最暗的一档，只用在折痕、眼窝、绑带。
fn soot() -> Color {
    Color::from_rgba8(26, 14, 14, 255)
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

/// 关于画布竖中线镜像。角、肩甲、眼都是左右各一份，写一遍就够。
fn mirror(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    points.iter().map(|(x, y)| (SIDE as f32 - x, *y)).collect()
}

/// 一条折起来的纸带：相邻两点之间是一段四边形，再沿对角线切成两个三角形。
///
/// 角、飘带都是这样的纸带。**不画曲线、不做圆头**——纸折出来的东西没有圆角，
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
        dot(pm, cx, cy, r * k, tint(ember(), alpha));
    }
}

/// 双角：从面具两侧的太阳穴往**外**平挑，末端再上勾。
///
/// 这是和大魔导分得最开的一处。大魔导的角从太阳穴往上长，跟尖帽并在一个方向上，
/// 轮廓还是"一根针"；这里的角往两侧铺开，顶部变成一个宽三角形。
/// 画在面具**之前**——根部要塞进面具后面，否则能看见两条接缝。
fn horns(pm: &mut Pixmap) {
    let tones = [
        (bone(), bone_dark()),
        (bone(), bone_dark()),
        (bone_dark(), mix(bone_dark(), soot(), 0.40)),
    ];
    let nodes = [
        (112.0f32, 78.0f32),
        (98.0, 60.0),
        (86.0, 46.0),
        (82.0, 36.0),
    ];
    let widths = [10.0f32, 8.0, 5.0, 1.6];
    ribbon(pm, &nodes, &widths, &tones);
    let (mirrored, _) = (mirror(&nodes), ());
    ribbon(pm, &mirrored, &widths, &tones);
    // 角根的一圈金箍，把"角插在面具上"这件事说清楚。
    for x in [110.0f32, 146.0] {
        line(
            pm,
            &[(x - 4.0, 77.0), (x + 4.0, 80.0)],
            tint(gold(), 0.9),
            2.4,
        );
    }
}

/// 外袍：上窄下宽，下摆三段锯齿。
///
/// 三层色（主面 / 左受光 / 右背光）而不是一层——纯色平涂在 64 px 以上会像一块红布，
/// 三块不同明度的面才撑得出体积。
fn coat(pm: &mut Pixmap) {
    fill(
        pm,
        &[
            (112.0, 104.0),
            (146.0, 104.0),
            (156.0, 140.0),
            (164.0, 170.0),
            (172.0, 208.0),
            (150.0, 218.0),
            (128.0, 206.0),
            (106.0, 218.0),
            (84.0, 208.0),
            (92.0, 170.0),
            (100.0, 140.0),
        ],
        cloak(),
    );
    fill(
        pm,
        &[
            (112.0, 107.0),
            (127.0, 107.0),
            (120.0, 200.0),
            (92.0, 174.0),
        ],
        cloak_lit(),
    );
    fill(
        pm,
        &[
            (138.0, 107.0),
            (152.0, 110.0),
            (163.0, 188.0),
            (147.0, 207.0),
        ],
        cloak_dark(),
    );
    // 下摆金边：三段锯齿各描一段，不闭合。
    line(
        pm,
        &[
            (84.0, 208.0),
            (106.0, 218.0),
            (128.0, 206.0),
            (150.0, 218.0),
            (172.0, 208.0),
        ],
        gold(),
        3.0,
    );
    // 腰带（obi）：金band + 中央一个深色的结。
    fill(
        pm,
        &[(94.0, 156.0), (162.0, 156.0), (164.0, 170.0), (92.0, 170.0)],
        gold(),
    );
    fill(
        pm,
        &[(94.0, 156.0), (162.0, 156.0), (162.0, 160.5), (94.0, 160.5)],
        gold_lit(),
    );
    fill(
        pm,
        &[
            (122.0, 157.0),
            (134.0, 157.0),
            (138.0, 167.0),
            (128.0, 174.0),
            (118.0, 167.0),
        ],
        cloak_dark(),
    );
    // 衣褶
    line(
        pm,
        &[(124.0, 174.0), (118.0, 200.0)],
        tint(soot(), 0.22),
        1.4,
    );
    line(
        pm,
        &[(136.0, 174.0), (140.0, 200.0)],
        tint(soot(), 0.18),
        1.4,
    );
}

/// 肩甲：两块斜切的方板，外侧垂下一点。
///
/// 肩宽刻意超出外袍（92..164），整只兽的轮廓因此变成"上宽下窄"的倒梯形——
/// 这是"武士"和"法师"最省事的区别。
fn plates(pm: &mut Pixmap) {
    for (plate, facet) in [(false, 0.0f32), (true, 0.0)] {
        let _ = facet;
        let mut outer = vec![
            (80.0f32, 110.0f32),
            (114.0, 102.0),
            (122.0, 126.0),
            (104.0, 144.0),
            (78.0, 136.0),
        ];
        let mut top = vec![
            (80.0f32, 110.0f32),
            (114.0, 102.0),
            (118.0, 113.0),
            (82.0, 120.0),
        ];
        if plate {
            outer = mirror(&outer);
            top = mirror(&top);
        }
        fill(pm, &outer, cloak_dark());
        fill(pm, &top, cloak_lit());
        line(pm, &[outer[2], outer[3], outer[4]], gold(), 2.6);
        line(pm, &[outer[0], outer[4]], tint(gold(), 0.8), 2.0);
        let studs = if plate {
            [(158.0f32, 126.0f32), (146.0, 134.0)]
        } else {
            [(98.0, 126.0), (110.0, 134.0)]
        };
        for (x, y) in studs {
            dot(pm, x, y, 2.4, gold_lit());
        }
    }
}

/// 面具：六边形，右半边压暗，眼窝一横条凹槽，只剩两道余烬橙的眼缝。
///
/// 不画嘴、不画鼻——32 px 下多一根线就糊成一团。
/// 立领：面具底下的一圈高领，把头和肩甲连成一块。
///
/// 没有它，下巴和外袍之间会露出一条黑缝，看着像留了山羊胡。
fn collar(pm: &mut Pixmap) {
    fill(
        pm,
        &[(104.0, 92.0), (152.0, 92.0), (156.0, 114.0), (100.0, 114.0)],
        cloak_dark(),
    );
    fill(
        pm,
        &[(104.0, 92.0), (128.0, 96.0), (128.0, 104.0), (101.0, 100.0)],
        mix(cloak_dark(), cloak(), 0.35),
    );
    line(pm, &[(104.0, 92.0), (152.0, 92.0)], tint(gold(), 0.85), 2.2);
}

fn head(pm: &mut Pixmap) {
    // 脖子：面具和外袍之间要有东西挡住缝。
    fill(
        pm,
        &[
            (122.0, 100.0),
            (134.0, 100.0),
            (136.0, 108.0),
            (120.0, 108.0),
        ],
        mix(soot(), cloak_dark(), 0.35),
    );
    let face = [
        (115.0, 60.0),
        (141.0, 60.0),
        (148.0, 76.0),
        (144.0, 95.0),
        (128.0, 106.0),
        (112.0, 95.0),
        (108.0, 76.0),
    ];
    fill(pm, &face, bone_dark());
    fill(
        pm,
        &[
            (115.0, 60.0),
            (128.0, 61.0),
            (128.0, 104.0),
            (113.0, 94.0),
            (109.0, 76.0),
        ],
        bone(),
    );
    // 眼窝：一条横贯的凹槽，两端窄中间宽。
    fill(
        pm,
        &[
            (111.0, 84.0),
            (128.0, 90.0),
            (145.0, 84.0),
            (145.0, 78.5),
            (128.0, 84.5),
            (111.0, 78.5),
        ],
        mix(soot(), bone_dark(), 0.18),
    );
    for (x, s) in [(120.0f32, 1.0f32), (136.0, -1.0)] {
        glow(pm, x, 89.0, 10.0);
        fill(
            pm,
            &[
                (x - 3.7 * s, 91.4),
                (x + 3.3 * s, 87.4),
                (x + 3.7 * s, 90.2),
                (x - 3.3 * s, 93.0),
            ],
            ember(),
        );
    }
    // 眉心一道竖折痕 + 下颌的暗边
    line(pm, &[(128.0, 66.0), (128.0, 78.0)], tint(soot(), 0.30), 1.4);
    line(
        pm,
        &[(114.0, 95.0), (128.0, 102.0), (142.0, 95.0)],
        tint(soot(), 0.26),
        1.5,
    );
}

/// 飘带：从领口往左下甩出去的两折。
///
/// 它不只是装饰——整套构图左边只有外袍，没有东西指出"风"的方向，
/// 单靠一块布料看着像立牌。
fn sash(pm: &mut Pixmap) {
    let tones = [
        (cloak_lit(), cloak()),
        (cloak_lit(), cloak()),
        (cloak(), cloak_dark()),
    ];
    ribbon(
        pm,
        &[(110.0, 116.0), (97.0, 132.0), (88.0, 146.0), (80.0, 158.0)],
        &[5.8, 4.8, 3.4, 1.6],
        &tones,
    );
}

/// 长刀：从右肩前方斜劈到左下角，刃口朝上（受光侧）。
///
/// 刀是**最后**画的，压在所有形体之上——"举在前面"才成立。
/// 全图唯一的冷色 + 唯一的长直线，32 px 时就靠它把剑圣和大魔导分开。
fn blade(pm: &mut Pixmap) {
    let a = (172.0f32, 101.0f32); // 护手处
    let b = (84.0f32, 186.0f32); // 刀尖
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux); // 指向左上 = 刃口一侧

    let at = |t: f32, off: f32| (a.0 + dx * t + nx * off, a.1 + dy * t + ny * off);
    // 半宽：护手处 4.4，刀尖收成尖。
    let p1 = at(0.0, 4.2);
    let p2 = at(0.0, -5.2);
    let p3 = at(1.0, -1.4);
    let p4 = at(1.0, 1.2);
    fill(pm, &[p1, p2, p3, p4], steel_dark());
    // 受光的左半（含刃口）
    fill(pm, &[p1, (a.0, a.1), (b.0, b.1), p4], steel());
    // 刃口亮线：比刀身亮一档，32 px 时这一道线就是"刀"
    line(
        pm,
        &[at(0.02, 2.4), at(0.99, 0.6)],
        mix(steel(), Color::from_rgba8(255, 255, 252, 255), 0.62),
        1.7,
    );
    // 鎺（刀身根部的一道暗箍）
    line(
        pm,
        &[at(0.06, 3.0), at(0.06, -4.0)],
        tint(soot(), 0.45),
        2.0,
    );

    // 柄：往右上伸，包两道金线
    let c = (a.0 - ux * 18.0, a.1 - uy * 18.0);
    let hp = [
        (a.0 + nx * 2.8, a.1 + ny * 2.8),
        (c.0 + nx * 2.8, c.1 + ny * 2.8),
        (c.0 - nx * 2.8, c.1 - ny * 2.8),
        (a.0 - nx * 2.8, a.1 - ny * 2.8),
    ];
    fill(pm, &hp, mix(cloak_dark(), soot(), 0.35));
    for t in [0.35f32, 0.72] {
        let m = (a.0 + (c.0 - a.0) * t, a.1 + (c.1 - a.1) * t);
        line(
            pm,
            &[
                (m.0 + nx * 2.8, m.1 + ny * 2.8),
                (m.0 - nx * 2.8, m.1 - ny * 2.8),
            ],
            tint(gold(), 0.85),
            1.6,
        );
    }
    // 握刀的手：先画手，护手压在上面——反过来的话护手会被手掌吃掉，
    // 整把刀看着就只剩一根棍子。
    fill(
        pm,
        &[(167.0, 96.0), (179.0, 92.0), (183.0, 102.0), (171.0, 108.0)],
        mix(cloak_dark(), soot(), 0.5),
    );
    line(pm, &[(170.0, 98.0), (179.0, 95.0)], tint(gold(), 0.55), 1.8);
    // 鍔（护手）：横着的一根金色短杆，几乎垂直于刀身
    fill(
        pm,
        &[
            (a.0 - uy * 11.0, a.1 + ux * 11.0),
            (a.0 + ux * 2.6 - uy * 11.0, a.1 + uy * 2.6 + ux * 11.0),
            (a.0 + ux * 2.6 + uy * 11.0, a.1 + uy * 2.6 - ux * 11.0),
            (a.0 + uy * 11.0, a.1 - ux * 11.0),
        ],
        gold(),
    );
    line(pm, &[at(0.0, 6.0), at(0.0, -6.0)], gold_lit(), 1.8);
}

fn body() -> Pixmap {
    let mut pm = canvas();
    horns(&mut pm);
    coat(&mut pm);
    plates(&mut pm);
    collar(&mut pm);
    head(&mut pm);
    sash(&mut pm);
    blade(&mut pm);
    pm
}

/// 闭眼层：两片盖住眼缝的眼皮 + 一道折痕。
///
/// 这一层是**盖在眼睛上**淡入的（`paint::draw_skin_buddy` 里的 `LayerRole::Blink`），
/// 所以必须是实心的；只画线条的话睁着的余烬会一直透出来。
fn blink() -> Pixmap {
    let mut pm = canvas();
    for (x, s) in [(120.0f32, 1.0f32), (136.0, -1.0)] {
        fill(
            &mut pm,
            &[
                (x - 4.6 * s, 87.4),
                (x + 4.6 * s, 86.6),
                (x + 4.6 * s, 92.6),
                (x - 4.6 * s, 93.2),
            ],
            bone_dark(),
        );
        line(
            &mut pm,
            &[(x - 3.8 * s, 90.8), (x + 3.8 * s, 90.0)],
            tint(soot(), 0.55),
            1.6,
        );
    }
    pm
}

/// 覆盖层：几粒隔空浮着的火星。
///
/// 放在覆盖层而不是画进 body，是因为它们**不随眨眼变**、也不该参与主体的
/// 质心与外轮廓自检。
fn overlay() -> Pixmap {
    let mut pm = canvas();
    for (x, y, r, a) in [
        (72.0f32, 112.0f32, 2.4f32, 0.85f32),
        (196.0, 124.0, 2.0, 0.72),
        (80.0, 186.0, 1.8, 0.62),
        (198.0, 178.0, 1.6, 0.55),
        (186.0, 66.0, 1.5, 0.50),
    ] {
        dot(&mut pm, x, y, r * 2.0, tint(ember(), a * 0.16));
        fill(
            &mut pm,
            &[(x, y - r * 1.7), (x + r, y), (x, y + r * 1.7), (x - r, y)],
            tint(ember(), a),
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

/// 把覆盖层叠到主体上：`preview()` 只接受一张图，尺寸对照图要看得到火星才算数。
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
    let path = dir.join("juggernaut-sizes.png");
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

    for (name, openness) in [
        ("juggernaut-alone.png", 0.0f32),
        ("juggernaut-ring.png", 1.0),
    ] {
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

/// 眨眼层在真实尺寸下会不会露馅：把两只眼睛盖上前后的样子各出一张。
fn blink_sheet(dir_in: &Path, dir_out: &Path) {
    let body = {
        let png = std::fs::read(dir_in.join("body.png")).expect("read body");
        Pixmap::decode_png(&png).expect("decode body")
    };
    let lids = {
        let png = std::fs::read(dir_in.join("blink.png")).expect("read blink");
        Pixmap::decode_png(&png).expect("decode blink")
    };
    let shut = composite(&body, &lids);
    let mut sheet = Pixmap::new(256 * 2 + 30, 256 + 28).expect("sheet");
    sheet.fill(Color::from_rgba8(244, 240, 234, 255));
    for (i, pm) in [&body, &shut].into_iter().enumerate() {
        for (idx, px) in pm.data().chunks_exact(4).enumerate() {
            if px[3] == 0 {
                continue;
            }
            blit(
                &mut sheet,
                14 + i as u32 * 286 + (idx as u32 % 256),
                14 + (idx as u32 / 256),
                px,
            );
        }
    }
    let path = dir_out.join("juggernaut-blink.png");
    sheet.save_png(&path).expect("save blink sheet");
    println!("blink -> {}", path.display());
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
    println!(
        "juggernaut: blob {BLOB_RATIO:.3} of side, silhouette {sil:.3} -> layer scale {scale:.3}"
    );

    write_png(&dir, "body.png", body(), 12_000);
    write_png(&dir, "blink.png", blink(), 80);
    write_png(&dir, "overlay.png", overlay(), 40);

    let manifest = format!(
        r#"# 剑圣 —— 官方第六套皮肤
id = "zouwu-juggernaut"
name = "剑圣"
author = "zouwu"
version = "1.0.0"

[geometry]
radius = 56.0
breath_amount = 0.03

# 主体在 256 画布里占 0.6（半径 0.30），所以放大 1/0.6 ≈ 1.67 倍，`radius` 才
# 仍然表示"桌面上的主体半径"。这个数由 make_skin_juggernaut.rs 按实测占比反推。
[[layers]]
image = "body.png"
scale = {scale:.2}

[[layers]]
image = "blink.png"
role = "blink"
scale = {scale:.2}

# 隔空浮着的火星。覆盖层的 `opacity` 是常驻的，不跟眨眼走。
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
    blink_sheet(&dir, &preview_dir());
    in_app(
        &preview_dir(),
        &manifest,
        &body_png,
        &blink_png,
        &overlay_png,
    );
}
