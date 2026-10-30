//! 子项「激光条」的几何（ADR-045）。
//!
//! 一颗自定义球可以挂若干子项。点开之后，子项不是再排一圈气泡，而是从这颗球
//! 身上**向外放射**几条激光条，每条末端挂一块横向文字牌——因为子项的名字是
//! 用户自己写的短句（"开 IDE"、"切到仓库"），要读得清就必须横着排、按文字
//! 宽度给够地方。所以这里的布局目标是：
//!
//! 1. 激光条朝"背离环心"的方向张开（球在外圈，条子往外射，不糊在本体和环上）；
//! 2. 文字牌之间**绝不重叠**，放宽一点也要错开（径向排队，见 `layout`）；
//! 3. 全部落在窗口里 —— 球贴着屏幕边时，牌子会被推回来而不是画到窗外。
//!
//! 只放几何，不放绘制与动画：条子的生长、文字牌的淡入由调用方按进度插值。

use crate::geom::{Point, Rect};
use std::f32::consts::PI;

/// 文字牌高度（逻辑像素）
pub const CHIP_H: f32 = 20.0;
/// 文字牌左右单侧内边距
pub const CHIP_PAD: f32 = 7.0;
/// 文字牌之间至少留的空隙
const CHIP_GAP: f32 = 5.0;
/// 文字牌最大宽度：再长的名字截断（截断由绘制层做，这里只要几何）
pub const CHIP_MAX_W: f32 = 168.0;
/// 激光条从球边起算的静空
const BEAM_GAP: f32 = 7.0;
/// 激光条本身的长度（不含文字牌）
const BEAM_LEN: f32 = 15.0;
/// 整扇张角上限
const MAX_FAN: f32 = 135.0 * PI / 180.0;
/// 每多一条子项再加的张角
const FAN_PER_BEAM: f32 = 45.0 * PI / 180.0;
/// 错不开时每次外推的距离
const PUSH_STEP: f32 = CHIP_H + CHIP_GAP;
/// 最多外推几轮（再多就认了，说明窗口太小或子项太多）
const PUSH_ROUNDS: usize = 8;

/// 一条激光条。
#[derive(Clone, Debug, PartialEq)]
pub struct Beam {
    /// 子项在父球 `children` 里的下标
    pub index: usize,
    /// 出射角（弧度）。以「父球圆心 → 文字牌中心」为方向，绘制层拿它画光。
    pub angle: f32,
    /// 起点：父球外沿（激光条从这里射出去）
    pub base: Point,
    /// 终点：文字牌中心（激光条画到牌子底下为止，牌子盖住收尾）
    pub tip: Point,
    /// 文字牌矩形（始终水平摆，不跟着激光条转）
    pub chip: Rect,
    /// 文字宽度（逻辑像素，已按 `CHIP_MAX_W` 截断过）
    pub text_width: f32,
    /// 牌子中心离**父球圆心**的自然距离（夹回窗口之前的值）。
    ///
    /// 调用方拿它算"要多大窗口才够放激光条"：按夹过的位置算会自我缩小，
    /// 窗口永远长不够。
    pub dist: f32,
}

/// 一次布局需要的输入。
pub struct BeamInput<'a> {
    /// 父球圆心
    pub parent: Point,
    pub parent_radius: f32,
    /// 环心：决定"哪边是外"
    pub center: Point,
    /// 窗口（逻辑坐标）
    pub viewport: Rect,
    /// 每条子项的文字
    pub labels: &'a [String],
}

/// 按文字宽度给子项排版。`measure` 返回某段文字在牌子字号下的宽度（逻辑像素）。
///
/// 返回顺序与 `labels` 一致；`labels` 为空时返回空。
pub fn layout(input: &BeamInput<'_>, measure: &mut dyn FnMut(&str) -> f32) -> Vec<Beam> {
    let n = input.labels.len();
    if n == 0 {
        return Vec::new();
    }

    // 1) 扇形张角：一条就直线，多了按数量张开（有上限，不然贴得满屏都是）。
    let fan = fan_angle(n);
    let base_angle = (input.parent.y - input.center.y).atan2(input.parent.x - input.center.x);

    // 牌子不许贴到窗口边：留一点静空，不然文字会像被切掉。
    let viewport = safe_inset(input.viewport, CHIP_GAP);

    // 2) 每条先按"文字多宽就给多远"摆出去，再逐条把撞上的往外推。
    let mut out: Vec<Beam> = Vec::with_capacity(n);
    for (i, label) in input.labels.iter().enumerate() {
        let angle = if n == 1 {
            base_angle
        } else {
            base_angle - fan * 0.5 + fan * i as f32 / (n - 1) as f32
        };
        let full = measure(label);
        let text_width = full.clamp(0.0, CHIP_MAX_W - CHIP_PAD * 2.0);
        let chip_w = text_width + CHIP_PAD * 2.0;

        // 牌子中心离父球圆心多远：球半径 + 静空 + 条长 + 半个牌子，
        // 这样激光条露在外面的长度是固定的，牌子宽窄只影响牌子自身占多少地方。
        let mut dist = input.parent_radius + BEAM_GAP + BEAM_LEN + chip_w * 0.5;
        let mut chip = chip_rect(Point::polar(input.parent, angle, dist), chip_w);

        for _ in 0..PUSH_ROUNDS {
            let cramped = out.iter().any(|b| overlaps(b.chip, chip, CHIP_GAP * 0.5));
            if !cramped {
                break;
            }
            dist += PUSH_STEP;
            chip = chip_rect(Point::polar(input.parent, angle, dist), chip_w);
        }

        // 3) 推回窗口：球贴着屏幕边时牌子不能画到窗外，宁可让条子短一点。
        out.push(Beam {
            index: i,
            angle,
            base: Point::polar(input.parent, angle, input.parent_radius + 1.5),
            tip: chip.center(),
            chip: clamp_into(chip, viewport),
            text_width,
            dist,
        });
    }

    // 4) 外推 + 夹回窗口之后仍可能撞上（球贴着屏幕顶最典型：所有牌子都被
    //    夹到同一行、横着叠在一起）。这时按"扇区"把它们顺着切线排开：
    //    朝外是上下方向的球，牌子就横着并排（像一把扇子）；朝外是左右方向的
    //    球，牌子就竖着排队。真排不开（窗口太小 / 子项太多）再退回纵向错行。
    if has_overlap(&out) {
        spread(&mut out, input.parent, input.center, viewport);
    }
    for b in out.iter_mut() {
        b.chip = clamp_into(b.chip, viewport);
        recompute(b, input.parent, input.parent_radius);
    }
    out
}

/// 两块牌子（各向外放宽半格）是否碰到一起。
fn has_overlap(beams: &[Beam]) -> bool {
    for (i, a) in beams.iter().enumerate() {
        for b in beams.iter().skip(i + 1) {
            if overlaps(a.chip, b.chip, CHIP_GAP * 0.5) {
                return true;
            }
        }
    }
    false
}

/// 顺着切线把牌子排开（扇区展开）。
///
/// 朝外是上下方向 → 牌子横排；朝外是左右方向 → 牌子竖排。排完还在窗口里才算
/// 成功，否则退回 [`relax_vertically`] 的错行保底。
fn spread(beams: &mut [Beam], parent: Point, center: Point, viewport: Rect) {
    let outward_y = (parent.y - center.y).abs() >= (parent.x - center.x).abs();
    let horizontal = outward_y && spread_along(beams, parent, viewport, true);
    if !horizontal && !spread_along(beams, parent, viewport, false) {
        relax_vertically(beams, parent, viewport);
    }
}

/// 沿一条轴（`horizontal = true` 则沿 x）把牌子首尾相接排开并居中在 `parent` 上。
/// 排不下（跑出窗口）返回 false，且不改动。
fn spread_along(beams: &mut [Beam], parent: Point, viewport: Rect, horizontal: bool) -> bool {
    let n = beams.len();
    if n == 0 {
        return true;
    }
    let size = |b: &Beam| if horizontal { b.chip.w } else { b.chip.h };
    let total: f32 = beams.iter().map(size).sum::<f32>() + CHIP_GAP * (n - 1) as f32;
    let anchor = if horizontal { parent.x } else { parent.y };
    let mut cursor = anchor - total * 0.5;

    // 按当前位置排序，保住"左到右 / 上到下"的阅读次序。
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| {
        let (pa, pb) = if horizontal {
            (beams[*a].chip.center().x, beams[*b].chip.center().x)
        } else {
            (beams[*a].chip.center().y, beams[*b].chip.center().y)
        };
        pa.total_cmp(&pb)
    });

    let mut placed: Vec<(usize, f32)> = Vec::with_capacity(n);
    for i in order {
        let start = cursor;
        cursor += size(&beams[i]) + CHIP_GAP;
        placed.push((i, start));
    }

    // 先试装，装不下就一个都不动
    for (i, start) in &placed {
        let chip = if horizontal {
            Rect::new(*start, beams[*i].chip.y, beams[*i].chip.w, beams[*i].chip.h)
        } else {
            Rect::new(beams[*i].chip.x, *start, beams[*i].chip.w, beams[*i].chip.h)
        };
        if chips_outside(chip, viewport) {
            return false;
        }
    }
    for (i, start) in placed {
        if horizontal {
            beams[i].chip.x = start;
        } else {
            beams[i].chip.y = start;
        }
    }
    true
}

fn chips_outside(chip: Rect, viewport: Rect) -> bool {
    chip.x < viewport.x - 0.01
        || chip.y < viewport.y - 0.01
        || chip.right() > viewport.right() + 0.01
        || chip.bottom() > viewport.bottom() + 0.01
}

/// 一条激光条的起点/角度按牌子的最终位置重算。
fn recompute(beam: &mut Beam, parent: Point, parent_radius: f32) {
    beam.tip = beam.chip.center();
    // 起点落在球沿该方向的外沿上：扇形张开时每条光从球的不同位置射出。
    beam.angle = (beam.tip.y - parent.y).atan2(beam.tip.x - parent.x);
    beam.base = Point::polar(parent, beam.angle, parent_radius + 1.5);
}

/// 错行保底：把互相重叠的牌子沿纵向推开（牌子本来是横排的，往下错一行
/// 比叠在一起看不清强，激光条斜过去接它并不难看）。
fn relax_vertically(beams: &mut [Beam], parent: Point, viewport: Rect) {
    for _ in 0..PUSH_ROUNDS {
        let mut moved = false;
        for i in 0..beams.len() {
            for j in (i + 1)..beams.len() {
                if !overlaps(beams[i].chip, beams[j].chip, CHIP_GAP * 0.5) {
                    continue;
                }
                let down = beams[i].chip.bottom() + CHIP_GAP - beams[j].chip.y;
                if beams[j].chip.y + down + beams[j].chip.h <= viewport.bottom() {
                    beams[j].chip.y += down;
                } else {
                    // 底下没地方了，往上让
                    beams[j].chip.y =
                        (beams[i].chip.y - CHIP_GAP - beams[j].chip.h).max(viewport.y);
                }
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    let _ = parent;
}

/// 扇形的总张角（弧度）。子项越多越开，但有上限。
pub fn fan_angle(n: usize) -> f32 {
    if n <= 1 {
        0.0
    } else {
        (FAN_PER_BEAM * (n - 1) as f32).min(MAX_FAN)
    }
}

/// 两块牌子（各向外放宽 `pad`）是否碰到一起。
fn overlaps(a: Rect, b: Rect, pad: f32) -> bool {
    a.inset(-pad).intersection(&b.inset(-pad)).is_some()
}

fn chip_rect(center: Point, width: f32) -> Rect {
    Rect::new(
        center.x - width * 0.5,
        center.y - CHIP_H * 0.5,
        width,
        CHIP_H,
    )
}

/// 向内收一点，但别把窗口收成负数（小窗口下 `Rect::inset` 会得到空矩形）。
fn safe_inset(rect: Rect, d: f32) -> Rect {
    if rect.w > d * 6.0 && rect.h > d * 6.0 {
        rect.inset(d)
    } else {
        rect
    }
}

/// 把矩形挪回容器里（容器比它还小就贴左上角，绘制层会自己截断）。
fn clamp_into(mut rect: Rect, viewport: Rect) -> Rect {
    let max_x = (viewport.right() - rect.w).max(viewport.x);
    let max_y = (viewport.bottom() - rect.h).max(viewport.y);
    rect.x = rect.x.clamp(viewport.x, max_x);
    rect.y = rect.y.clamp(viewport.y, max_y);
    rect
}

/// 命中：返回被点到的文字牌对应的子项下标（后画的在上，所以倒着找）。
pub fn hit(beams: &[Beam], p: Point, tolerance: f32) -> Option<usize> {
    beams
        .iter()
        .rev()
        .find(|b| b.chip.inset(-tolerance).contains(p))
        .map(|b| b.index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Rect {
        Rect::new(0.0, 0.0, 600.0, 400.0)
    }

    /// 等宽假字体：一个字符 7px。真字体由 platform 的 GDI 度量提供。
    fn measure(text: &str) -> f32 {
        text.chars().count() as f32 * 7.0
    }

    fn layout_at(parent: Point, labels: &[&str]) -> Vec<Beam> {
        let labels: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
        layout(
            &BeamInput {
                parent,
                parent_radius: 24.0,
                center: Point::new(300.0, 200.0),
                viewport: viewport(),
                labels: &labels,
            },
            &mut |t| measure(t),
        )
    }

    #[test]
    fn a_single_child_shoots_straight_out() {
        // 父球在环心正上方 → 朝上（-90°）
        let beams = layout_at(Point::new(300.0, 88.0), &["开 IDE"]);
        assert_eq!(beams.len(), 1);
        let angle = beams[0].angle.to_degrees();
        assert!((angle + 90.0).abs() < 1.0, "应当朝正上，实际 {angle}°");
        assert!(beams[0].tip.y < beams[0].base.y, "牌子应当在球的上方");
    }

    #[test]
    fn more_children_fan_wider_but_stay_bounded() {
        // 张角本身（进布局前的扇形）随数量变大、但有上限
        assert_eq!(fan_angle(0), 0.0);
        assert_eq!(fan_angle(1), 0.0);
        assert!(fan_angle(3) > fan_angle(2), "子项多了要张开");
        assert!(
            fan_angle(99) <= MAX_FAN + 1e-4,
            "张角要有上限，实际 {}°",
            fan_angle(99).to_degrees()
        );

        // 落到窗口里之后，每条光仍然分得开（不会被挤成同一个方向）
        let five = layout_at(Point::new(300.0, 88.0), &["a", "b", "c", "d", "e"]);
        assert_eq!(five.len(), 5);
        for (i, a) in five.iter().enumerate() {
            for b in five.iter().skip(i + 1) {
                assert_ne!(
                    a.chip.center().x.to_bits(),
                    b.chip.center().x.to_bits(),
                    "两条牌子坐在同一个 x 上，说明根本没分扇形"
                );
            }
        }
    }

    #[test]
    fn chips_never_overlap() {
        // 名字长短差很多，最容易撞；放宽 1px 判重叠，留一点浮点余量。
        let beams = layout_at(
            Point::new(300.0, 88.0),
            &["开 IDE", "切到仓库", "重启服务", "a", "打开终端"],
        );
        for (i, a) in beams.iter().enumerate() {
            for b in beams.iter().skip(i + 1) {
                assert!(
                    !overlaps(a.chip, b.chip, 0.5),
                    "牌子重叠了：{:?} 与 {:?}",
                    a.chip,
                    b.chip
                );
            }
        }
    }

    #[test]
    fn wider_text_pushes_the_chip_further_out() {
        let short = layout_at(Point::new(300.0, 88.0), &["ab"]);
        let long = layout_at(Point::new(300.0, 88.0), &["abababababababababab"]);
        let d_short = short[0].tip.distance(Point::new(300.0, 88.0));
        let d_long = long[0].tip.distance(Point::new(300.0, 88.0));
        assert!(
            d_long > d_short,
            "文字更长的牌子要摆得更远（{d_long} vs {d_short}）"
        );
        assert!(long[0].text_width > short[0].text_width);
    }

    #[test]
    fn text_width_is_capped() {
        let huge = "字".repeat(120);
        let beams = layout_at(Point::new(300.0, 88.0), &[huge.as_str()]);
        assert!(
            beams[0].text_width <= CHIP_MAX_W - CHIP_PAD * 2.0 + 0.01,
            "超长名字要截断，不然牌子顶出屏幕"
        );
    }

    #[test]
    fn chips_are_kept_inside_the_viewport() {
        // 父球贴右上角：朝外的条子会把牌子推到窗口外
        let beams = layout_at(Point::new(585.0, 15.0), &["很长的一个子项名字", "另一个"]);
        for b in &beams {
            assert!(
                b.chip.x >= viewport().x - 0.01
                    && b.chip.y >= viewport().y - 0.01
                    && b.chip.right() <= viewport().right() + 0.01
                    && b.chip.bottom() <= viewport().bottom() + 0.01,
                "牌子跑出窗口了：{:?}",
                b.chip
            );
        }
    }

    #[test]
    fn hit_finds_the_chip_under_the_pointer() {
        let beams = layout_at(Point::new(300.0, 88.0), &["a", "b"]);
        for b in &beams {
            assert_eq!(hit(&beams, b.chip.center(), 0.0), Some(b.index));
        }
        assert_eq!(hit(&beams, Point::new(5.0, 395.0), 0.0), None);
    }

    #[test]
    fn no_children_no_beams() {
        assert!(layout_at(Point::new(300.0, 88.0), &[]).is_empty());
    }
}
