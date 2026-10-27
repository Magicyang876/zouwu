//! 动作（环上每一颗气泡）的领域模型。
//!
//! M0 只提供"内置动作的静态描述 + 简笔图标"，不做真正的执行。
//! 真实执行走 `Action` trait（M3 起），外部接入走声明式 TOML（M4）。

use crate::theme::Rgba;

/// 简笔图标：M0 用几何图形代替图标资源，避免引入字体/图片管线。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// 截图：取景框 + 四角
    Frame,
    /// 翻译：两段平行字条
    Lines,
    /// 二维码：方块矩阵
    Grid,
    /// 剪贴板：夹子
    Clip,
    /// 取色：水滴
    Drop,
    /// 启动器：闪电
    Bolt,
    /// 番茄钟：表盘
    Clock,
    /// 更多：三点
    Dots,
}

#[derive(Clone, Copy, Debug)]
pub struct ActionItem {
    /// 稳定 id，用于配置引用、热键绑定、事件上报
    pub id: &'static str,
    /// i18n key（M1 接入 Fluent 后使用）
    pub label_key: &'static str,
    pub glyph: Glyph,
    /// 强调色覆盖（None = 用主题 glyph 色）
    pub tint: Option<Rgba>,
}

pub fn default_ring() -> Vec<ActionItem> {
    vec![
        ActionItem {
            id: "builtin.screenshot",
            label_key: "action.screenshot",
            glyph: Glyph::Frame,
            tint: None,
        },
        ActionItem {
            id: "builtin.translate",
            label_key: "action.translate",
            glyph: Glyph::Lines,
            tint: None,
        },
        ActionItem {
            id: "builtin.qrcode",
            label_key: "action.qrcode",
            glyph: Glyph::Grid,
            tint: None,
        },
        ActionItem {
            id: "builtin.clipboard",
            label_key: "action.clipboard",
            glyph: Glyph::Clip,
            tint: None,
        },
        ActionItem {
            id: "builtin.colorpicker",
            label_key: "action.colorpicker",
            glyph: Glyph::Drop,
            tint: None,
        },
        ActionItem {
            id: "builtin.launcher",
            label_key: "action.launcher",
            glyph: Glyph::Bolt,
            tint: None,
        },
        ActionItem {
            id: "builtin.pomodoro",
            label_key: "action.pomodoro",
            glyph: Glyph::Clock,
            tint: None,
        },
        ActionItem {
            id: "builtin.more",
            label_key: "action.more",
            glyph: Glyph::Dots,
            tint: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ring_matches_default_ring_capacity() {
        let items = default_ring();
        assert_eq!(items.len(), 8, "default ring should fit one bubble ring");
        let ids: std::collections::HashSet<_> = items.iter().map(|i| i.id).collect();
        assert_eq!(ids.len(), items.len(), "action ids must be unique");
    }
}
