//! 窗口拖动与「点击 vs 拖动」判定。
//!
//! 为什么要单独一个模块：这段逻辑必须能被**测试**。GUI 自动化在这台机器上
//! 不可靠（实测光标会被外部力量瞬移几十上百像素），所以判定规则用纯函数表达，
//! 让 `cargo test` 去覆盖边界，`poll_pointer` 只负责喂当前光标。
//!
//! 坐标约定：**屏幕物理像素**。分层窗口收不到鼠标事件，只能主动读 `GetCursorPos`，
//! 而 `GetCursorPos` 给的就是屏幕坐标；用它做锚点还能顺便解决「拖太快光标跑出窗口」。

/// 判定为「拖动」的位移平方阈值（4px）。
///
/// 手指/触控板按下去总会抖 1~2px，阈值太小会把每次点击都当成拖动；
/// 太大则让「想微调位置」的人觉得窗口粘住不动。4px 是 Windows 自己拖动
/// 图标的量级（`SM_CXDRAG` 通常为 4）。
pub const DRAG_THRESHOLD_SQ: i32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drag {
    /// 按下时的光标位置（屏幕物理像素）
    pub cursor: (i32, i32),
    /// 按下时的窗口左上角（屏幕物理像素）
    pub origin: (i32, i32),
    /// 是否已经越过阈值。一旦越过就**不再回退**：手抖回原位也不该变成点击。
    pub moved: bool,
}

/// 一次按下的结局。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragOutcome {
    /// 没动过：这是一次点击。
    Click,
    /// 动过：位置已经落在 `(x, y)`，该落盘了。
    Moved { x: i32, y: i32 },
}

impl Drag {
    /// 开始一次按下。`cursor` 与 `origin` 都必须来自同一时刻。
    pub fn begin(cursor: (i32, i32), origin: (i32, i32)) -> Self {
        Self {
            cursor,
            origin,
            moved: false,
        }
    }

    /// 拖动中：给出窗口应有的新位置；未越过阈值时返回 `None`（窗口不动）。
    pub fn track(&mut self, cursor: (i32, i32)) -> Option<(i32, i32)> {
        let dx = cursor.0 - self.cursor.0;
        let dy = cursor.1 - self.cursor.1;
        if !self.moved && dx * dx + dy * dy < DRAG_THRESHOLD_SQ {
            return None;
        }
        self.moved = true;
        Some((self.origin.0 + dx, self.origin.1 + dy))
    }

    /// 松手。用「随时记录的最新位置」而不是松手瞬间的光标：
    /// 松手那一刻光标可能已经被移走，而窗口应该停在最后一次跟随到的位置。
    /// 调用方应当把每帧跟踪到的位置写进 `last`。
    pub fn finish(&self, last: (i32, i32)) -> DragOutcome {
        if self.moved {
            DragOutcome::Moved {
                x: last.0,
                y: last.1,
            }
        } else {
            DragOutcome::Click
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_under_threshold_is_a_click() {
        let mut d = Drag::begin((100, 100), (10, 10));
        assert_eq!(d.track((101, 100)), None);
        assert_eq!(d.track((102, 102)), None); // d²=8 < 16
        assert_eq!(d.finish((10, 10)), DragOutcome::Click);
    }

    #[test]
    fn crossing_the_threshold_starts_moving() {
        let mut d = Drag::begin((100, 100), (10, 10));
        assert_eq!(d.track((104, 100)), Some((14, 10))); // d²=16，正好到阈值
        assert_eq!(d.track((110, 90)), Some((20, 0)));
    }

    #[test]
    fn moving_back_does_not_turn_into_a_click() {
        // 拖出去再拖回原处：用户显然是想搬东西，不是想点它。
        let mut d = Drag::begin((100, 100), (10, 10));
        assert!(d.track((160, 100)).is_some());
        assert_eq!(d.track((100, 100)), Some((10, 10)));
        assert_eq!(d.finish((10, 10)), DragOutcome::Moved { x: 10, y: 10 });
    }

    #[test]
    fn offset_is_relative_to_the_press_point_not_delta_per_frame() {
        // 每一帧都用「按下时的光标」做锚点：这样即便某一帧读数异常，
        // 下一帧到真实位置就会自己纠正回来，不会累积误差。
        let mut d = Drag::begin((500, 500), (0, 0));
        assert_eq!(d.track((520, 530)), Some((20, 30)));
        assert_eq!(d.track((450, 400)), Some((-50, -100)));
    }

    #[test]
    fn cursor_leaving_the_window_keeps_dragging() {
        // 拖得快时光标必然跑出窗口矩形；判定不能依赖「光标是否还在窗口内」。
        let mut d = Drag::begin((0, 0), (100, 100));
        assert_eq!(d.track((-400, -300)), Some((-300, -200)));
        assert_eq!(
            d.finish((-300, -200)),
            DragOutcome::Moved { x: -300, y: -200 }
        );
    }

    #[test]
    fn finish_reports_the_last_tracked_position() {
        let mut d = Drag::begin((0, 0), (100, 100));
        let pos = d.track((50, 50)).unwrap();
        // 松手时光标可能已经跳到别处；窗口应当停在 pos，而不是跟着跳。
        assert_eq!(d.finish(pos), DragOutcome::Moved { x: 150, y: 150 });
    }
}
