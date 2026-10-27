//! 弹簧动画（半隐式欧拉，带固定子步长保证稳定）。
//!
//! 设计要点：每个可动元素持有一个 `Spring`，动画是"把 target 拨到 1 或 0"，
//! 而不是"跑一条时长固定的时间线"。这样：
//! * 天然支持打断（从当前 value/velocity 继续），不会跳变；
//! * 过冲（overshoot）是物理自然产生的，不用手调 back 曲线；
//! * 用 `is_settled()` 决定何时停帧 —— 空闲 0 开销的基础。

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    /// 刚度，越大越快。常用 120..=260
    pub stiffness: f32,
    /// 阻尼，越大越"不弹"。常用 18..=30（临界阻尼 = 2*sqrt(stiffness)）
    pub damping: f32,
}

impl Spring {
    pub const fn new(initial: f32, stiffness: f32, damping: f32) -> Self {
        Self {
            value: initial,
            velocity: 0.0,
            target: initial,
            stiffness,
            damping,
        }
    }

    /// 常用的"轻过冲"手感：stiffness 180 / damping 22。
    pub const fn soft(initial: f32) -> Self {
        Self::new(initial, 180.0, 22.0)
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// 直接跳到目标值（用于初始化、或 reduced-motion 模式）。
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 1e-3 && self.velocity.abs() < 1e-3
    }

    /// 推进一次物理积分。`dt` 单位为秒，内部按最大 1/240s 子步长切分。
    pub fn step(&mut self, dt: f32) {
        const MAX_STEP: f32 = 1.0 / 240.0;
        let mut remaining = dt.clamp(0.0, 0.1);
        while remaining > 0.0 {
            let h = remaining.min(MAX_STEP);
            let accel = -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
            remaining -= h;
        }
        // 收敛后吸附，避免无限微抖导致永不停帧。
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }
}

/// 平滑起止（0..1 之间），用于透明度等不想要过冲的量。
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges_to_target() {
        let mut s = Spring::soft(0.0);
        s.set_target(1.0);
        for _ in 0..180 {
            s.step(1.0 / 60.0);
        }
        assert!(s.is_settled(), "spring should settle, got {s:?}");
        assert!((s.value - 1.0).abs() < 1e-3);
    }

    #[test]
    fn spring_overshoots_softly() {
        let mut s = Spring::soft(0.0);
        s.set_target(1.0);
        let mut peak: f32 = 0.0;
        for _ in 0..120 {
            s.step(1.0 / 60.0);
            peak = peak.max(s.value);
        }
        assert!(peak > 1.0, "expected overshoot, peak = {peak}");
        assert!(peak < 1.25, "overshoot too large: {peak}");
    }

    #[test]
    fn spring_is_interruptible() {
        let mut s = Spring::soft(0.0);
        s.set_target(1.0);
        for _ in 0..10 {
            s.step(1.0 / 60.0);
        }
        let mid = s.value;
        s.set_target(0.0);
        // 反向时不应跳到 0
        assert!((s.value - mid).abs() < 1e-6);
    }

    #[test]
    fn smoothstep_endpoints() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert!((smoothstep(0.5) - 0.5).abs() < 1e-6);
    }
}
