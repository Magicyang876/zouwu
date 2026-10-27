//! 基础几何类型（逻辑像素）。

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Point) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }

    pub fn distance_sq(self, other: Point) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }

    /// 从极坐标生成点：`angle` 以弧度计，屏幕坐标系（y 向下，顺时针为正）。
    pub fn polar(origin: Point, angle: f32, radius: f32) -> Self {
        Self::new(
            origin.x + radius * angle.cos(),
            origin.y + radius * angle.sin(),
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    pub fn inset(&self, d: f32) -> Self {
        Self::new(
            self.x + d,
            self.y + d,
            (self.w - 2.0 * d).max(0.0),
            (self.h - 2.0 * d).max(0.0),
        )
    }
}

impl Rect {
    pub fn area(&self) -> f32 {
        // 显式判零而不是 max(0.0)：`f32::max(-0.0, 0.0)` 会原样返回 **-0.0**，
        // 于是面积变成 -0.0 并一路传染到 `visible_ratio`（日志里显示成 "-0%"）。
        if self.w <= 0.0 || self.h <= 0.0 {
            return 0.0;
        }
        self.w * self.h
    }

    /// 两个矩形的交集；不相交时 `None`。
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    }
}

/// `rect` 落在这些显示器上的最大可见比例（0..=1）。
///
/// 拔掉一块外接屏之后，昨晚记下的坐标会落在"已经不存在的桌面"上：
/// 窗口还在、进程还活着、托盘还在，但用户什么都看不见——这是最难自查的故障之一。
pub fn visible_ratio(rect: Rect, monitors: &[Rect]) -> f32 {
    let area = rect.area();
    if area <= 0.0 {
        return 0.0;
    }
    let covered: f32 = monitors
        .iter()
        .filter_map(|m| rect.intersection(m).map(|i| i.area()))
        .sum();
    // 同样为了 -0.0：`clamp` 在区间内会原样返回自己，所以先归一化再夹。
    let ratio = covered / area;
    if ratio > 0.0 {
        ratio.min(1.0)
    } else {
        0.0
    }
}

/// "宠物位"：主屏右侧、垂直方向 55% 处（偏下一点，避开任务栏和屏幕中线）。
///
/// `monitors[0]` 当主屏用（调用方保证顺序），`size` 是窗口边长，
/// `margin` 是离屏幕边缘的留白。
pub fn home_slot(monitors: &[Rect], size: f32, margin: f32) -> Point {
    let screen = monitors
        .first()
        .copied()
        .unwrap_or(Rect::new(0.0, 0.0, 1920.0, 1080.0));
    Point::new(
        screen.x + screen.w - size - margin,
        screen.y + screen.h * 0.55 - size * 0.5,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn ratio_is_fully_visible_inside_a_monitor() {
        let monitors = [monitor(0.0, 0.0, 1920.0, 1080.0)];
        let rect = Rect::new(1500.0, 400.0, 340.0, 340.0);
        assert_eq!(visible_ratio(rect, &monitors), 1.0);
    }

    #[test]
    fn ratio_is_zero_when_the_monitor_is_gone() {
        // 昨晚在第二块屏上，今天拔了：只剩主屏 0..1920
        let monitors = [monitor(0.0, 0.0, 1920.0, 1080.0)];
        let rect = Rect::new(2400.0, 400.0, 340.0, 340.0);
        assert_eq!(visible_ratio(rect, &monitors), 0.0);
    }

    #[test]
    fn ratio_counts_partial_overlap_across_monitors() {
        let monitors = [
            monitor(0.0, 0.0, 1920.0, 1080.0),
            monitor(1920.0, 0.0, 1080.0, 1920.0),
        ];
        // 骑在两块屏中间：一半在主屏、一半在副屏 —— 两块诗口都看得见
        let rect = Rect::new(1750.0, 400.0, 340.0, 340.0);
        assert!((visible_ratio(rect, &monitors) - 1.0).abs() < 0.001);

        // 碰到主屏边缘的 100px：只看到 100/340
        let rect = Rect::new(1820.0, 400.0, 340.0, 340.0);
        let ratio = visible_ratio(rect, &[monitor(0.0, 0.0, 1920.0, 1080.0)]);
        assert!((ratio - 100.0 / 340.0).abs() < 0.01, "got {ratio}");
    }

    #[test]
    fn home_slot_sits_on_the_right_edge_of_the_primary() {
        let monitors = [monitor(0.0, 0.0, 1920.0, 1080.0)];
        let p = home_slot(&monitors, 340.0, 48.0);
        assert_eq!(p.x, 1920.0 - 340.0 - 48.0);
        assert!((p.y - (1080.0 * 0.55 - 170.0)).abs() < 0.01);
    }

    #[test]
    fn home_slot_on_a_second_monitor_only_uses_the_first() {
        // 主屏在右边（负坐标或偏移）：主页槽必须跟主屏走
        let monitors = [monitor(-1920.0, 0.0, 1920.0, 1080.0)];
        let p = home_slot(&monitors, 340.0, 48.0);
        assert_eq!(p.x, -1920.0 + 1920.0 - 340.0 - 48.0);
    }
}

#[cfg(test)]
mod zero_guards {
    use super::*;

    #[test]
    fn invisible_rect_reports_positive_zero() {
        let r = Rect::new(5000.0, 400.0, 680.0, 680.0);
        let monitors = [Rect::new(0.0, 0.0, 1920.0, 1080.0)];
        let v = visible_ratio(r, &monitors);
        assert_eq!(v, 0.0);
        // -0.0 会 == 0.0，所以上面的断言放它过去；但日志里会印成 "-0%"，
        // 用户看到的是一个莫名其妙的负数。
        assert!(!v.is_sign_negative(), "ratio must not be negative zero");
    }

    #[test]
    fn degenerate_rect_has_no_area() {
        let flat = Rect::new(10.0, 10.0, -0.0, 40.0);
        assert_eq!(flat.area(), 0.0);
        assert!(!flat.area().is_sign_negative());
    }
}
