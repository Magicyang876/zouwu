//! 气泡环（Bubble Ring）几何与命中判定。
//!
//! 形态决策见 `docs/decisions.md` ADR-003：
//! 每颗气泡是独立圆形，等角环绕小兽；命中用"圆心距离"精确判定。

use crate::geom::{Point, Rect};
use std::f32::consts::{PI, TAU};

pub const DEG: f32 = PI / 180.0;

/// 环的几何参数（来自 `Config::ring`）。
#[derive(Clone, Copy, Debug)]
pub struct RingConfig {
    /// 第一环半径（气泡圆心到小兽中心的距离）
    pub radius: f32,
    /// 气泡半径（不是直径）
    pub bubble_radius: f32,
    /// 基准角，-90° = 正上方
    pub orbit_start: f32,
    /// 超过多少颗气泡时溢出到第二环
    pub max_per_ring: usize,
    /// 第二环相对第一环的半径增量
    pub ring_gap: f32,
}

impl Default for RingConfig {
    fn default() -> Self {
        Self {
            radius: 112.0,
            bubble_radius: 24.0,
            orbit_start: -90.0 * DEG,
            max_per_ring: 8,
            ring_gap: 70.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Bubble {
    pub index: usize,
    /// 所在环序号（0 = 第一环）
    pub ring: usize,
    /// 最终角度（含基准角与整体旋转）
    pub angle: f32,
    pub center: Point,
    pub radius: f32,
    /// 轨道浮动相位（避免所有气泡同步"呼吸"）
    pub float_phase: f32,
}

impl RingConfig {
    /// 计算 `n` 颗气泡的布局。`rotation` 用于贴边时整环偏转。
    pub fn layout(&self, n: usize, center: Point, rotation: f32) -> Vec<Bubble> {
        if n == 0 {
            return Vec::new();
        }
        let per_ring = self.max_per_ring.max(1);
        let mut out = Vec::with_capacity(n);
        let mut remaining = n;
        let mut ring = 0usize;
        let mut index = 0usize;
        while remaining > 0 {
            let count = remaining.min(per_ring);
            let radius = self.radius + ring as f32 * self.ring_gap;
            let bubble_r = (self.bubble_radius - ring as f32 * 3.0).max(10.0);
            let start = self.orbit_start + rotation;
            for i in 0..count {
                let angle = start + i as f32 * TAU / count as f32;
                out.push(Bubble {
                    index,
                    ring,
                    angle,
                    center: Point::polar(center, angle, radius),
                    radius: bubble_r,
                    float_phase: index as f32 * 1.7,
                });
                index += 1;
            }
            remaining -= count;
            ring += 1;
        }
        out
    }

    /// 命中测试：返回最近的气泡索引。`tolerance` 用于放大命中区（默认 1.15）。
    pub fn hit_test(&self, bubbles: &[Bubble], p: Point, tolerance: f32) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for b in bubbles {
            let limit = b.radius * tolerance;
            let d = p.distance_sq(b.center);
            let closer = best.map_or(true, |(_, bd)| d < bd);
            if d <= limit * limit && closer {
                best = Some((b.index, d));
            }
        }
        best.map(|(i, _)| i)
    }
}

/// 贴边时求解"让所有气泡都留在可见区内"的最小旋转角。
///
/// 找不到可行解时返回 0（调用方应退化为压缩半径或折叠"更多"）。
pub fn solve_rotation(cfg: &RingConfig, center: Point, visible: Rect, n: usize) -> f32 {
    let fits = |rotation: f32| -> bool {
        let bubbles = cfg.layout(n, center, rotation);
        bubbles.iter().all(|b| {
            let m = b.radius + 4.0;
            b.center.x - m >= visible.x
                && b.center.x + m <= visible.right()
                && b.center.y - m >= visible.y
                && b.center.y + m <= visible.bottom()
        })
    };

    if fits(0.0) {
        return 0.0;
    }
    // 以 5° 为步长向两侧搜索，取绝对偏移最小者。
    for step in 1..=72 {
        let d = step as f32 * 5.0 * DEG;
        if fits(d) {
            return d;
        }
        if fits(-d) {
            return -d;
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RingConfig {
        RingConfig {
            radius: 112.0,
            bubble_radius: 24.0,
            orbit_start: -90.0 * DEG,
            max_per_ring: 8,
            ring_gap: 70.0,
        }
    }

    #[test]
    fn layout_places_first_bubble_on_top() {
        let b = cfg().layout(6, Point::new(100.0, 100.0), 0.0);
        assert_eq!(b.len(), 6);
        assert!((b[0].center.x - 100.0).abs() < 1e-3);
        assert!((b[0].center.y - (100.0 - 112.0)).abs() < 1e-3);
    }

    #[test]
    fn layout_spills_to_second_ring() {
        let b = cfg().layout(10, Point::new(0.0, 0.0), 0.0);
        assert_eq!(b.len(), 10);
        assert_eq!(b[8].ring, 1);
    }

    #[test]
    fn hit_test_picks_exact_bubble() {
        let c = cfg();
        let bubbles = c.layout(8, Point::new(100.0, 100.0), 0.0);
        let target = bubbles[3];
        assert_eq!(c.hit_test(&bubbles, target.center, 1.15), Some(3));
        // 圆心正中间（小兽身上）不命中任何气泡
        assert_eq!(c.hit_test(&bubbles, Point::new(100.0, 100.0), 1.15), None);
    }

    #[test]
    fn hit_test_keeps_previous_when_between() {
        let c = cfg();
        let bubbles = c.layout(8, Point::new(100.0, 100.0), 0.0);
        let a = bubbles[0].center;
        let b = bubbles[1].center;
        let mid = Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
        // 两颗气泡之间：命中判定交给调用方保号，这里应返回 None 或其一，
        // 但绝不 panic，且不应命中第三颗。
        let hit = c.hit_test(&bubbles, mid, 1.15);
        assert!(hit.is_none() || hit == Some(0) || hit == Some(1));
    }

    #[test]
    fn rotation_is_zero_when_room_enough() {
        let r = solve_rotation(
            &cfg(),
            Point::new(170.0, 170.0),
            Rect::new(0.0, 0.0, 340.0, 340.0),
            6,
        );
        assert_eq!(r, 0.0);
    }

    #[test]
    fn rotation_avoids_clipping() {
        // 扁视口：竖直方向放不下，需要整体偏转
        let r = solve_rotation(
            &cfg(),
            Point::new(170.0, 120.0),
            Rect::new(0.0, 0.0, 340.0, 240.0),
            4,
        );
        assert_ne!(r, 0.0, "expected a non-zero rotation");
        assert!(r.abs() <= 50.0 * DEG, "rotation too large: {}", r / DEG);
    }
}
