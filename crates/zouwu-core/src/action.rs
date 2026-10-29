//! 动作（环上每一颗气泡）的领域模型。
//!
//! 环上有什么、按什么次序，全部来自配置（`config.ring.items`，ADR-042）：
//! 这里只负责"把配置清单翻译成可以画的球"。
//!
//! `BUILTINS` **只列真的接了线的功能**。截图/翻译/取色/启动器/番茄钟/更多
//! 这些历史遗留的占位早就从清单里撤掉了——那时候点下去只会写一行
//! "action not implemented yet" 的日志，比没有更糟。

use crate::config::RingItem;
use crate::i18n::I18n;
use crate::theme::Rgba;

/// 简笔图标：不引入字体/图片管线，用几何图形代替图标资源。
/// 自定义球配了图标文件（`RingItem::icon`）时，绘制层会用图替换它。
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
    /// 启动器 / 自定义命令：闪电
    Bolt,
    /// 番茄钟：表盘
    Clock,
    /// 更多：三点
    Dots,
}

/// 扫屏幕二维码的动作 id。
pub const QRCODE_ID: &str = "builtin.qrcode";
/// 打开剪贴板历史的动作 id。
pub const CLIPBOARD_ID: &str = "builtin.clipboard";

/// 一个内置功能的静态描述。
pub struct Builtin {
    /// 稳定 id，出现在配置与事件日志里
    pub id: &'static str,
    /// i18n key（`action.<name>`）
    pub label_key: &'static str,
    pub glyph: Glyph,
    /// 新配置里默认给它的热键（空 = 默认不给）
    pub default_hotkey: &'static str,
}

/// 内置功能清单。**只有实现好的功能才配待在这里。**
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        id: QRCODE_ID,
        label_key: "action.qrcode",
        glyph: Glyph::Grid,
        default_hotkey: "Ctrl+Alt+Q",
    },
    Builtin {
        id: CLIPBOARD_ID,
        label_key: "action.clipboard",
        glyph: Glyph::Clip,
        default_hotkey: "Ctrl+Alt+V",
    },
];

pub fn builtin(id: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.id == id)
}

pub fn default_hotkey(id: &str) -> &'static str {
    builtin(id).map(|b| b.default_hotkey).unwrap_or("")
}

/// 一颗"已经解析好、可以直接画"的球。
///
/// 内置球的名字来自 i18n，自定义球的名字来自配置；两者都在 [`resolve`] 里定下来，
/// 所以绘制层不需要知道语言或配置长什么样。
#[derive(Clone, Debug, PartialEq)]
pub struct ActionItem {
    /// 稳定 id（内置 `builtin.*`，自定义 `custom.N`）
    pub id: String,
    /// 显示名字（悬浮提示、无障碍、日志用）
    pub label: String,
    pub glyph: Glyph,
    /// 自定义球要运行的东西；内置球为 `None`
    pub command: Option<String>,
    /// 自定义球的图标文件路径；`None` = 用 `glyph`
    pub icon: Option<String>,
    /// 强调色覆盖（None = 用主题 glyph 色）
    pub tint: Option<Rgba>,
}

impl ActionItem {
    /// 是不是自定义球（用 `command` 判断，id 只是标识）。
    pub fn is_custom(&self) -> bool {
        self.command.is_some()
    }
}

/// 把配置清单解析成环上真正要画的球。
///
/// 只包含 `enabled` 的条目，**顺序严格按配置顺序**——面板的上移/下移改的就是
/// 这个顺序，所以"按次序展开"是自然的：`expand_stagger_ms` 按索引错峰。
pub fn resolve(items: &[RingItem], i18n: &I18n) -> Vec<ActionItem> {
    items
        .iter()
        .filter(|item| item.enabled)
        .map(|item| match builtin(&item.id) {
            Some(known) => ActionItem {
                id: item.id.clone(),
                label: i18n.t(known.label_key),
                glyph: known.glyph,
                command: None,
                icon: None,
                tint: None,
            },
            // 认不出来的 id：当自定义球处理，有命令就能点，没命令点下去只写日志。
            None => ActionItem {
                id: item.id.clone(),
                label: item.display_name().to_string(),
                glyph: Glyph::Bolt,
                command: Some(item.command.clone()),
                icon: (!item.icon.is_empty()).then(|| item.icon.clone()),
                tint: None,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_ring_items;

    fn i18n() -> I18n {
        I18n::new("zh-CN")
    }

    #[test]
    fn builtin_ids_are_unique_and_implemented() {
        let ids: std::collections::HashSet<_> = BUILTINS.iter().map(|b| b.id).collect();
        assert_eq!(ids.len(), BUILTINS.len(), "内置功能 id 必须唯一");
        assert!(builtin(QRCODE_ID).is_some());
        assert!(builtin(CLIPBOARD_ID).is_some());
        // 没实现的功能不许混进清单（ADR-042）
        assert!(builtin("builtin.screenshot").is_none());
        assert!(builtin("builtin.pomodoro").is_none());
    }

    #[test]
    fn resolve_keeps_order_and_skips_disabled() {
        let items = default_ring_items();
        let ring = resolve(&items, &i18n());
        assert_eq!(ring.len(), 2);
        assert_eq!(ring[0].id, QRCODE_ID);
        assert_eq!(ring[0].label, "扫码", "内置球的名字应该走 i18n");
        assert_eq!(ring[1].id, CLIPBOARD_ID);
        assert!(!ring[0].is_custom());

        // 顺序就是配置顺序：把两颗球对调，环上也对调
        let mut swapped = items.clone();
        swapped.reverse();
        let ring = resolve(&swapped, &i18n());
        assert_eq!(ring[0].id, CLIPBOARD_ID);
        assert_eq!(ring[1].id, QRCODE_ID);

        // 关掉的那颗不上环
        let mut only_one = items.clone();
        only_one[0].enabled = false;
        let ring = resolve(&only_one, &i18n());
        assert_eq!(ring.len(), 1);
        assert_eq!(ring[0].id, CLIPBOARD_ID);
    }

    #[test]
    fn custom_items_fall_back_to_id_and_bolt_glyph() {
        let items = vec![RingItem::custom("custom.1")];
        let ring = resolve(&items, &i18n());
        assert_eq!(ring.len(), 1);
        assert_eq!(ring[0].label, "custom.1", "没填名字时显示 id，别显示空行");
        assert_eq!(ring[0].glyph, Glyph::Bolt);
        assert!(ring[0].is_custom());
        assert!(ring[0].icon.is_none());

        let mut named = items.clone();
        named[0].label = " 终端 ".to_string();
        named[0].icon = "C:\\icons\\term.png".to_string();
        let ring = resolve(&named, &i18n());
        assert_eq!(ring[0].label, "终端");
        assert_eq!(ring[0].icon.as_deref(), Some("C:\\icons\\term.png"));
    }
}
