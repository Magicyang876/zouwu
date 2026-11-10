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
        .join("zouwu-shadowfiend");
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

/// 暗紫近黑：躯体主面。影魔是"从烟里长出来的"，所以最暗的一档不当底色用。
fn hide() -> Color {
    Color::from_rgba8(36, 27, 46, 255)
}
/// 迎光的一面（光从左上来，与整只兽一致）。
fn hide_lit() -> Color {
    Color::from_rgba8(62, 48, 80, 255)
}
/// 背光面、膜的里子、下摆的烟。
fn hide_dark() -> Color {
    Color::from_rgba8(21, 15, 28, 255)
}
/// 灰骨：角与肩刺的主面。
fn bone() -> Color {
    Color::from_rgba8(184, 171, 196, 255)
}
/// 灰骨的背光面。
fn bone_dark() -> Color {
    Color::from_rgba8(122, 110, 142, 255)
}
/// 灵魂绿：眼睛、胸口的裂缝、浮着的魂火。全图只有它发光。
fn soul() -> Color {
    Color::from_rgba8(140, 242, 206, 255)
}
/// 灵魂绿的暗面：光晕与不发光的那部分。
fn soul_dim() -> Color {
    Color::from_rgba8(79, 169, 140, 255)
}
/// 余烬红：胸口的第二道裂、浮着的火星。
fn ember() -> Color {
    Color::from_rgba8(210, 85, 58, 255)
}
/// 焦黑：折痕、眼窝、嘴缝。比 `hide_dark()` 还暗一档。
fn soot() -> Color {
    Color::from_rgba8(10, 7, 14, 255)
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
/// 之后和真模糊没有区别。`color` 由调用方给：影魔身上绿的（魂火）和红的（余烬）都有。
fn glow(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, color: Color) {
    for (k, alpha) in [(1.0f32, 0.05f32), (0.74, 0.08), (0.50, 0.12), (0.30, 0.18)] {
        dot(pm, cx, cy, r * k, tint(color, alpha));
    }
}

/// 一根向上收尖的火舌：外焰暗、内焰亮，两片折线套在一起。
///
/// `lean` 是火舌往哪边歪——全都直上会像一排栅栏，歪一点才像在烧。
fn tongue(pm: &mut Pixmap, base_x: f32, base_y: f32, height: f32, width: f32, lean: f32) {
    let (w, h) = (width, height);
    // 三层套在一起：外焰宽而半透、中焰偏亮、芯子最亮最窄。
    // 没有渐变可用，"亮在里面"就是用递减宽度 + 递增不透明度堆出来的。
    for (k, alpha, spread) in [
        (1.0f32, 0.20f32, 1.0f32),
        (0.90, 0.42, 0.66),
        (0.80, 0.90, 0.34),
    ] {
        let box_w = w * spread;
        let tip = base_y - h * k;
        let shape = [
            (base_x - box_w, base_y),
            (base_x - box_w * 0.55 + lean * 0.25, base_y - h * k * 0.42),
            (base_x - box_w * 0.80 + lean * 0.55, base_y - h * k * 0.58),
            (base_x + lean * 0.85, tip),
            (base_x + box_w * 0.45 + lean * 0.55, base_y - h * k * 0.52),
            (base_x + box_w * 0.85 + lean * 0.25, base_y - h * k * 0.26),
            (base_x + box_w, base_y),
        ];
        let color = match spread {
            1.0 => tint(mix(soul_dim(), soul(), 0.15), alpha),
            0.68 => tint(mix(soul_dim(), soul(), 0.55), alpha),
            _ => tint(soul(), alpha),
        };
        fill(pm, &shape, color);
    }
}

/// 一簇火：中间最高、两边依次矮下去。单根火舌看起来像根尖刺，三根挤在一起才有"烧"的感觉。
/// 一簇火：中间最高、两边各一根矮的，彼此留出缝。
///
/// 单根火舌像根尖刺，三根挤在一起又会因为外焰半透 alpha 叠加变成一整块绿板
/// （在 2 倍尺寸下看起来像一对绿翅膀）。所以只给两根矮的、并且拉开距离。
fn cluster(pm: &mut Pixmap, x: f32, y: f32, height: f32, width: f32, lean: f32) {
    for (k, dx) in [(1.0f32, 0.0f32), (0.55, -1.20)] {
        tongue(
            pm,
            x + dx * width,
            y,
            height * k,
            width * (0.40 + 0.26 * k),
            lean * (0.6 + 0.5 * k),
        );
    }
}

/// 背后那一大片魂火：从肩后往上窜。
///
/// 影魔的辨识度一半在角、一半在"他整个人是烧着的"。第一版一点火都没有，
/// 所以像只蝙蝠状的黑色骑士。这一层画在身体之前，火苗从肩后透出来。
fn flames(pm: &mut Pixmap) {
    for (x, y, h, w, lean, glow_r) in [
        // 肩外侧：大半根火苗在轮廓外边，才能对着背景读成火；
        // 画在轮廓里面会变成一块半透的绿板。
        (78.0f32, 120.0f32, 54.0f32, 8.0f32, -13.0f32, 16.0f32),
        (178.0, 120.0, 50.0, 8.0, 13.0, 16.0),
        (96.0, 108.0, 34.0, 5.5, -7.0, 10.0),
        (160.0, 108.0, 30.0, 5.5, 7.0, 10.0),
    ] {
        glow(pm, x, y - h * 0.35, glow_r, soul());
        cluster(pm, x, y, h, w, lean);
    }
}

/// 贴在身上的火：腰侧与烟根。画在身体之后，所以是"从身体里冒出来"的。
fn flares(pm: &mut Pixmap) {
    for (x, y, h, w, lean) in [
        // 这两簇画在身体之后面（ flares 最后调），所以它们压在肩甲外缘上，
        // 看起来是"火苗从身上舔出来"——全部画在背后会变成一对绿翅膀。
        (74.0f32, 138.0f32, 34.0f32, 6.0f32, -13.0f32),
        (182.0, 138.0, 32.0, 6.0, 13.0),
        (104.0, 164.0, 30.0, 6.0, -6.0),
        (152.0, 164.0, 28.0, 6.0, 6.0),
    ] {
        cluster(pm, x, y, h, w, lean);
    }
}

/// 破烂的膜翼：从肩后向外摊开，边缘全是锯齿。
///
/// 影魔没有翅膀——这是他自己那层"披风状的皮"。放在最底下当背面，
/// 作用是把上半身撑宽，和"长袍"类的下宽上窄正好相反。
fn wings(pm: &mut Pixmap) {
    let left = [
        (106.0f32, 106.0f32),
        (88.0, 94.0),
        (64.0, 90.0),
        (74.0, 104.0),
        (58.0, 112.0),
        (76.0, 122.0),
        (66.0, 136.0),
        (88.0, 136.0),
        (86.0, 152.0),
        (104.0, 140.0),
    ];
    fill(pm, &left, mix(hide_dark(), hide(), 0.35));
    fill(
        pm,
        &[(98.0, 104.0), (88.0, 94.0), (72.0, 102.0), (86.0, 120.0)],
        mix(hide_dark(), hide_lit(), 0.45),
    );
    let crease = tint(mix(hide(), bone_dark(), 0.45), 0.55);
    line(pm, &[(96.0, 110.0), (68.0, 94.0)], crease, 1.6);
    line(pm, &[(98.0, 122.0), (70.0, 118.0)], crease, 1.4);
    line(pm, &[(100.0, 134.0), (78.0, 138.0)], crease, 1.2);
    stroke(pm, &left, tint(mix(hide(), bone_dark(), 0.5), 0.35), 1.6);
    let right = mirror(&left);
    fill(pm, &right, mix(hide_dark(), hide(), 0.35));
    stroke(pm, &right, tint(mix(hide(), bone_dark(), 0.5), 0.35), 1.6);
    line(pm, &mirror(&[(96.0, 110.0), (68.0, 94.0)]), crease, 1.6);
    line(pm, &mirror(&[(98.0, 122.0), (70.0, 118.0)]), crease, 1.4);
    line(pm, &mirror(&[(100.0, 134.0), (78.0, 138.0)]), crease, 1.2);
}

/// 下半身：没有腿，是三瓣往上收的烟。
///
/// 影魔是飘着的。三瓣而不是一整条：一整条会读成"长袍/钟形"，分成三瓣才有
/// "从下往上烧掉"的意思，也顺手把外轮廓底部收成锯齿。
fn smoke(pm: &mut Pixmap) {
    let mid = [
        (112.0f32, 144.0f32),
        (144.0, 144.0),
        (136.0, 176.0),
        (129.0, 208.0),
        (128.0, 228.0),
        (121.0, 206.0),
        (114.0, 174.0),
    ];
    let left = [
        (108.0f32, 148.0f32),
        (122.0, 158.0),
        (116.0, 180.0),
        (104.0, 196.0),
        (98.0, 176.0),
        (102.0, 160.0),
    ];
    let edge = tint(mix(hide(), soul_dim(), 0.35), 0.55);
    fill(pm, &left, hide_dark());
    fill(pm, &mid, hide_dark());
    fill(
        pm,
        &[
            (114.0, 146.0),
            (127.0, 146.0),
            (124.0, 204.0),
            (117.0, 174.0),
        ],
        mix(hide_dark(), hide(), 0.5),
    );
    stroke(pm, &left, tint(mix(hide(), bone_dark(), 0.5), 0.35), 1.6);
    let right = mirror(&left);
    fill(pm, &right, mix(hide_dark(), hide(), 0.35));
    stroke(pm, &right, tint(mix(hide(), bone_dark(), 0.5), 0.35), 1.6);
    line(
        pm,
        &[
            (112.0, 144.0),
            (114.0, 174.0),
            (121.0, 206.0),
            (128.0, 228.0),
        ],
        edge,
        1.2,
    );
    line(
        pm,
        &[
            (144.0, 144.0),
            (142.0, 176.0),
            (135.0, 206.0),
            (128.0, 228.0),
        ],
        edge,
        1.2,
    );
    line(
        pm,
        &[
            (108.0, 150.0),
            (102.0, 162.0),
            (98.0, 176.0),
            (104.0, 196.0),
        ],
        edge,
        1.4,
    );
    line(
        pm,
        &[
            (148.0, 150.0),
            (154.0, 162.0),
            (158.0, 176.0),
            (152.0, 196.0),
        ],
        edge,
        1.4,
    );
    // 旁边两条往上飘的细烟丝：给"飘"一个方向。
    for (x, dir) in [(92.0f32, -1.0f32), (164.0, 1.0)] {
        let wisp = [
            (x, 168.0),
            (x + dir * 14.0, 152.0),
            (x + dir * 8.0, 170.0),
            (x + dir * 16.0, 182.0),
        ];
        fill(pm, &wisp, tint(hide_dark(), 0.9));
        line(pm, &wisp, tint(mix(hide(), soul_dim(), 0.5), 0.6), 1.2);
    }
}

/// 胸腹：上宽下窄的楔形，左受光、右背光两块面分开。
fn torso(pm: &mut Pixmap) {
    fill(
        pm,
        &[
            (104.0, 100.0),
            (152.0, 100.0),
            (164.0, 120.0),
            (157.0, 150.0),
            (99.0, 150.0),
            (92.0, 120.0),
        ],
        hide(),
    );
    fill(
        pm,
        &[
            (104.0, 102.0),
            (126.0, 102.0),
            (121.0, 148.0),
            (100.0, 148.0),
            (96.0, 122.0),
        ],
        hide_lit(),
    );
    fill(
        pm,
        &[
            (134.0, 102.0),
            (150.0, 102.0),
            (160.0, 122.0),
            (154.0, 148.0),
            (139.0, 148.0),
        ],
        hide_dark(),
    );
    // 肋骨：三道骨色的折线，从胸骨往两侧包回去。中间让裂缝透出来。
    for (y, half, alpha) in [
        (114.0f32, 15.0f32, 0.55f32),
        (126.0, 17.0, 0.5),
        (138.0, 13.0, 0.40),
    ] {
        let rib = tint(bone_dark(), alpha);
        line(
            pm,
            &[
                (128.0 - half, y),
                (128.0 - half * 0.45, y + 4.5),
                (128.0, y + 2.5),
            ],
            rib,
            2.2,
        );
        line(
            pm,
            &[
                (128.0 + half, y),
                (128.0 + half * 0.45, y + 4.5),
                (128.0, y + 2.5),
            ],
            rib,
            2.2,
        );
    }
}

/// 胸口的裂缝：一条折来折去的竖缝，里面是魂火。
///
/// 这是整只兽唯一的"发光体"，也是 32 px 下最先被认出来的东西——
/// 所以它必须比周围亮一档以上，并且粗到缩一半还在。
fn furnace(pm: &mut Pixmap) {
    let crack = [
        (128.0f32, 104.0f32),
        (121.0, 118.0),
        (129.0, 130.0),
        (122.0, 142.0),
        (127.0, 152.0),
    ];
    glow(pm, 126.0, 128.0, 26.0, soul());
    stroke(pm, &crack, tint(soul_dim(), 0.95), 7.0);
    line(pm, &crack, soul(), 2.6);
    // 第二道：红的，从主缝分出岔，走到肋骨之间就断。
    line(
        pm,
        &[(126.0, 122.0), (113.0, 130.0)],
        tint(ember(), 0.85),
        1.8,
    );
    line(
        pm,
        &[(128.0, 134.0), (142.0, 140.0)],
        tint(ember(), 0.75),
        1.6,
    );
    glow(pm, 113.0, 130.0, 9.0, ember());
}

/// 肩甲：两块斜板，各两根**暗色**骨刺。
///
/// 刺是暗的、角才是亮的——第一版两边都用了亮骨色，头顶五根白刺挤在一起，
/// 远看像戴了顶王冠，不像影魔。亮色只留给角。
fn plates(pm: &mut Pixmap) {
    for side in [false, true] {
        let mut plate = vec![
            (100.0f32, 102.0f32),
            (86.0, 106.0),
            (76.0, 120.0),
            (82.0, 134.0),
            (96.0, 130.0),
            (103.0, 118.0),
        ];
        let mut facet = vec![(100.0, 102.0), (88.0, 108.0), (80.0, 120.0), (95.0, 116.0)];
        let mut spikes = vec![
            vec![(86.0f32, 106.0f32), (70.0, 100.0), (92.0, 114.0)],
            vec![(78.0, 118.0), (58.0, 116.0), (84.0, 128.0)],
        ];
        if side {
            plate = mirror(&plate);
            facet = mirror(&facet);
            spikes = spikes.iter().map(|s| mirror(s)).collect();
        }
        for spike in &spikes {
            fill(pm, spike, spike_color());
            line(pm, &[spike[0], spike[1]], tint(bone_dark(), 0.6), 1.5);
        }
        fill(pm, &plate, hide_dark());
        fill(pm, &facet, mix(hide_dark(), hide_lit(), 0.55));
        line(
            pm,
            &[plate[2], plate[3], plate[4]],
            tint(bone_dark(), 0.65),
            2.0,
        );
    }
}

/// 肩刺与第二对角的颜色：比 `hide_dark()` 亮、比 `bone_dark()` 暗的一档。
fn spike_color() -> Color {
    mix(hide_dark(), bone_dark(), 0.42)
}

/// 手臂：两条细长的，垂到腰侧，末端三根爪。
///
/// 比身体细很多——影魔是"大肩小肢"，手臂一粗就变成另一个角色
/// （恶魔战士而不是法师）。爪用骨色：它是下半身唯一的亮点。
fn arms(pm: &mut Pixmap) {
    for side in [false, true] {
        let mut nodes = vec![(94.0f32, 122.0f32), (86.0, 144.0), (84.0, 164.0)];
        let mut claws = vec![
            vec![(80.0f32, 164.0f32), (76.0, 182.0), (85.0, 167.0)],
            vec![(85.0, 167.0), (85.0, 186.0), (89.0, 167.0)],
            vec![(88.0, 165.0), (93.0, 180.0), (92.0, 163.0)],
        ];
        if side {
            nodes = mirror(&nodes);
            claws = claws.iter().map(|c| mirror(c)).collect();
        }
        let tones = [
            (hide_lit(), mix(hide(), hide_dark(), 0.5)),
            (hide(), hide_dark()),
        ];
        let widths = [8.5f32, 6.5, 4.5];
        ribbon(pm, &nodes, &widths, &tones);
        for claw in &claws {
            fill(pm, claw, bone_dark());
        }
    }
}

/// 头：宽的颚、窄的顶，正面朝前。
///
/// 和剑圣的六边形面具不同，这里用**倒梯形**（上面 112..144，下巴 106..150），
/// 于是脸颊两侧是斜的，远看像骷髅而不是头盔。
fn skull(pm: &mut Pixmap) {
    fill(
        pm,
        &[
            (110.0, 62.0),
            (146.0, 62.0),
            (154.0, 88.0),
            (144.0, 106.0),
            (112.0, 106.0),
            (102.0, 88.0),
        ],
        hide(),
    );
    fill(
        pm,
        &[
            (110.0, 64.0),
            (128.0, 64.0),
            (128.0, 104.0),
            (114.0, 104.0),
            (104.0, 86.0),
        ],
        hide_lit(),
    );
    fill(
        pm,
        &[
            (136.0, 64.0),
            (146.0, 64.0),
            (154.0, 88.0),
            (144.0, 104.0),
            (136.0, 104.0),
        ],
        hide_dark(),
    );
    // 眉骨：两道往里压的斜线，不画它额头会显得很呆。
    let brow = tint(mix(bone_dark(), soot(), 0.35), 0.9);
    line(pm, &[(106.0, 78.0), (124.0, 74.0)], brow, 2.6);
    line(pm, &[(150.0, 78.0), (132.0, 74.0)], brow, 2.6);
    // 颧骨到下颌的一刀折痕。
    line(
        pm,
        &[(106.0, 92.0), (116.0, 103.0)],
        tint(soot(), 0.45),
        1.6,
    );
    line(
        pm,
        &[(150.0, 92.0), (140.0, 103.0)],
        tint(soot(), 0.45),
        1.6,
    );
    // 嘴：一道锯开的横缝，两端各两颗尖牙。再多就糊了。
    line(
        pm,
        &[(118.0, 98.0), (126.0, 100.0), (132.0, 98.0), (140.0, 100.0)],
        soot(),
        2.4,
    );
    for (x, y) in [(120.0f32, 99.0f32), (136.0, 99.0)] {
        fill(pm, &[(x - 1.8, y), (x, y + 5.0), (x + 1.8, y)], bone());
    }
}

/// 眼睛：两道窄缝，内侧压低——"怒目"全靠这一条斜。
///
/// 第一版用的是圆点，缩到 32 px 变成两只大灯泡，像青蛙。
fn eyes(pm: &mut Pixmap) {
    for (x, inward) in [(120.0f32, 1.0f32), (136.0, -1.0)] {
        glow(pm, x, 88.0, 13.0, soul());
        let (xi, xo) = (x + inward * 5.6, x - inward * 5.6);
        fill(
            pm,
            &[(xi, 89.4), (xo, 86.0), (xo, 88.2), (xi, 91.0)],
            soul(),
        );
        fill(
            pm,
            &[
                (xi - inward * 1.6, 89.6),
                (xi - inward * 4.0, 87.2),
                (xi - inward * 3.6, 88.9),
            ],
            Color::from_rgba8(248, 255, 250, 235),
        );
    }
}

/// 角：一对大的向外挑，一对暗的贴着头顶往上。
///
/// 四根角是影魔最认得出的特征，所以给得比"合理比例"更大一圈；
/// 但第二对用暗色，只负责把角根填满，不抢戏。
fn horns(pm: &mut Pixmap) {
    let tones = [
        (bone(), bone_dark()),
        (bone(), bone_dark()),
        (bone_dark(), mix(bone_dark(), soot(), 0.45)),
        (mix(bone_dark(), soot(), 0.55), soot()),
    ];
    // 先向外、再向上：整根角是「先横后竖」的弯，不是一条斜线。
    // 第一版是斜着平挑出去的，远看像顶宽檐帽而不是角。
    let big = [
        (114.0f32, 82.0f32),
        (102.0, 72.0),
        (92.0, 56.0),
        (88.0, 38.0),
        (88.0, 28.0),
    ];
    let widths = [13.0f32, 10.0, 6.5, 3.0, 1.4];
    ribbon(pm, &big, &widths, &tones);
    ribbon(pm, &mirror(&big), &widths, &tones);
    // 头顶第二对暗角删掉了：两条在中间交汇，读成"第三根角"，反而更乱。
}

fn body() -> Pixmap {
    let mut pm = canvas();
    flames(&mut pm);
    wings(&mut pm);
    smoke(&mut pm);
    torso(&mut pm);
    furnace(&mut pm);
    plates(&mut pm);
    arms(&mut pm);
    horns(&mut pm);
    skull(&mut pm);
    eyes(&mut pm);
    flares(&mut pm);
    pm
}

/// 闭眼层：两片盖住眼睛的眼皮 + 一道折痕。
///
/// 这一层是**盖在眼睛上**淡入的（`paint::draw_skin_buddy` 里的 `LayerRole::Blink`），
/// 所以必须是实心的；只画线条的话睁着的魂火会一直透出来。
fn blink() -> Pixmap {
    let mut pm = canvas();
    for x in [120.0f32, 136.0] {
        fill(
            &mut pm,
            &[
                (x - 6.4, 78.6),
                (x + 6.4, 78.2),
                (x + 6.4, 92.0),
                (x - 6.4, 92.4),
            ],
            hide(),
        );
        line(
            &mut pm,
            &[(x - 5.6, 88.6), (x + 5.6, 88.2)],
            tint(soot(), 0.6),
            1.8,
        );
        line(
            &mut pm,
            &[(x - 5.0, 80.6), (x + 5.0, 80.2)],
            tint(mix(hide_lit(), bone_dark(), 0.4), 0.55),
            1.2,
        );
    }
    pm
}

/// 覆盖层：浮着的魂火与余烬。
///
/// 放在覆盖层而不是画进 body，是因为它们**不随眨眼变**、也不该参与主体的
/// 质心与外轮廓自检。
fn overlay() -> Pixmap {
    let mut pm = canvas();
    for (x, y, r, a, color) in [
        (72.0f32, 150.0f32, 2.6f32, 0.9f32, soul()),
        (188.0, 138.0, 2.2, 0.8, soul()),
        (94.0, 214.0, 1.8, 0.6, soul()),
        (170.0, 206.0, 1.6, 0.55, soul()),
        (198.0, 88.0, 1.5, 0.5, soul()),
        (62.0, 170.0, 1.8, 0.7, ember()),
        (194.0, 118.0, 1.5, 0.55, ember()),
    ] {
        glow(&mut pm, x, y, r * 2.0, color);
        fill(
            &mut pm,
            &[
                (x, y - r * 1.6),
                (x + r * 0.8, y),
                (x, y + r * 1.6),
                (x - r * 0.8, y),
            ],
            tint(color, a),
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
    let path = dir.join("shadowfiend-sizes.png");
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
        ("shadowfiend-alone.png", 0.0f32),
        ("shadowfiend-ring.png", 1.0),
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
    let path = dir_out.join("shadowfiend-blink.png");
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
        "shadowfiend: blob {BLOB_RATIO:.3} of side, silhouette {sil:.3} -> layer scale {scale:.3}"
    );

    write_png(&dir, "body.png", body(), 12_000);
    write_png(&dir, "blink.png", blink(), 80);
    write_png(&dir, "overlay.png", overlay(), 40);

    let manifest = format!(
        r#"# 影魔 —— 官方第七套皮肤
id = "zouwu-shadowfiend"
name = "影魔"
author = "zouwu"
version = "1.0.0"

[geometry]
radius = 56.0
breath_amount = 0.03

# 主体在 256 画布里占 0.6（半径 0.30），所以放大 1/0.6 ≈ 1.67 倍，`radius` 才
# 仍然表示"桌面上的主体半径"。这个数由 make_skin_shadowfiend.rs 按实测占比反推。
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
