//! 设置面板：模型 + 布局 + 绘制 + 命中。
//!
//! 面板刻意做成**一张表格**（左边标签、右边控件），而不是卡片式布局：
//! 这类工具的设置项都是"一个开关/一个数字"，表格最省视线移动，
//! 也能让键盘导航（↑↓ 选行、←→ 调值）自然成立。
//!
//! 三个设计决定：
//! 1. **没有"保存"按钮**。改动立即写回 `config.toml`，因为配置本来就支持热重载，
//!    多一个"保存/取消"只会多一条用户可能忘记的路。面板底部明说这一点。
//! 2. **布局与绘制分离**。`layout()` 产出带坐标的行，`draw()` 与 `hit()` 都消费它，
//!    所以"看到的位置"和"点到的位置"不可能不一致（这类 bug 在立即模式 UI 里很常见）。
//! 3. **区间外不画也不命中**。滚动带用 `Painter::set_clip` 裁掉，命中用
//!    `content.contains()` 先挡一道。

use zouwu_core::action::builtin;
use zouwu_core::config::{ChangeKind, Config, LogCfg, RingCfg, RingItem};
use zouwu_core::geom::Point;
use zouwu_core::hotkey::{Hotkey, HotkeyError};
use zouwu_core::i18n::{I18n, AUTO, LANGUAGES};
use zouwu_core::theme::{Rgba, Theme};

use crate::skin::Skin;
use crate::thumb::{self, Thumb};
use crate::widget::{Painter, Rect, Weight};
use zouwu_core::skin::PROCEDURAL_SKIN_ID;

/// 面板的逻辑初始尺寸。宽度刻意刚好放下"长标签 + 控件"两列。
pub const DEFAULT_SIZE: (f32, f32) = (760.0, 600.0);
pub const MIN_SIZE: (f32, f32) = (560.0, 420.0);

const MARGIN: f32 = 24.0;
/// 顶部标签页带的高度。
///
/// 这里**不画标题**：窗口标题栏已经写着「驺吾设置」，面板里面再写一遍
/// 等于同一句话说两次，且白占掉 40px 垂直空间。整个 header 就是一条标签页带。
const HEADER_H: f32 = 52.0;
const FOOTER_H: f32 = 38.0;
const ROW_H: f32 = 46.0;
/// 带提示的行要多高。
///
/// 以前提示行也按 `ROW_H` 给高度，于是 14pt 的标签和 11pt 的提示挤在 46px 里：
/// 两行底色（25px + 15px）加起来超过行高，提示字顶到了标签字的下缘，
/// 看起来像被「遮盖」。多给 18px，两行之间就有了真正的行距。
const HINT_ROW_EXTRA: f32 = 18.0;
/// 标签与提示之间的间隙。
const HINT_GAP: f32 = 2.0;
/// 展开说明的行距（逻辑像素）。
///
/// 18 而不是 `line_height(11.0)`：行高在布局阶段拿不到画笔，而 11pt 的行高
/// 在 100%/150% DPI 下折成逻辑像素分别是 15/14.7 —— 都小于 18，所以固定 18
/// 在两种缩放下都不会把两行压在一起，且不依赖任何运行时度量。
const NOTE_LINE_H: f32 = 18.0;
/// 说明块上下各留一点气口，否则第一行会贴着按钮。
const NOTE_PAD: f32 = 7.0;
/// section 标题占的高度：上方留白 + 一行小字 + 一条分隔线。
///
/// 原来是 34，标题压在行底、分隔线紧跟其后，紧贴上一个控件行，
/// 视觉上像"标题被上一行吃掉了一半"。
const SECTION_H: f32 = 44.0;
/// section 标题上方额外留白（和上一个控件行拉开距离）。
const SECTION_TOP_GAP: f32 = 10.0;
/// 分隔线距 section 底边的距离。
const SECTION_RULE_INSET: f32 = 8.0;
const TEXT_ROW_H: f32 = 34.0;
const CONTROL_W: f32 = 210.0;
const BUTTON_W: f32 = 170.0;

// 气泡球行里复合子控件的宽度（从右往左：开关 / 热键框 / ↓ / ↑）。
const RING_TOGGLE_W: f32 = 46.0;
const RING_HOTKEY_W: f32 = 150.0;
const RING_ARROW_W: f32 = 26.0;
/// 缩略图格子的间距。
const GALLERY_GAP: f32 = 12.0;
/// 一个缩略图格子（含底下的名字）的高度。
const GALLERY_CELL_H: f32 = 104.0;
/// 缩略图墙脚那一行说明文字预留的高度。
const GALLERY_HINT_H: f32 = 20.0;
/// 名义列数：只用来估算"要几行"，真实列数在 `gallery_geom` 里按宽度算。
/// 取 5 是因为默认 760 宽的窗口扣掉边距正好放 5 个 132px 的格子。
const GALLERY_COLS_NOMINAL: usize = 5;
/// 缩略图墙最多显示几行。更多皮肤请去托盘菜单选：
/// 一个设置页里堆三行以上的皮肤墙，没人会往下滚。
const GALLERY_MAX_ROWS: usize = 2;

// ── 标签页 ────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    General,
    /// 环上有什么球、什么次序、各自的热键（ADR-042）。
    Bubbles,
    Interaction,
    Appearance,
    Skins,
    About,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::General,
        Tab::Bubbles,
        Tab::Interaction,
        Tab::Appearance,
        Tab::Skins,
        Tab::About,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Tab::General => "settings.tab.general",
            Tab::Bubbles => "settings.tab.bubbles",
            Tab::Interaction => "settings.tab.interaction",
            Tab::Appearance => "settings.tab.appearance",
            Tab::Skins => "settings.tab.skins",
            Tab::About => "settings.tab.about",
        }
    }
}

// ── 可编辑字段 ────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Language,
    Hotkey,
    StartOnBoot,
    ShowOnStart,
    BringToFront,
    AlwaysOnTop,
    WindowSize,
    HoverDelay,
    CollapseDelay,
    ExpandOnClick,
    ExpandOnSummon,
    HoverFactor,
    CollapseFactor,
    HitTolerance,
    BuddyRadius,
    Breathe,
    RingRadius,
    OrbitStart,
    BubbleRadius,
    MaxPerRing,
    RingGap,
    FloatAmplitude,
    ExpandStagger,
    Theme,
    Skin,
    FontPack,
    ClipboardEnabled,
    ClipboardLimit,
    EyeCareEnabled,
    EyeCareStrength,
    AltDragEnabled,
    LogLevel,
    LogToFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Ms,
    Px,
    Percent,
    Count,
    /// 角度：设置面板里直接用 `°`，配置文件里存的是度数（不是弧度）。
    Deg,
    /// 气泡个数：单独一个单位，因为「8 条」读起来像剪贴板条数。
    Bubbles,
}

#[derive(Clone, Copy, Debug)]
struct NumSpec {
    min: i64,
    max: i64,
    step: i64,
    unit: Unit,
}

impl Field {
    fn label_key(self) -> &'static str {
        match self {
            Field::Language => "settings.language",
            Field::Hotkey => "settings.hotkey",
            Field::StartOnBoot => "settings.start-on-boot",
            Field::ShowOnStart => "settings.show-on-start",
            Field::BringToFront => "settings.bring-to-front",
            Field::AlwaysOnTop => "settings.always-on-top",
            Field::WindowSize => "settings.window-size",
            Field::HoverDelay => "settings.hover-delay",
            Field::CollapseDelay => "settings.collapse-delay",
            Field::ExpandOnClick => "settings.expand-on-click",
            Field::ExpandOnSummon => "settings.expand-on-summon",
            Field::HoverFactor => "settings.hover-factor",
            Field::CollapseFactor => "settings.collapse-factor",
            Field::HitTolerance => "settings.hit-tolerance",
            Field::BuddyRadius => "settings.buddy-radius",
            Field::Breathe => "settings.breathe",
            Field::RingRadius => "settings.ring-radius",
            Field::OrbitStart => "settings.orbit-start",
            Field::BubbleRadius => "settings.bubble-radius",
            Field::MaxPerRing => "settings.max-per-ring",
            Field::RingGap => "settings.ring-gap",
            Field::FloatAmplitude => "settings.float-amplitude",
            Field::ExpandStagger => "settings.expand-stagger",
            Field::Theme => "settings.theme",
            Field::Skin => "settings.skin",
            Field::FontPack => "settings.font",
            Field::ClipboardEnabled => "settings.clipboard",
            Field::ClipboardLimit => "settings.clipboard.limit",
            Field::EyeCareEnabled => "settings.eye-care",
            Field::EyeCareStrength => "settings.eye-care.strength",
            Field::AltDragEnabled => "settings.alt-drag",
            Field::LogLevel => "settings.log-level",
            Field::LogToFile => "settings.log-to-file",
        }
    }

    fn num_spec(self) -> Option<NumSpec> {
        let spec = match self {
            // 0 保留成"立即"，所以下限是 0 而不是 50。
            Field::HoverDelay => NumSpec {
                min: 0,
                max: 1000,
                step: 50,
                unit: Unit::Ms,
            },
            Field::CollapseDelay => NumSpec {
                min: 0,
                max: 1500,
                step: 50,
                unit: Unit::Ms,
            },
            Field::WindowSize => NumSpec {
                min: 220,
                max: 520,
                step: 20,
                unit: Unit::Px,
            },
            Field::ClipboardLimit => NumSpec {
                // 下限 10：历史要么有点用，要么干脆关掉；
                // 跳一个 10 条的步子，500 条也只需按几十下。
                min: 10,
                max: zouwu_core::history::LIMIT_MAX as i64,
                step: 10,
                unit: Unit::Count,
            },
            Field::HoverFactor => NumSpec {
                min: 100,
                max: 250,
                step: 5,
                unit: Unit::Percent,
            },
            Field::EyeCareStrength => NumSpec {
                // 下限 5：配置里 0 等于“什么都没涂”，不如让用户直接关开关。
                min: 5,
                max: zouwu_core::config::EyeCare::MAX_STRENGTH as i64,
                step: 5,
                unit: Unit::Percent,
            },
            Field::CollapseFactor => NumSpec {
                min: 100,
                max: 250,
                step: 5,
                unit: Unit::Percent,
            },
            Field::HitTolerance => NumSpec {
                min: 100,
                max: 200,
                step: 5,
                unit: Unit::Percent,
            },
            Field::BuddyRadius => NumSpec {
                min: 28,
                max: 110,
                step: 2,
                unit: Unit::Px,
            },
            Field::RingRadius => NumSpec {
                min: 60,
                max: 260,
                step: 4,
                unit: Unit::Px,
            },
            Field::BubbleRadius => NumSpec {
                min: 16,
                max: 64,
                step: 2,
                unit: Unit::Px,
            },
            Field::OrbitStart => NumSpec {
                min: -180,
                max: 180,
                step: 15,
                unit: Unit::Deg,
            },
            Field::MaxPerRing => NumSpec {
                min: 3,
                max: 16,
                step: 1,
                unit: Unit::Bubbles,
            },
            Field::RingGap => NumSpec {
                min: 0,
                max: 200,
                step: 4,
                unit: Unit::Px,
            },
            Field::FloatAmplitude => NumSpec {
                min: 0,
                max: 10,
                step: 1,
                unit: Unit::Px,
            },
            Field::ExpandStagger => NumSpec {
                min: 0,
                max: 120,
                step: 4,
                unit: Unit::Ms,
            },
            _ => return None,
        };
        Some(spec)
    }

    fn num(self, cfg: &Config) -> Option<i64> {
        let v = match self {
            Field::HoverDelay => cfg.interaction.hover_delay_ms as i64,
            Field::CollapseDelay => cfg.interaction.collapse_delay_ms as i64,
            Field::WindowSize => cfg.window.size.round() as i64,
            Field::ClipboardLimit => cfg.clipboard.limit as i64,
            Field::EyeCareStrength => cfg.eye_care.strength as i64,
            Field::HoverFactor => (cfg.interaction.hover_radius_factor * 100.0).round() as i64,
            Field::CollapseFactor => {
                (cfg.interaction.collapse_radius_factor * 100.0).round() as i64
            }
            Field::HitTolerance => (cfg.interaction.hit_tolerance * 100.0).round() as i64,
            Field::BuddyRadius => cfg.buddy.radius.round() as i64,
            Field::RingRadius => cfg.ring.radius.round() as i64,
            Field::OrbitStart => cfg.ring.orbit_start_deg.round() as i64,
            Field::BubbleRadius => cfg.ring.bubble_radius.round() as i64,
            Field::MaxPerRing => cfg.ring.max_per_ring as i64,
            Field::RingGap => cfg.ring.ring_gap.round() as i64,
            Field::FloatAmplitude => cfg.ring.float_amplitude.round() as i64,
            Field::ExpandStagger => cfg.ring.expand_stagger_ms as i64,
            _ => return None,
        };
        Some(v)
    }

    fn set_num(self, cfg: &mut Config, v: i64) {
        match self {
            Field::HoverDelay => cfg.interaction.hover_delay_ms = v.max(0) as u64,
            Field::CollapseDelay => cfg.interaction.collapse_delay_ms = v.max(0) as u64,
            Field::WindowSize => cfg.window.size = v as f32,
            Field::ClipboardLimit => cfg.clipboard.limit = v.max(1) as usize,
            Field::EyeCareStrength => {
                cfg.eye_care.strength =
                    v.clamp(0, zouwu_core::config::EyeCare::MAX_STRENGTH as i64) as u8
            }
            Field::HoverFactor => cfg.interaction.hover_radius_factor = v as f32 / 100.0,
            Field::CollapseFactor => cfg.interaction.collapse_radius_factor = v as f32 / 100.0,
            Field::HitTolerance => cfg.interaction.hit_tolerance = v as f32 / 100.0,
            Field::BuddyRadius => cfg.buddy.radius = v as f32,
            Field::RingRadius => cfg.ring.radius = v as f32,
            Field::OrbitStart => cfg.ring.orbit_start_deg = v as f32,
            Field::BubbleRadius => cfg.ring.bubble_radius = v as f32,
            Field::MaxPerRing => cfg.ring.max_per_ring = v.max(1) as usize,
            Field::RingGap => cfg.ring.ring_gap = v.max(0) as f32,
            Field::FloatAmplitude => cfg.ring.float_amplitude = v as f32,
            Field::ExpandStagger => cfg.ring.expand_stagger_ms = v.max(0) as u64,
            _ => {}
        }
    }

    fn bool(self, cfg: &Config) -> Option<bool> {
        let v = match self {
            Field::StartOnBoot => cfg.general.start_on_boot,
            Field::ShowOnStart => cfg.general.show_on_start,
            Field::BringToFront => cfg.general.bring_to_front,
            Field::AlwaysOnTop => cfg.window.always_on_top,
            Field::AltDragEnabled => cfg.alt_drag.enabled,
            Field::ExpandOnClick => cfg.interaction.expand_on_click,
            Field::ExpandOnSummon => cfg.interaction.expand_on_summon,
            Field::ClipboardEnabled => cfg.clipboard.enabled,
            Field::EyeCareEnabled => cfg.eye_care.enabled,
            Field::Breathe => cfg.buddy.breathe,
            Field::LogToFile => cfg.log.to_file,
            _ => return None,
        };
        Some(v)
    }

    fn set_bool(self, cfg: &mut Config, on: bool) {
        match self {
            Field::StartOnBoot => cfg.general.start_on_boot = on,
            Field::ShowOnStart => cfg.general.show_on_start = on,
            Field::BringToFront => cfg.general.bring_to_front = on,
            Field::AlwaysOnTop => cfg.window.always_on_top = on,
            Field::AltDragEnabled => cfg.alt_drag.enabled = on,
            Field::ExpandOnClick => cfg.interaction.expand_on_click = on,
            Field::ExpandOnSummon => cfg.interaction.expand_on_summon = on,
            Field::ClipboardEnabled => cfg.clipboard.enabled = on,
            Field::EyeCareEnabled => cfg.eye_care.enabled = on,
            Field::Breathe => cfg.buddy.breathe = on,
            Field::LogToFile => cfg.log.to_file = on,
            _ => {}
        }
    }
}

// ── 行模型 ────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum Control {
    Toggle {
        on: bool,
    },
    /// 数值步进：显示文本已经格式化好（含单位）。
    Step {
        text: String,
        at_min: bool,
        at_max: bool,
    },
    /// 枚举选择：`‹ 显示文本 ›`
    Choice {
        text: String,
    },
    Hotkey {
        text: String,
        capturing: bool,
    },
    Button {
        text: String,
        action: Action,
    },
    /// 只读文本（版本、路径）。
    Value {
        text: String,
    },
    /// 主题色板（只读装饰）。
    Swatches,
    /// 带方框的勾选项。与 `Toggle` 的区别：它背后**不是一个配置字段**，
    /// 而是一个一次性意图（比如"这次导入允许覆盖"），所以状态存在 `Model` 里。
    Checkbox {
        on: bool,
        action: Action,
    },
    /// 皮肤缩略图网格。`selected`/`hover` 是 `Model::skins` 的下标。
    Gallery {
        count: usize,
        selected: Option<usize>,
        hover: Option<usize>,
    },
    /// 环上的一颗球：一行里塞下「上移 / 下移 / 热键 / 开关」（ADR-042）。
    ///
    /// 用复合控件而不是四行，是因为"顺序"只有和邻居摆在一起才读得出来：
    /// 拆成四行之后，一颗球的四行和下一颗球的四行混在一条列表里，
    /// 谁也数不清现在到底什么在上什么在下。
    RingItem {
        /// `config.ring.items` 的下标
        index: usize,
        name: String,
        enabled: bool,
        hotkey: String,
        capturing: bool,
    },
    /// 可编辑的单行文本（自定义球的名字、要运行的程序或命令）。
    Text {
        text: String,
        editing: bool,
        placeholder: String,
        /// 这个框改的是哪个球的哪个字段（点进来 / 写回去都靠它）。
        target: EditTarget,
    },
    Section {
        text: String,
    },
    /// 展开后的长说明：一段会自动换行的正文。
    ///
    /// 行高由 `layout()` 按**实际宽度**换算出来的行数决定 —— 所以这里存原文，
    /// 换行结果存在 `Layout::notes` 里（与 `rows` 同下标）。把换行放在布局阶段
    /// 是有意的：`build_rows` 不知道窗口多宽，而说明文字一旦换了行，
    /// 少一行会截断、多一行会留白，两种都很难看。
    Note {
        text: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    OpenConfig,
    OpenLogs,
    Reset,
    /// 从 `.zwskin` 导入（会弹系统文件对话框）。
    ImportSkin,
    /// 把当前皮肤导出成 `.zwskin`。
    ExportSkin,
    /// 在资源管理器里打开 `skins/`。
    OpenSkinsDir,
    /// 删掉当前这套皮肤。
    DeleteSkin,
    /// 切换"导入时覆盖已安装皮肤"。
    ToggleOverwrite,
    /// 展开/收起"覆盖安装"的长说明。
    ToggleOverwriteHelp,
    /// 导入字体文件（`.ttf` / `.otf` / `.ttc`）。
    ImportFont,
    /// 删掉当前字体包。
    DeleteFont,
    /// 在资源管理器里打开 `fonts/`。
    OpenFontsDir,
    /// 清空剪贴板历史（只删记录文件，不碰系统剪贴板）。
    ClearHistory,
    /// 在环的末尾加一颗空的自定义球（名字/命令随后在面板里填）。
    AddItem,
    /// 选一个程序文件，加一颗“打开它”的球（名字默认取文件名）。
    AddProgram,
    /// 给第 `usize` 颗自定义球选图标（PNG）。
    PickIcon(usize),
    /// 从环上移除第 `usize` 颗球（不可撤销，所以要问一次）。
    RemoveItem(usize),
}
#[derive(Clone, Debug)]
pub struct Row {
    pub field: Option<Field>,
    pub label: String,
    pub hint: Option<String>,
    pub control: Control,
}

impl Row {
    fn section(text: String) -> Self {
        Self {
            field: None,
            label: String::new(),
            hint: None,
            control: Control::Section { text },
        }
    }

    fn focusable(&self) -> bool {
        self.field.is_some()
            || matches!(
                self.control,
                Control::Button { .. } | Control::Checkbox { .. } | Control::Text { .. }
            )
            // 气泡球行整行都是「改了就算数」的行：焦点落到这里就能用
            // 空格切开关、↑↓ 换顺序（见 settings_window 的按键路由）。
            || matches!(self.control, Control::RingItem { .. })
    }

    fn height(&self) -> f32 {
        match self.control {
            Control::Section { .. } => SECTION_H,
            Control::Value { .. } | Control::Swatches => TEXT_ROW_H,
            // 占位：真实高度要等 `layout()` 换行后才知道（见 `Control::Note`）。
            Control::Note { .. } => 0.0,
            // 按**实际**行数给高度：只装一套皮肤时不该留两行的空白。
            Control::Gallery { count, .. } => {
                let rows = count
                    .div_ceil(GALLERY_COLS_NOMINAL)
                    .clamp(1, GALLERY_MAX_ROWS);
                // 缩略图墙的说明文字放在最下面一行，所以高度里要多给它一条。
                GALLERY_CELL_H * rows as f32
                    + GALLERY_GAP * (rows - 1) as f32
                    + 12.0
                    + GALLERY_HINT_H
            }
            _ => {
                if self.hint.is_some() {
                    ROW_H + HINT_ROW_EXTRA
                } else {
                    ROW_H
                }
            }
        }
    }
}

// ── 文本编辑状态 ──────────────────────────────────────

/// 正在捕获的热键属于谁。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureTarget {
    /// 呼出小兽（`general.hotkey`）
    Summon,
    /// 环上第 `usize` 颗球（`ring.items[i].hotkey`）
    Item(usize),
}

/// 一个可编辑文本的身份。用"哪个球的哪个字段"而不是行号：
/// 行号会随其它行的开关/顺序变化，存下来的行号两秒后就不对了。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTarget {
    ItemLabel(usize),
    ItemCommand(usize),
}

/// 编辑框的状态：缓冲区 + 光标 + 锚点（都是字节下标，落在字符边界上）。
///
/// `anchor` 是"选区的另一头"：它和 `cursor` 不相等时就是一段选区。
/// 单击时两者相等（没选区）；按住拖拽时 `anchor` 不动、`cursor` 跟着鼠标跑；
/// Shift + 方向键也是只动 `cursor`。
#[derive(Clone, Debug, PartialEq)]
pub struct Editing {
    pub target: EditTarget,
    pub buffer: String,
    pub cursor: usize,
    pub anchor: usize,
    /// 水平滚动的可视起点（字节下标，落在字符边界上）。
    ///
    /// 单行框放不下时靠它向左滚，而不是靠“画的时候截掉前几个字”：
    /// 光标位置、选区、滚动条 thumb 全要跟它对齐，各算各的必然错位。
    pub offset: usize,
    /// 内框宽度（逻辑像素）。布局就绪后由 `set_edit_viewport` 填。
    ///
    /// 默认 `INFINITY`：没量过宽度时 `ensure_caret_visible` 什么都不做，
    /// 于是“只改模型、不画界面”的单元测试不需要伪造视口。
    pub width: f32,
    /// 量文字用的 DPI 缩放（`width` 与它在同一次调用里设）。
    pub scale: f32,
}

/// 编辑框能识别的按键（winit 类型留在窗口层，这一层只认这个枚举）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKey {
    Backspace,
    Delete,
    /// 回车/失焦：写回配置
    Commit,
    /// Esc：丢掉这次改动
    Cancel,
}

/// 光标移动（方向键 / Home / End），`extend` 即是否按住 Shift 拉选区。
///
/// 单独立一个枚举而不是塞进 `EditKey`：它们要不要带 Shift 是窗口层的事，
/// 而塞进 `EditKey` 只会在每个 `match` 里多出四个 `unreachable!()`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMotion {
    Left,
    Right,
    Home,
    End,
}

// ── 模型 ──────────────────────────────────────────────────

pub struct Model {
    /// 工作副本。任何修改都在这里发生，然后整体交给 App 落地。
    pub config: Config,
    pub themes: Vec<Theme>,
    pub skins: Vec<(String, String)>,
    /// 字体包选项：`("" , 系统默认)` + 每个已安装的包。
    /// 由 `refresh_fonts()` 从磁盘扫出来（不在 `new` 里做，测试就不必碰磁盘）。
    pub fonts: Vec<(String, String)>,
    /// 字体包的完整信息（与 `fonts` 一一对应，用于"有没有中文字形"这类提示）。
    font_packs: Vec<zouwu_platform::fontpack::FontPack>,
    pub cwd_paths: Paths,
    pub tab: Tab,
    /// 键盘焦点（`Layout::rows` 的下标）。
    pub focus: Option<usize>,
    /// 鼠标悬停的行（仅视觉反馈）。
    pub hover: Option<usize>,
    pub keyboard_used: bool,
    pub scroll: f32,
    /// 正在捕获哪一条热键（`None` = 没在捕获）。
    ///
    /// 从 `bool` 改成带目标的枚举，是因为现在每颗球都有自己的热键：
    /// 只知道"在捕获"而不知道"在给谁捕获"，就只能写回呼出热键那一个字段。
    pub capturing: Option<CaptureTarget>,
    /// 正在编辑的文本框（`None` = 没有在编辑任何文本）。
    pub editing: Option<Editing>,
    pub notice: Option<String>,
    pub i18n: I18n,
    /// 形象页的缩略图缓存，下标与 `skins` 对齐。`None` = 这套皮肤打不开。
    pub thumbs: Vec<Option<Thumb>>,
    /// 鼠标悬停在第几个缩略图上（仅视觉反馈）。
    pub hover_skin: Option<usize>,
    /// 导入时是否覆盖同名皮肤。
    ///
    /// **故意不持久化**：覆盖是破坏性动作，每次打开面板都要重新表态，
    /// 而不是上周勾过一次、这周就默认覆盖掉别人的皮肤。
    pub overwrite_skin: bool,
    /// "覆盖安装"的长说明是否展开。纯界面状态，不落盘：下次打开面板
    /// 应该是干净的一屏，而不是接着上次读了一半的说明。
    pub overwrite_help_open: bool,
    /// 缩略图缓存键：主题或皮肤列表变了才重算。
    thumbs_key: String,
}

/// 面板要展示的路径（关于页）。用 `String` 是为了在这里就能截断。
pub struct Paths {
    pub config: String,
    pub log: String,
}

impl Model {
    pub fn new(
        config: Config,
        themes: Vec<Theme>,
        skins: Vec<(String, String)>,
        paths: Paths,
    ) -> Self {
        let i18n = I18n::new(&config.general.language);
        Self {
            config,
            themes,
            skins,
            fonts: Vec::new(),
            font_packs: Vec::new(),
            cwd_paths: paths,
            tab: Tab::General,
            focus: None,
            hover: None,
            keyboard_used: false,
            scroll: 0.0,
            capturing: None,
            editing: None,
            notice: None,
            i18n,
            thumbs: Vec::new(),
            hover_skin: None,
            overwrite_skin: false,
            overwrite_help_open: false,
            thumbs_key: String::new(),
        }
    }

    /// 重扫字体包（导入/删除之后要调；面板打开时也调一次）。
    pub fn refresh_fonts(&mut self) {
        self.font_packs = zouwu_platform::fontpack::list();
        self.fonts = std::iter::once((String::new(), self.i18n.t("settings.font.system")))
            .chain(
                self.font_packs
                    .iter()
                    .map(|p| (p.id.clone(), p.display_name().to_string())),
            )
            .collect();
        // 设置里的包可能已经被删掉（在资源管理器里手删目录），
        // 这时下拉框找不到当前值，显示会是裸 id。把配置拉回空串 = 系统字体。
        if !self.config.appearance.font_pack.is_empty()
            && !self
                .fonts
                .iter()
                .any(|(id, _)| *id == self.config.appearance.font_pack)
        {
            self.config.appearance.font_pack = String::new();
        }
    }

    /// 当前字体家族的显示名（`None` = 系统默认字体）。
    pub fn font_family(&self) -> Option<String> {
        self.current_font_pack().map(|p| p.family.clone())
    }

    /// 当前选中的字体包（系统字体时是 `None`）。
    pub fn current_font_pack(&self) -> Option<&zouwu_platform::fontpack::FontPack> {
        let id = &self.config.appearance.font_pack;
        if id.is_empty() {
            return None;
        }
        self.font_packs.iter().find(|p| &p.id == id)
    }

    /// 选中字体包的提醒文案：只有"缺中文字形"这一种值得打断用户。
    ///
    /// 不说清楚的话，用户会看到中文突然变成宋体点阵、还比拉丁字窄，
    /// 只会以为是渲染坏了。
    fn font_note(&self) -> Option<String> {
        let pack = self.current_font_pack()?;
        if pack.has_cjk {
            return None;
        }
        Some(self.i18n.t("settings.font.no.cjk"))
    }

    pub fn theme(&self) -> Theme {
        // 面板自身也用当前主题画：换肤在设置面板里也立刻看得见。
        // 注意要查 `themes`（含用户外部主题），不能只查内置表。
        self.themes
            .iter()
            .find(|t| t.id == self.config.appearance.theme)
            .cloned()
            .unwrap_or_else(Theme::fallback)
    }

    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            // 换页 = 离开当前页面：正在输的文本先落地，不然用户刚打的字凭空没了。
            commit_edit(self);
            self.tab = tab;
            self.scroll = 0.0;
            self.focus = None;
            self.capturing = None;
            self.editing = None;
            self.hover = None;
            self.hover_skin = None;
        }
    }

    /// 当前皮肤在 `skins` 里的下标（找不到 → `None`，例如被手工删掉了）。
    pub fn skin_index(&self) -> Option<usize> {
        self.skins
            .iter()
            .position(|(id, _)| *id == self.config.appearance.skin)
    }

    /// 重新扫描磁盘上的皮肤。导入/删除之后必须调，否则列表还是旧的。
    pub fn refresh_skins(&mut self) {
        self.skins = zouwu_platform::assets::skin_choices(&self.i18n);
        self.thumbs_key.clear();
        self.hover_skin = None;
    }

    /// 懒加载缩略图。
    ///
    /// 只在主题或皮肤数量变化时重算：一套皮肤要解码 PNG + 走一遍主渲染管线，
    /// 每帧都做的话这个面板的 CPU 会从 0% 变成几十。
    pub fn ensure_thumbs(&mut self) {
        let key = format!("{}:{}", self.config.appearance.theme, self.skins.len());
        if key == self.thumbs_key && self.thumbs.len() == self.skins.len() {
            return;
        }
        let theme = self.theme();
        self.thumbs = self
            .skins
            .iter()
            .map(|(id, _)| {
                let procedural = id == PROCEDURAL_SKIN_ID;
                let skin = if procedural {
                    None
                } else {
                    zouwu_platform::assets::load_skin(id)
                        .ok()
                        .and_then(|pkg| Skin::decode(&pkg).ok())
                };
                // 素材皮肤打不开时**不给**缩略图（界面画成"打不开"），
                // 而不是退回代码绘制——那会让坏皮肤看起来是好的。
                if !procedural && skin.is_none() {
                    return None;
                }
                thumb::render(skin.as_ref(), &theme, 128).ok()
            })
            .collect();
        self.thumbs_key = key;
    }
}

// ── 行构建 ────────────────────────────────────────────────

pub fn build_rows(model: &Model) -> Vec<Row> {
    let i18n = &model.i18n;
    let mut rows = Vec::new();
    match model.tab {
        Tab::General => {
            rows.push(choice_row(model, Field::Language, language_options(i18n)));
            rows.push(Row {
                field: Some(Field::Hotkey),
                label: i18n.t("settings.hotkey"),
                hint: Some(i18n.t("settings.hotkey.hint")),
                control: Control::Hotkey {
                    text: hotkey_text(model),
                    capturing: model.capturing == Some(CaptureTarget::Summon),
                },
            });
            rows.push(toggle_row(model, Field::StartOnBoot));
            rows.push(toggle_row(model, Field::ShowOnStart));
            rows.push(toggle_row(model, Field::BringToFront));
            rows.push(toggle_row(model, Field::AlwaysOnTop));
            rows.push(step_row(model, Field::WindowSize));

            rows.push(Row::section(i18n.t("settings.section.eye-care")));
            rows.push(Row {
                field: Some(Field::EyeCareEnabled),
                label: i18n.t("settings.eye-care"),
                hint: Some(i18n.t("settings.eye-care.hint")),
                control: Control::Toggle {
                    on: model.config.eye_care.enabled,
                },
            });
            rows.push(Row {
                field: Some(Field::EyeCareStrength),
                label: i18n.t("settings.eye-care.strength"),
                hint: Some(i18n.t("settings.eye-care.strength.hint")),
                control: step_control(model, Field::EyeCareStrength),
            });

            rows.push(Row::section(i18n.t("settings.section.alt-drag")));
            rows.push(Row {
                field: Some(Field::AltDragEnabled),
                label: i18n.t("settings.alt-drag"),
                hint: Some(i18n.t("settings.alt-drag.hint")),
                control: Control::Toggle {
                    on: model.config.alt_drag.enabled,
                },
            });

            rows.push(Row::section(i18n.t("settings.section.clipboard")));
            rows.push(Row {
                field: Some(Field::ClipboardEnabled),
                label: i18n.t("settings.clipboard"),
                hint: Some(i18n.t("settings.clipboard.hint")),
                control: Control::Toggle {
                    on: model.config.clipboard.enabled,
                },
            });
            rows.push(Row {
                field: Some(Field::ClipboardLimit),
                label: i18n.t("settings.clipboard.limit"),
                hint: Some(i18n.t("settings.clipboard.limit.hint")),
                control: step_control(model, Field::ClipboardLimit),
            });
            rows.push(Row {
                field: None,
                label: i18n.t("settings.clipboard.clear"),
                hint: Some(i18n.t("settings.clipboard.clear.hint")),
                control: Control::Button {
                    text: i18n.t("settings.clipboard.clear.button"),
                    action: Action::ClearHistory,
                },
            });
        }
        Tab::Bubbles => {
            rows.push(Row::section(i18n.t("settings.section.bubbles")));
            rows.push(Row {
                field: None,
                label: String::new(),
                hint: None,
                control: Control::Note {
                    text: i18n.t("settings.bubbles.note"),
                },
            });
            let items = &model.config.ring.items;
            if items.is_empty() {
                rows.push(Row {
                    field: None,
                    label: i18n.t("settings.bubbles.empty"),
                    hint: None,
                    control: Control::Value {
                        text: String::new(),
                    },
                });
            }
            for (index, item) in items.iter().enumerate() {
                let custom = builtin(&item.id).is_none();
                rows.push(ring_item_row(model, index));
                if !custom {
                    continue;
                }
                // 自定义球才需要名字 / 命令 / 图标：内置球的名字来自 i18n，
                // 图标是写死的字形，给它们开输入框只会诱使用户改坏。
                rows.push(ring_text_row(
                    model,
                    EditTarget::ItemLabel(index),
                    "settings.bubbles.name",
                    "settings.bubbles.name.hint",
                ));
                rows.push(ring_text_row(
                    model,
                    EditTarget::ItemCommand(index),
                    "settings.bubbles.command",
                    "settings.bubbles.command.hint",
                ));
                rows.push(Row {
                    field: None,
                    label: i18n.t("settings.bubbles.icon"),
                    hint: None,
                    control: Control::Button {
                        text: if item.icon.trim().is_empty() {
                            i18n.t("settings.bubbles.icon.default")
                        } else {
                            i18n.t("settings.bubbles.icon.pick")
                        },
                        action: Action::PickIcon(index),
                    },
                });
                rows.push(Row {
                    field: None,
                    label: String::new(),
                    hint: Some(i18n.t("settings.bubbles.remove.hint")),
                    control: Control::Button {
                        text: i18n.t("settings.bubbles.remove"),
                        action: Action::RemoveItem(index),
                    },
                });
            }
            rows.push(Row::section(i18n.t("settings.section.bubbles.add")));
            for (text_key, action, hint) in [
                (
                    "settings.bubbles.add.command",
                    Action::AddItem,
                    Some(i18n.t("settings.bubbles.add.command.hint")),
                ),
                ("settings.bubbles.add.program", Action::AddProgram, None),
            ] {
                rows.push(Row {
                    field: None,
                    label: String::new(),
                    hint,
                    control: Control::Button {
                        text: i18n.t(text_key),
                        action,
                    },
                });
            }
        }
        Tab::Interaction => {
            rows.push(Row::section(i18n.t("settings.section.behavior")));
            rows.push(step_row(model, Field::HoverDelay));
            rows.push(step_row(model, Field::CollapseDelay));
            rows.push(toggle_row(model, Field::ExpandOnClick));
            rows.push(Row {
                field: Some(Field::ExpandOnSummon),
                label: i18n.t(Field::ExpandOnSummon.label_key()),
                hint: Some(i18n.t("settings.expand-on-summon.hint")),
                control: Control::Toggle {
                    on: model.config.interaction.expand_on_summon,
                },
            });
            rows.push(step_row(model, Field::HoverFactor));
            rows.push(step_row(model, Field::CollapseFactor));
            rows.push(step_row(model, Field::HitTolerance));
            rows.push(Row::section(i18n.t("settings.section.beast")));
            rows.push(step_row(model, Field::BuddyRadius));
            rows.push(toggle_row(model, Field::Breathe));
            rows.push(Row::section(i18n.t("settings.section.ring")));
            rows.push(step_row(model, Field::RingRadius));
            rows.push(step_row_hint(
                model,
                Field::OrbitStart,
                "settings.orbit-start.hint",
            ));
            rows.push(step_row(model, Field::BubbleRadius));
            rows.push(step_row(model, Field::MaxPerRing));
            rows.push(step_row_hint(
                model,
                Field::RingGap,
                "settings.ring-gap.hint",
            ));
            rows.push(step_row(model, Field::FloatAmplitude));
            rows.push(step_row(model, Field::ExpandStagger));
        }
        Tab::Appearance => {
            rows.push(choice_row(model, Field::Theme, theme_options(model)));
            rows.push(Row {
                field: None,
                label: String::new(),
                hint: None,
                control: Control::Swatches,
            });
            rows.push(choice_row(model, Field::Skin, model.skins.clone()));
            rows.push(Row {
                field: Some(Field::FontPack),
                label: i18n.t("settings.font"),
                // 缺中文字形时把"说明"换成"警告"：这一条会影响整个界面观感，
                // 比"怎么导入"重要得多。
                hint: model
                    .font_note()
                    .or_else(|| Some(i18n.t("settings.font.hint"))),
                control: Control::Choice {
                    text: model
                        .fonts
                        .iter()
                        .find(|(id, _)| *id == model.config.appearance.font_pack)
                        .map(|(_, label)| label.clone())
                        .unwrap_or_else(|| i18n.t("settings.font.system")),
                },
            });
            rows.push(Row {
                field: None,
                label: i18n.t("settings.font.family"),
                hint: None,
                control: Control::Value {
                    text: model
                        .font_family()
                        .unwrap_or_else(|| i18n.t("settings.font.family.system")),
                },
            });
            for (text_key, action) in [
                ("settings.font.import", Action::ImportFont),
                ("settings.font.open.dir", Action::OpenFontsDir),
            ] {
                rows.push(Row {
                    field: None,
                    label: String::new(),
                    hint: None,
                    control: Control::Button {
                        text: i18n.t(text_key),
                        action,
                    },
                });
            }
            rows.push(Row {
                field: None,
                label: String::new(),
                hint: Some(i18n.t("settings.font.delete.hint")),
                control: Control::Button {
                    text: i18n.t("settings.font.delete"),
                    action: Action::DeleteFont,
                },
            });
        }
        Tab::Skins => {
            let current = model.skin_index();
            rows.push(Row::section(i18n.t("settings.section.skins")));
            rows.push(Row {
                field: None,
                label: i18n.t("settings.skin.current"),
                hint: None,
                control: Control::Value {
                    text: current
                        .and_then(|i| model.skins.get(i))
                        .map(|(id, name)| format!("{name} · {id}"))
                        .unwrap_or_else(|| model.config.appearance.skin.clone()),
                },
            });
            rows.push(Row {
                field: None,
                label: String::new(),
                hint: Some(i18n.t("settings.skin.gallery.hint")),
                control: Control::Gallery {
                    count: model.skins.len(),
                    selected: current,
                    hover: model.hover_skin,
                },
            });
            rows.push(Row::section(i18n.t("settings.section.skinpack")));
            rows.push(Row {
                field: None,
                label: i18n.t("settings.skin.overwrite"),
                // 一行能说完的短句：长解释收进下面的"说明…"，需要时才展开。
                hint: Some(i18n.t("settings.skin.overwrite.hint")),
                control: Control::Checkbox {
                    on: model.overwrite_skin,
                    action: Action::ToggleOverwrite,
                },
            });
            rows.push(Row {
                field: None,
                label: String::new(),
                hint: None,
                control: Control::Button {
                    text: i18n.t(if model.overwrite_help_open {
                        "settings.skin.overwrite.less"
                    } else {
                        "settings.skin.overwrite.more"
                    }),
                    action: Action::ToggleOverwriteHelp,
                },
            });
            if model.overwrite_help_open {
                rows.push(Row {
                    field: None,
                    label: String::new(),
                    hint: None,
                    control: Control::Note {
                        text: i18n.t("settings.skin.overwrite.note"),
                    },
                });
            }
            for (text_key, action) in [
                ("settings.skin.import", Action::ImportSkin),
                ("settings.skin.export", Action::ExportSkin),
                ("settings.skin.open.dir", Action::OpenSkinsDir),
            ] {
                rows.push(Row {
                    field: None,
                    label: String::new(),
                    hint: None,
                    control: Control::Button {
                        text: i18n.t(text_key),
                        action,
                    },
                });
            }
            rows.push(Row {
                field: None,
                label: String::new(),
                hint: Some(i18n.t("settings.skin.delete.hint")),
                control: Control::Button {
                    text: i18n.t("settings.skin.delete"),
                    action: Action::DeleteSkin,
                },
            });
        }
        Tab::About => {
            rows.push(Row::section(i18n.t("settings.section.logging")));
            rows.push(choice_row(model, Field::LogLevel, log_level_options()));
            rows.push(toggle_row(model, Field::LogToFile));
            rows.push(Row {
                field: None,
                label: i18n.t("settings.log.file"),
                hint: None,
                control: Control::Value {
                    text: model.cwd_paths.log.clone(),
                },
            });
            rows.push(Row {
                field: None,
                label: i18n.t("settings.about.version"),
                hint: None,
                control: Control::Value {
                    text: format!("v{}", env!("CARGO_PKG_VERSION")),
                },
            });
            rows.push(Row {
                field: None,
                label: i18n.t("settings.about.install"),
                hint: None,
                control: Control::Value {
                    text: install_label(i18n),
                },
            });
            rows.push(Row {
                field: None,
                label: i18n.t("settings.about.platform"),
                hint: None,
                control: Control::Value {
                    text: zouwu_platform::platform_name().to_string(),
                },
            });
            rows.push(Row {
                field: None,
                label: i18n.t("settings.about.config"),
                hint: None,
                control: Control::Value {
                    text: model.cwd_paths.config.clone(),
                },
            });
            rows.push(Row {
                field: None,
                label: String::new(),
                hint: None,
                control: Control::Button {
                    text: i18n.t("settings.open.config"),
                    action: Action::OpenConfig,
                },
            });
            rows.push(Row {
                field: None,
                label: String::new(),
                hint: None,
                control: Control::Button {
                    text: i18n.t("settings.open.logs"),
                    action: Action::OpenLogs,
                },
            });
            rows.push(Row {
                field: None,
                label: String::new(),
                hint: Some(i18n.t("settings.reset.hint")),
                control: Control::Button {
                    text: i18n.t("settings.reset"),
                    action: Action::Reset,
                },
            });
        }
    }
    rows
}

/// 气泡球行：名字 + 开关 + 热键框 + ↑↓。
fn ring_item_row(model: &Model, index: usize) -> Row {
    let i18n = &model.i18n;
    let Some(item) = model.config.ring.items.get(index) else {
        // 行是从 `items` 现算的，索引不可能越界；真越界时给一条空行，
        // 也不要 panic 把整个面板打死。
        return Row {
            field: None,
            label: String::new(),
            hint: None,
            control: Control::Value {
                text: String::new(),
            },
        };
    };
    let name = match builtin(&item.id) {
        Some(b) => i18n.t(b.label_key),
        None => item.display_name().to_string(),
    };
    Row {
        field: None,
        label: name,
        // 不加 hint：次序怎么改已经写在页首那条说明里，四行重复一遍只会把
        // 每行撑高 18px、把「移除」按钮挤到滚动条外面。
        hint: None,
        control: Control::RingItem {
            index,
            name: item.id.clone(),
            enabled: item.enabled,
            hotkey: if model.capturing == Some(CaptureTarget::Item(index)) {
                i18n.t("settings.hotkey.capturing")
            } else if item.hotkey.trim().is_empty() {
                i18n.t("settings.hotkey.none")
            } else {
                item.hotkey.clone()
            },
            capturing: model.capturing == Some(CaptureTarget::Item(index)),
        },
    }
}

/// 一行可编辑文本（名字 / 命令）。编辑中的缓冲从 `model.editing` 取，
/// 所以正在打字时看到的就是缓冲内容，而不是落盘后的旧值。
fn ring_text_row(model: &Model, target: EditTarget, label_key: &str, hint_key: &str) -> Row {
    let i18n = &model.i18n;
    let editing = model.editing.as_ref().filter(|e| e.target == target);
    let stored = match target {
        EditTarget::ItemLabel(i) => model
            .config
            .ring
            .items
            .get(i)
            .map(|it| it.label.clone())
            .unwrap_or_default(),
        EditTarget::ItemCommand(i) => model
            .config
            .ring
            .items
            .get(i)
            .map(|it| it.command.clone())
            .unwrap_or_default(),
    };
    Row {
        field: None,
        label: i18n.t(label_key),
        hint: Some(i18n.t(hint_key)),
        control: Control::Text {
            text: editing.map(|e| e.buffer.clone()).unwrap_or(stored),
            editing: editing.is_some(),
            placeholder: i18n.t("settings.bubbles.empty.text"),
            target,
        },
    }
}

fn toggle_row(model: &Model, field: Field) -> Row {
    Row {
        field: Some(field),
        label: model.i18n.t(field.label_key()),
        hint: None,
        control: Control::Toggle {
            on: field.bool(&model.config).unwrap_or(false),
        },
    }
}

fn step_row(model: &Model, field: Field) -> Row {
    Row {
        field: Some(field),
        label: model.i18n.t(field.label_key()),
        hint: None,
        control: step_control(model, field),
    }
}

/// 同上，但多一行说明（角度、外圈这类“光看名字不知道填多少”的项用）。
fn step_row_hint(model: &Model, field: Field, hint_key: &str) -> Row {
    let mut row = step_row(model, field);
    row.hint = Some(model.i18n.t(hint_key));
    row
}

/// 数值行的控件部分（带提示的行自己拼 `Row`，但仍要用同一套控件）。
fn step_control(model: &Model, field: Field) -> Control {
    let spec = field.num_spec().expect("step field needs a spec");
    let value = field.num(&model.config).unwrap_or(spec.min);
    Control::Step {
        text: format_value(&model.i18n, value, spec.unit),
        at_min: value <= spec.min,
        at_max: value >= spec.max,
    }
}

fn choice_row(model: &Model, field: Field, options: Vec<(String, String)>) -> Row {
    let current = current_choice(model, field);
    let text = options
        .iter()
        .find(|(value, _)| *value == current)
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| current.clone());
    Row {
        field: Some(field),
        label: model.i18n.t(field.label_key()),
        hint: None,
        control: Control::Choice { text },
    }
}

fn format_value(i18n: &I18n, value: i64, unit: Unit) -> String {
    match unit {
        Unit::Ms => i18n.t_args("settings.ms", &[("ms", &value.to_string())]),
        Unit::Px => i18n.t_args("settings.px", &[("px", &value.to_string())]),
        Unit::Percent => format!("{value}%"),
        Unit::Count => i18n.t_args("settings.count", &[("count", &value.to_string())]),
        Unit::Deg => format!("{value}\u{b0}"),
        Unit::Bubbles => i18n.t_args("settings.bubbles.count", &[("count", &value.to_string())]),
    }
}

fn hotkey_text(model: &Model) -> String {
    if model.capturing == Some(CaptureTarget::Summon) {
        return model.i18n.t("settings.hotkey.capturing");
    }
    if model.config.general.hotkey.trim().is_empty() {
        return model.i18n.t("settings.hotkey.empty");
    }
    model.config.general.hotkey.clone()
}

fn language_options(i18n: &I18n) -> Vec<(String, String)> {
    let mut out = vec![(AUTO.to_string(), i18n.t("settings.language.auto"))];
    for lang in LANGUAGES {
        // 语言名用**自身语言**写：英文界面里找中文的用户认得"简体中文"，
        // 而不一定认得 "Chinese (Simplified)"。
        let label = match lang {
            "zh-CN" => "简体中文",
            "en-US" => "English",
            other => other,
        };
        out.push((lang.to_string(), label.to_string()));
    }
    out
}

fn theme_options(model: &Model) -> Vec<(String, String)> {
    model
        .themes
        .iter()
        .map(|theme| (theme.id.clone(), theme.name.clone()))
        .collect()
}

fn log_level_options() -> Vec<(String, String)> {
    LogCfg::LEVELS
        .iter()
        .map(|level| (level.to_string(), level.to_string()))
        .collect()
}

fn current_choice(model: &Model, field: Field) -> String {
    match field {
        Field::Language => model.config.general.language.clone(),
        Field::Theme => model.config.appearance.theme.clone(),
        Field::Skin => model.config.appearance.skin.clone(),
        Field::FontPack => model.config.appearance.font_pack.clone(),
        Field::LogLevel => model.config.log.level.clone(),
        _ => String::new(),
    }
}

fn choice_options(model: &Model, field: Field) -> Vec<(String, String)> {
    match field {
        Field::Language => language_options(&model.i18n),
        Field::Theme => theme_options(model),
        Field::Skin => model.skins.clone(),
        Field::FontPack => model.fonts.clone(),
        Field::LogLevel => log_level_options(),
        _ => Vec::new(),
    }
}

// ── 布局 ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct RowLayout {
    /// 已含滚动偏移的窗口坐标。
    pub rect: Rect,
    /// 控件区（整体）。
    pub control: Rect,
    /// 递减/递增按钮（步进、选择）。
    pub dec: Rect,
    pub inc: Rect,
    /// 按下就触发的区域（开关、热键框、按钮）。对复合控件是**开关**。
    pub hit: Rect,
    /// 复合控件（`Control::RingItem`）的子区域：上移 / 下移 / 热键框。
    /// 非复合控件是退化矩形（宽高为 0），`contains()` 永远为假。
    pub up: Rect,
    pub down: Rect,
    pub hotkey: Rect,
}

pub struct Layout {
    pub w: f32,
    pub h: f32,
    pub content: Rect,
    /// 键盘焦点（由 `Model::focus` 带进来，供 `move_focus` 计算下一项）。
    pub focus: Option<usize>,
    /// 全部行的总高度（含滚动），用于滚动条与夹取。
    pub rows_h: f32,
    pub tabs: Vec<(Tab, Rect)>,
    pub rows: Vec<(Row, RowLayout)>,
    /// 与 `rows` 同下标的换行结果：只有 `Control::Note` 的行非空。
    pub notes: Vec<Vec<String>>,
    pub scrollbar: Option<(Rect, Rect)>,
    /// 底部提示的基线
    pub footer: Rect,
}

/// 把滚动位置吸附到「行边界」上。
///
/// 不吸附的话，内容带的上下边缘会随机切过某一行文字——看起来就像字被挡住了一半
/// （用户报的"文字遮挡"里有一半是这个）。吸附之后，边缘永远落在两行之间。
///
/// 例外：滚到底时直接给 `max_scroll`，让最后一行完整可见（否则最后一行永远差几像素）。
/// 把一段说明按**逻辑像素**宽度折行。
///
/// 度量在平台层是物理像素，这里传的是逻辑字号与逻辑宽度 —— 两者同比例，
/// 折行位置因此与 DPI 无关（汉字是定宽 1em，这一点是精确的；
/// 拉丁文在极端 DPI 下可能差一个词，且差的方向是"多一行"，由
/// `Layout::notes` 的高度兜住，不会截断）。
fn wrap_note(text: &str, width: f32) -> Vec<String> {
    crate::text::TextEngine::with(|engine| engine.wrap(text, 11.0, width))
}

fn snap_scroll(heights: &[f32], raw: f32, max_scroll: f32) -> f32 {
    if raw <= 0.5 {
        return 0.0;
    }
    if raw >= max_scroll - 1.0 {
        return max_scroll;
    }
    let mut acc = 0.0f32;
    let mut best = 0.0f32;
    let mut best_dist = f32::MAX;
    for h in heights {
        // 往上、往下都试：吸到"离得最近的那条边界"。
        for cand in [acc, acc + h] {
            let d = (cand - raw).abs();
            if d < best_dist {
                best_dist = d;
                best = cand;
            }
        }
        acc += h;
    }
    best.clamp(0.0, max_scroll)
}

/// 控件区的默认位置：贴右边、高度 30、居中于行内。
fn default_control(rect: Rect) -> Rect {
    Rect::new(
        rect.right() - CONTROL_W,
        rect.y + (rect.h - 30.0) * 0.5,
        CONTROL_W,
        30.0,
    )
}

pub fn layout(model: &Model, w: f32, h: f32) -> Layout {
    let content = Rect::new(0.0, HEADER_H, w, (h - HEADER_H - FOOTER_H).max(0.0));

    // 标签页：等宽铺开，左对齐到内容左边。
    //
    // 宽度按可用宽度均分并夹住上限：标签个数会变（ADR-042 加了「气泡球」），
    // 写死的 108px × 6 在最小窗口（560）里会直接溢出、被窗口裁掉一截。
    let tab_h = 30.0;
    let count = Tab::ALL.len() as f32;
    let gap = 8.0;
    let tab_w = ((w - MARGIN * 2.0 - gap * (count - 1.0)) / count).clamp(64.0, 108.0);
    // 垂直居中在 header 带里，而不是贴底：header 现在只有标签页一层的厚度。
    let tabs_y = (HEADER_H - tab_h) * 0.5;
    let mut tabs = Vec::with_capacity(Tab::ALL.len());
    for (i, tab) in Tab::ALL.iter().enumerate() {
        tabs.push((
            *tab,
            Rect::new(MARGIN + i as f32 * (tab_w + gap), tabs_y, tab_w, tab_h),
        ));
    }

    let rows = build_rows(model);

    // 说明文字在这里换行：只有此刻才知道"内容区到底有多宽"。
    // 别用整行宽度：说明块左右各内缩 4px，再留 4% 余量。
    //
    // 4% 不是玄学：折行用的是**逻辑**字号与逻辑宽度（布局阶段拿不到 DPI），
    // 而绘制时字号被乘上缩放 —— GDI 的字符步进要取整，150% 下折成的宽度
    // 会比线性外推宽几个百分点。余量吃掉这点误差，右侧才不会顶出边框。
    let avail = ((w - MARGIN * 2.0 - 8.0) * 0.96).max(80.0);
    let mut notes: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    let mut heights: Vec<f32> = Vec::with_capacity(rows.len());
    for row in &rows {
        match &row.control {
            Control::Note { text } => {
                let lines = wrap_note(text, avail);
                heights.push(NOTE_PAD * 2.0 + lines.len() as f32 * NOTE_LINE_H);
                notes.push(lines);
            }
            _ => {
                heights.push(row.height());
                notes.push(Vec::new());
            }
        }
    }

    let rows_h: f32 = heights.iter().sum::<f32>() + MARGIN * 2.0;
    let max_scroll = (rows_h - content.h).max(0.0);
    let scroll = snap_scroll(&heights, model.scroll.clamp(0.0, max_scroll), max_scroll);

    let mut out_rows = Vec::with_capacity(rows.len());
    let mut y = content.y + MARGIN - scroll;
    for (row, rh) in rows.into_iter().zip(heights.iter().copied()) {
        let rect = Rect::new(MARGIN, y, w - MARGIN * 2.0, rh);
        // 退化矩形：不是复合控件的行用不到这几个子区域，但 `RowLayout`
        // 要保持结构统一，以免每个消费方都要 `Option` 一层。
        let none = Rect::new(rect.right(), y, 0.0, 0.0);
        let control = default_control(rect);
        let (control, up, down, hotkey, hit) = match row.control {
            Control::Toggle { .. } => {
                let hit = Rect::new(control.right() - 46.0, y + (rh - 26.0) * 0.5, 46.0, 26.0);
                (control, none, none, none, hit)
            }
            Control::Hotkey { .. } => {
                let hit = Rect::new(control.x, y + (rh - 30.0) * 0.5, CONTROL_W, 30.0);
                (control, none, none, none, hit)
            }
            Control::Button { .. } => {
                let hit = Rect::new(
                    rect.right() - BUTTON_W,
                    y + (rh - 30.0) * 0.5,
                    BUTTON_W,
                    30.0,
                );
                (control, none, none, none, hit)
            }
            Control::Checkbox { .. } => {
                let hit = Rect::new(rect.right() - 24.0, y + (rh - 24.0) * 0.5, 24.0, 24.0);
                (control, none, none, none, hit)
            }
            // 缩略图墙占满整行宽度：格子要尽量大，缩略图才有辨识度。
            Control::Gallery { .. } => {
                let hit = Rect::new(rect.x, y + 6.0, rect.w, rh - 12.0);
                (hit, none, none, none, hit)
            }
            // 复合控件：从右往左排「开关 / 热键框 / 下移 / 上移」，
            // 剩下的宽度全给名字。
            Control::RingItem { .. } => {
                let cy = y + rh * 0.5;
                let toggle =
                    Rect::new(rect.right() - RING_TOGGLE_W, cy - 13.0, RING_TOGGLE_W, 26.0);
                let hotkey = Rect::new(
                    toggle.x - 8.0 - RING_HOTKEY_W,
                    cy - 15.0,
                    RING_HOTKEY_W,
                    30.0,
                );
                let down = Rect::new(hotkey.x - 8.0 - RING_ARROW_W, cy - 13.0, RING_ARROW_W, 26.0);
                let up = Rect::new(down.x - 4.0 - RING_ARROW_W, cy - 13.0, RING_ARROW_W, 26.0);
                // `control` 指整个复合区的左缘：`draw_row` 用它算名字区宽度。
                let control = Rect::new(up.x, cy - 15.0, rect.right() - up.x, 30.0);
                (control, up, down, hotkey, toggle)
            }
            // 文本框：给一半宽度，够看清一条命令行，又留得下左边的名字。
            Control::Text { .. } => {
                let tw = (rect.w * 0.52).clamp(160.0, 360.0);
                let box_rect = Rect::new(rect.right() - tw, y + (rh - 30.0) * 0.5, tw, 30.0);
                (box_rect, none, none, none, box_rect)
            }
            _ => (control, none, none, none, control),
        };
        let dec = Rect::new(control.x, control.y, 30.0, control.h);
        let inc = Rect::new(control.right() - 30.0, control.y, 30.0, control.h);
        out_rows.push((
            row,
            RowLayout {
                rect,
                control,
                dec,
                inc,
                hit,
                up,
                down,
                hotkey,
            },
        ));
        y += rh;
    }

    let scrollbar = if max_scroll > 0.5 {
        let track = Rect::new(w - 10.0, content.y + 4.0, 4.0, content.h - 8.0);
        let ratio = content.h / rows_h;
        let thumb_h = (track.h * ratio).max(24.0);
        let t = if max_scroll > 0.0 {
            scroll / max_scroll
        } else {
            0.0
        };
        let thumb = Rect::new(track.x, track.y + (track.h - thumb_h) * t, track.w, thumb_h);
        Some((track, thumb))
    } else {
        None
    };

    Layout {
        w,
        h,
        content,
        notes,
        focus: model.focus,
        rows_h,
        tabs,
        rows: out_rows,
        scrollbar,
        footer: Rect::new(MARGIN, h - FOOTER_H + 8.0, w - MARGIN * 2.0, FOOTER_H - 8.0),
    }
}

impl Layout {
    pub fn max_scroll(&self) -> f32 {
        (self.rows_h - self.content.h).max(0.0)
    }

    /// 键盘焦点可停的行。
    fn focusable(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, (row, _))| row.focusable())
            .map(|(i, _)| i)
            .collect()
    }

    pub fn move_focus(&self, forward: bool) -> Option<usize> {
        let list = self.focusable();
        if list.is_empty() {
            return None;
        }
        let current = list.iter().position(|i| Some(*i) == self.focus_hint());
        let next = match (current, forward) {
            (Some(0), false) => list.len() - 1,
            (None, false) => list.len() - 1,
            (Some(i), false) => i - 1,
            (Some(i), true) => (i + 1) % list.len(),
            (None, true) => 0,
        };
        Some(list[next])
    }

    /// 由 `Model::focus` 注入的"上次焦点"，仅用于 `move_focus` 计算。
    fn focus_hint(&self) -> Option<usize> {
        self.focus
    }
}

// ── 命中的种类 ────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(Tab),
    /// 整行（用于聚焦）
    Row(usize),
    /// 数值/枚举的 ±
    Step(usize, i32),
    /// 开关 / 热键框 / 按钮
    Press(usize),
    /// 气泡球行里的 ↑↓：`(行号, -1 上移 / +1 下移)`
    RingMove(usize, i32),
    /// 气泡球行里的热键框（点了就开始录键）
    RingHotkey(usize),
    /// 缩略图墙里的第 `usize` 张（`Model::skins` 下标）
    GalleryDot(usize),
    Scrollbar,
}

pub fn hit(layout: &Layout, p: Point) -> Option<Hit> {
    for (tab, rect) in &layout.tabs {
        if rect.contains(p) {
            return Some(Hit::Tab(*tab));
        }
    }
    if let Some((track, _)) = &layout.scrollbar {
        if track.inset(-2.0, 0.0).contains(p) {
            return Some(Hit::Scrollbar);
        }
    }
    if !layout.content.contains(p) {
        return None;
    }
    for (i, (row, rl)) in layout.rows.iter().enumerate() {
        if !rl.rect.contains(p) {
            continue;
        }
        match row.control {
            Control::Toggle { .. } | Control::Hotkey { .. } | Control::Button { .. } => {
                return Some(if rl.hit.contains(p) {
                    Hit::Press(i)
                } else {
                    Hit::Row(i)
                });
            }
            Control::RingItem { index, .. } => {
                // 注意：`i` 是**行**下标，`index` 是**配置里的第几颗球**。
                // 这两者因为页首的 section/note 行并不相等，
                // 上下移和绑热键必须用 `index`，否则点第一颗球的 ↑ 会挪动第三颗。
                if rl.up.contains(p) {
                    return Some(Hit::RingMove(index, -1));
                }
                if rl.down.contains(p) {
                    return Some(Hit::RingMove(index, 1));
                }
                if rl.hotkey.contains(p) {
                    return Some(Hit::RingHotkey(index));
                }
                return Some(if rl.hit.contains(p) {
                    Hit::Press(i)
                } else {
                    Hit::Row(i)
                });
            }
            // 文本框：只有框里才开编辑，点行别处不动（免得手滑丢焦点）。
            Control::Text { .. } => return Some(Hit::Press(i)),
            Control::Step { .. } | Control::Choice { .. } => {
                if rl.dec.contains(p) {
                    return Some(Hit::Step(i, -1));
                }
                if rl.inc.contains(p) {
                    return Some(Hit::Step(i, 1));
                }
                return Some(Hit::Row(i));
            }
            Control::Gallery { count, .. } => {
                let (cols, cell_w) = gallery_geom(rl.hit, count);
                for dot in 0..count {
                    if gallery_cell(rl.hit, dot, cols, cell_w).contains(p) {
                        return Some(Hit::GalleryDot(dot));
                    }
                }
                return Some(Hit::Row(i));
            }
            _ => return Some(Hit::Row(i)),
        }
    }
    None
}

// ── 应用一次编辑 ──────────────────────────────────────────

/// 用户在某一行上按了"激活"（空格/回车/点击控件）。
pub fn activate(model: &mut Model, layout: &Layout, index: usize) -> Vec<ChangeKind> {
    let Some((row, _)) = layout.rows.get(index) else {
        return Vec::new();
    };
    let control = row.control.clone();
    // 按钮行没有 field：先处理掉，否则 `let Some(field)` 会把它挡在外面。
    if let Control::Button { action, .. } = control {
        // 点按钮=离开正在编辑的框：先把缓冲写回去，不然用户填了一半就没了。
        let committed = commit_edit(model);
        match action {
            Action::OpenConfig => {
                zouwu_platform::open_config_dir();
            }
            Action::OpenLogs => {
                zouwu_platform::logging::open_log_dir();
            }
            Action::Reset => reset(model),
            Action::ImportSkin => return import_skin(model),
            Action::ExportSkin => {
                export_skin(model);
            }
            Action::OpenSkinsDir => zouwu_platform::assets::reveal_skins_dir(),
            Action::DeleteSkin => return delete_skin(model),
            Action::ImportFont => return import_font(model),
            Action::DeleteFont => return delete_font(model),
            Action::OpenFontsDir => zouwu_platform::fontpack::reveal_dir(),
            Action::ClearHistory => {
                // 不可撤销，所以先问一次。问完直接写一个空的历史文件：
                // 面板不掌握运行期数据，真正把内存里的清掉靠宿主收到
                // `ChangeKind::Clipboard` 后重读磁盘（同一套机制管着开关与容量）。
                if zouwu_platform::dialogs::confirm(
                    &model.i18n.t("settings.clipboard.clear"),
                    &model.i18n.t("settings.clipboard.clear.confirm"),
                ) {
                    let empty = zouwu_core::history::History::new(model.config.clipboard.limit);
                    zouwu_platform::save_history(&empty);
                    return vec![ChangeKind::Clipboard];
                }
            }
            Action::ToggleOverwrite => {
                model.overwrite_skin = !model.overwrite_skin;
            }
            Action::ToggleOverwriteHelp => {
                model.overwrite_help_open = !model.overwrite_help_open;
                // 展开后内容变高，滚动位置要跟着重算；收起时若停在底部，
                // 就把滚动拉回去，避免出现"底下多出一大片空白"。
                model.scroll = 0.0;
            }
            Action::AddItem => {
                if model.config.ring.items.len() >= RingCfg::MAX_ITEMS {
                    let max = RingCfg::MAX_ITEMS.to_string();
                    let notice = model.i18n.t_args("settings.bubbles.full", &[("max", &max)]);
                    model.notice = Some(notice);
                    return Vec::new();
                }
                let id = unique_custom_id(&model.config.ring.items);
                let index = model.config.ring.items.len();
                model.config.ring.items.push(RingItem::custom(&id));
                // 新球的名字/命令还是空的：直接把光标停到“名称”上，
                // 用户下一步必然是填它。
                begin_edit(model, EditTarget::ItemLabel(index));
                return vec![ChangeKind::Ring];
            }
            Action::AddProgram => {
                let Some(path) = zouwu_platform::dialogs::pick_program() else {
                    return committed.unwrap_or_default();
                };
                if model.config.ring.items.len() >= RingCfg::MAX_ITEMS {
                    let max = RingCfg::MAX_ITEMS.to_string();
                    let notice = model.i18n.t_args("settings.bubbles.full", &[("max", &max)]);
                    model.notice = Some(notice);
                    return committed.unwrap_or_default();
                }
                let id = unique_custom_id(&model.config.ring.items);
                let mut item = RingItem::custom(&id);
                // 名字默认取文件名（不带扩展名）：用户看到“WeChat”比看到
                // “custom.3”更知道自己加了个什么东西。
                item.label = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                item.command = path.display().to_string();
                model.config.ring.items.push(item);
                let mut kinds = committed.unwrap_or_default();
                kinds.push(ChangeKind::Ring);
                return kinds;
            }
            Action::PickIcon(index) => {
                let Some(path) = zouwu_platform::dialogs::pick_icon_image() else {
                    return committed.unwrap_or_default();
                };
                match zouwu_platform::assets::import_icon(&path) {
                    Ok(name) => {
                        if let Some(item) = model.config.ring.items.get_mut(index) {
                            item.icon = name;
                        }
                        let mut kinds = committed.unwrap_or_default();
                        kinds.push(ChangeKind::Ring);
                        return kinds;
                    }
                    Err(err) => {
                        zouwu_platform::dialogs::warn(
                            &model.i18n.t("settings.bubbles.icon.failed"),
                            &err,
                        );
                    }
                }
            }
            Action::RemoveItem(index) => {
                let Some(item) = model.config.ring.items.get(index) else {
                    return committed.unwrap_or_default();
                };
                let name = item.display_name().to_string();
                // 删一行是不可撤销的，而且环上的顺序是用户亲手排的，所以问一次。
                if !zouwu_platform::dialogs::confirm(
                    &model.i18n.t("settings.bubbles.remove"),
                    &name,
                ) {
                    return committed.unwrap_or_default();
                }
                let mut kinds = committed.unwrap_or_default();
                kinds.extend(remove_item(model, index));
                return kinds;
            }
        }
        return committed.unwrap_or_default();
    }
    let Some(field) = row.field else {
        // 没有 field 的复合控件：气泡球行和文本框。
        match control {
            Control::RingItem { index, enabled, .. } => {
                let committed = commit_edit(model);
                if let Some(item) = model.config.ring.items.get_mut(index) {
                    item.enabled = !enabled;
                }
                let mut kinds = committed.unwrap_or_default();
                kinds.push(ChangeKind::Ring);
                return kinds;
            }
            Control::Text { target, .. } => {
                begin_edit(model, target);
            }
            _ => {}
        }
        return Vec::new();
    };
    match control {
        Control::Toggle { on } => {
            field.set_bool(&mut model.config, !on);
            changed(model, field)
        }
        Control::Hotkey { .. } => {
            // 互斥：正在录某个球的键时又去点总开关，应该切换成总开关。
            model.capturing = match model.capturing {
                Some(CaptureTarget::Summon) => None,
                _ => Some(CaptureTarget::Summon),
            };
            Vec::new()
        }
        Control::Choice { .. } => step(model, field, 1),
        Control::Step { .. } => {
            // 空格激活一个数值行没有意义（± 才是操作），但键盘用户按到它时
            // 往前跳一步比什么都不发生更容易理解。
            step(model, field, 1)
        }
        _ => Vec::new(),
    }
}

/// 点缩略图 = 选中并立刻apply。返回 `ChangeKind::Appearance` 让宿主换皮。
pub fn select_skin(model: &mut Model, index: usize) -> Vec<ChangeKind> {
    let Some((id, _)) = model.skins.get(index) else {
        return Vec::new();
    };
    if *id == model.config.appearance.skin {
        return Vec::new();
    }
    model.config.appearance.skin = id.clone();
    vec![ChangeKind::Appearance]
}

/// 导入皮肤包。失败一律弹窗——这里没有第二个渠道能让用户知道发生了什么。
fn import_skin(model: &mut Model) -> Vec<ChangeKind> {
    let Some(path) = zouwu_platform::dialogs::pick_skin_package() else {
        return Vec::new();
    };
    // 勾了"覆盖"且确实会覆盖到东西时，再问一次：这一步是不可逆的文件删除。
    if model.overwrite_skin {
        if let Ok(id) = zouwu_platform::skinpack::peek_id(&path) {
            if zouwu_platform::skinpack::installed(&id)
                && !zouwu_platform::dialogs::confirm(
                    &model.i18n.t("settings.skin.overwrite.confirm"),
                    &id,
                )
            {
                return Vec::new();
            }
        }
    }
    match zouwu_platform::skinpack::import_with(&path, model.overwrite_skin) {
        Ok(id) => {
            model.refresh_skins();
            model.notice = Some(model.i18n.t_args("settings.skin.imported", &[("id", &id)]));
            model.config.appearance.skin = id;
            vec![ChangeKind::Appearance]
        }
        Err(err) => {
            // 最常见的一种失败值得一句话指路：同名皮肤已装，去勾上面那个框。
            let text = if err.contains("already installed") {
                format!(
                    "{err}

{}",
                    model.i18n.t("skin.import.overwrite.hint")
                )
            } else {
                err
            };
            zouwu_platform::dialogs::warn(&model.i18n.t("skin.import.failed"), &text);
            Vec::new()
        }
    }
}

fn export_skin(model: &Model) {
    let id = &model.config.appearance.skin;
    if id == PROCEDURAL_SKIN_ID {
        zouwu_platform::dialogs::warn(
            &model.i18n.t("skin.export.failed"),
            &model.i18n.t("skin.export.procedural"),
        );
        return;
    }
    let Some(path) = zouwu_platform::dialogs::pick_export_path(&format!("{id}.zwskin")) else {
        return;
    };
    if let Err(err) = zouwu_platform::skinpack::export_to(id, &path) {
        zouwu_platform::dialogs::warn(&model.i18n.t("skin.export.failed"), &err);
    }
}

/// 删除当前皮肤，并把配置退回代码绘制的那只。
fn delete_skin(model: &mut Model) -> Vec<ChangeKind> {
    let id = model.config.appearance.skin.clone();
    if id == PROCEDURAL_SKIN_ID {
        return Vec::new();
    }
    if !zouwu_platform::dialogs::confirm(&model.i18n.t("settings.skin.delete"), &id) {
        return Vec::new();
    }
    match zouwu_platform::skinpack::delete(&id) {
        Ok(()) => {
            model.config.appearance.skin = PROCEDURAL_SKIN_ID.to_string();
            model.refresh_skins();
            vec![ChangeKind::Appearance]
        }
        Err(err) => {
            zouwu_platform::dialogs::warn(&model.i18n.t("settings.skin.delete.failed"), &err);
            Vec::new()
        }
    }
}

fn import_font(model: &mut Model) -> Vec<ChangeKind> {
    let Some(path) = zouwu_platform::dialogs::pick_font_file() else {
        return Vec::new();
    };
    match zouwu_platform::fontpack::import(&path) {
        Ok(pack) => {
            // 立刻注册进本进程：不注册的话"换字体"要等下次启动才看得见。
            let dir = zouwu_platform::fontpack::fonts_dir().join(&pack.id);
            for file in &pack.files {
                zouwu_platform::text_gdi::add_font_file(&dir.join(file));
            }
            model.config.appearance.font_pack = pack.id.clone();
            model.refresh_fonts();
            // 面板自己的字也要立刻换（不然用户在一个旧字体里选新字体，看着像没生效）
            crate::text::set_family(Some(pack.family));
            vec![ChangeKind::Appearance]
        }
        Err(err) => {
            zouwu_platform::dialogs::warn(&model.i18n.t("settings.font.import.failed"), &err);
            Vec::new()
        }
    }
}

fn delete_font(model: &mut Model) -> Vec<ChangeKind> {
    let id = model.config.appearance.font_pack.clone();
    if id.is_empty() {
        return Vec::new();
    }
    let Some(pack) = zouwu_platform::fontpack::get(&id) else {
        return Vec::new();
    };
    let question = model.i18n.t_args(
        "settings.font.delete.confirm",
        &[("name", pack.display_name())],
    );
    if !zouwu_platform::dialogs::confirm(&model.i18n.t("settings.font.delete"), &question) {
        return Vec::new();
    }
    match zouwu_platform::fontpack::delete(&id) {
        Ok(()) => {
            model.config.appearance.font_pack = String::new();
            model.refresh_fonts();
            crate::text::set_family(None);
            vec![ChangeKind::Appearance]
        }
        Err(err) => {
            zouwu_platform::dialogs::warn(&model.i18n.t("settings.font.delete.failed"), &err);
            Vec::new()
        }
    }
}

/// 「装在哪」：便携运行 / 就是安装的那份 / 装过但当前在跑别的副本。
///
/// 三种状态都值得显示：用户从 zip 里双击时看到的应该是"便携运行"，
/// 而不是一个让人以为已经装好的路径。
fn install_label(i18n: &I18n) -> String {
    #[cfg(windows)]
    {
        if zouwu_platform::install::running_installed_copy() {
            return zouwu_platform::install::installed_exe()
                .display()
                .to_string();
        }
        if zouwu_platform::install::is_installed() {
            return i18n.t("settings.about.install.other");
        }
    }
    i18n.t("settings.about.install.portable")
}

/// 缩略图网格的列数与格子宽度。
///
/// **绘制与命中共用这一个函数**：各算一遍是"看到的位置点不到"的经典来源。
fn gallery_geom(rect: Rect, count: usize) -> (usize, f32) {
    const MAX_CELL: f32 = 132.0;
    let usable = rect.w.max(1.0);
    let cols = (((usable + GALLERY_GAP) / (MAX_CELL + GALLERY_GAP)).floor() as usize)
        .clamp(1, count.max(1));
    let cell_w = ((usable - GALLERY_GAP * (cols.saturating_sub(1)) as f32) / cols as f32)
        .clamp(48.0, MAX_CELL);
    (cols, cell_w)
}

fn gallery_cell(rect: Rect, index: usize, cols: usize, cell_w: f32) -> Rect {
    let cols = cols.max(1);
    let col = index % cols;
    let row = index / cols;
    // 左对齐：和上面「当前形象」的标签踩同一条竖线，比居中更像一张表。
    let x0 = rect.x;
    Rect::new(
        x0 + col as f32 * (cell_w + GALLERY_GAP),
        rect.y + row as f32 * (GALLERY_CELL_H + GALLERY_GAP),
        cell_w,
        GALLERY_CELL_H,
    )
}

/// `±` 一步（键盘 ←→ 或点击箭头）。
pub fn step(model: &mut Model, field: Field, dir: i32) -> Vec<ChangeKind> {
    if let Some(spec) = field.num_spec() {
        let current = field.num(&model.config).unwrap_or(spec.min);
        let next = (current + spec.step * dir as i64).clamp(spec.min, spec.max);
        if next != current {
            field.set_num(&mut model.config, next);
            return changed(model, field);
        }
        return Vec::new();
    }
    // 枚举：循环。
    let options = choice_options(model, field);
    if options.is_empty() {
        return Vec::new();
    }
    let current = current_choice(model, field);
    let idx = options
        .iter()
        .position(|(value, _)| *value == current)
        .unwrap_or(0);
    let len = options.len() as i32;
    let next = ((idx as i32 + dir) % len + len) % len;
    let (value, _) = options[next as usize].clone();
    match field {
        Field::Language => model.config.general.language = value,
        Field::Theme => model.config.appearance.theme = value,
        Field::Skin => model.config.appearance.skin = value,
        Field::FontPack => model.config.appearance.font_pack = value,
        Field::LogLevel => model.config.log.level = value,
        _ => return Vec::new(),
    }
    changed(model, field)
}

fn changed(model: &mut Model, field: Field) -> Vec<ChangeKind> {
    let mut kinds = Vec::new();
    match field {
        Field::Language => {
            model.i18n = I18n::new(&model.config.general.language);
            kinds.push(ChangeKind::Language);
        }
        Field::Hotkey => kinds.push(ChangeKind::Hotkey),
        Field::StartOnBoot => kinds.push(ChangeKind::Autostart),
        Field::Theme | Field::Skin | Field::FontPack => kinds.push(ChangeKind::Appearance),
        Field::LogLevel | Field::LogToFile => kinds.push(ChangeKind::Log),
        Field::AlwaysOnTop => kinds.push(ChangeKind::WindowLevel),
        Field::ClipboardEnabled | Field::ClipboardLimit => kinds.push(ChangeKind::Clipboard),
        Field::EyeCareEnabled | Field::EyeCareStrength => kinds.push(ChangeKind::EyeCare),
        Field::AltDragEnabled => kinds.push(ChangeKind::AltDrag),
        Field::WindowSize => kinds.push(ChangeKind::WindowSize),
        Field::BuddyRadius | Field::Breathe => kinds.push(ChangeKind::Buddy),
        Field::RingRadius
        | Field::OrbitStart
        | Field::BubbleRadius
        | Field::MaxPerRing
        | Field::RingGap
        | Field::FloatAmplitude
        | Field::ExpandStagger => kinds.push(ChangeKind::Ring),
        _ => kinds.push(ChangeKind::Interaction),
    }
    kinds
}

// ── 气泡球的顺序 / 热键 / 文本编辑 ────────

/// 新球的 id：`custom.1`、`custom.2`…（已存在就往下数）。
///
/// 不从“当前最大编号+1”算而是“第一个没被占用的”：用户删掉 custom.2 后
/// 再新增，应该重新拿到 custom.2，而不是跳号越跑越大。
fn unique_custom_id(items: &[RingItem]) -> String {
    for n in 1..=RingCfg::MAX_ITEMS * 2 {
        let id = format!("custom.{n}");
        if !items.iter().any(|it| it.id == id) {
            return id;
        }
    }
    // 实在排不下：用毫秒时间戳兜底，保证 id 唯一比好看重要。
    format!(
        "custom.{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    )
}

/// 把第 `index` 颗球上移（`dir = -1`）或下移（`+1`）。
///
/// **同时改配置与磁盘的持久位置**：`ring.items` 的顺序就是环上的展开顺序，
/// 不另存一份排序表——两份顺序早晚会对不上。
pub fn ring_move(model: &mut Model, index: usize, dir: i32) -> Vec<ChangeKind> {
    let len = model.config.ring.items.len();
    if index >= len || len < 2 {
        return Vec::new();
    }
    let target = index as i32 + dir;
    if target < 0 || target as usize >= len {
        return Vec::new();
    }
    let target = target as usize;
    model.config.ring.items.swap(index, target);
    // 正在编辑/录键的那颗球跟着换位置（身份不变，行号变了）。
    model.editing = None;
    model.capturing = None;
    vec![ChangeKind::Ring]
}

/// 开始录一个热键（总开关或某颗球）。
/// 从配置里删掉第 `index` 颗球。
///
/// 确认对话框留在调用方（`activate`）：那一步是系统模态窗，
/// 放进来这些规则就没法在测试里跑。
pub fn remove_item(model: &mut Model, index: usize) -> Vec<ChangeKind> {
    if index >= model.config.ring.items.len() {
        return Vec::new();
    }
    model.config.ring.items.remove(index);
    // 行号全变了：正在编辑/录键的目标必须先收掉，否则改的是别人。
    model.editing = None;
    model.capturing = None;
    vec![ChangeKind::Ring]
}

pub fn begin_capture(model: &mut Model, target: CaptureTarget) {
    commit_edit(model);
    model.capturing = Some(target);
}

/// 开始编辑某个文本框，光标停在末尾、缓冲取当前值。
pub fn begin_edit(model: &mut Model, target: EditTarget) {
    let buffer = edit_value(model, target);
    let cursor = buffer.len();
    model.editing = Some(Editing {
        target,
        cursor,
        anchor: cursor,
        buffer,
        offset: 0,
        width: f32::INFINITY,
        scale: 1.0,
    });
    // 同一个球只能干一件事：一边录键一边打字最后谁也不知道按了啥。
    model.capturing = None;
}

/// 本输入框的左右内边距（绘制与命中必须用同一份几何，差 1px 就点不准）。
pub const TEXT_PAD: f32 = 10.0;
/// 输入框字号（逻辑像素）。
pub const TEXT_PX: f32 = 12.0;

/// 输入框的内框（去掉左右内边距）。
pub fn text_inner(rect: Rect) -> Rect {
    Rect::new(
        rect.x + TEXT_PAD,
        rect.y,
        (rect.w - TEXT_PAD * 2.0).max(10.0),
        rect.h,
    )
}

/// 量一个字符串的宽度（逻辑像素）。`scale` 是当前的 DPI 缩放。
fn measure_text(text: &str, scale: f32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let px = TEXT_PX * scale;
    crate::text::TextEngine::with(|engine| engine.measure(text, px, false)) / scale
}

/// 量一段字节区间（`from..to`）的宽度。
///
/// 空区间要返回 **0.0**（不是 `measure_text("")` 那种“没量过就按 0 算”），
/// 光标正好落在可视起点时 x 必须是 0，否则光标会被画偏一个字。
fn width_between(text: &str, from: usize, to: usize, scale: f32) -> f32 {
    let from = clamp_boundary(text, from);
    let to = clamp_boundary(text, to).max(from);
    measure_text(&text[from..to], scale)
}

/// 整段文本的宽度（用来判断要不要出现滚动条）。
fn text_width(text: &str, scale: f32) -> f32 {
    width_between(text, 0, text.len(), scale)
}

/// 把光标拉回可视区：跑到左边就跟过去，跑到右边就往前吃字。
///
/// 只在“宽度量过了”之后才动 `offset`：没视口时改它只会让测试里的
/// 光标位置莫名其妙缩到 0。
fn ensure_caret_visible(editing: &mut Editing) {
    if !editing.width.is_finite() || editing.width <= 0.0 {
        return;
    }
    let (buffer, scale, avail) = (editing.buffer.clone(), editing.scale, editing.width);
    let cursor = clamp_boundary(&buffer, editing.cursor);
    let start = clamp_boundary(&buffer, editing.offset).min(cursor);
    if cursor < start {
        editing.offset = cursor;
        return;
    }
    if width_between(&buffer, start, cursor, scale) <= avail {
        // 光标本来就在框里，但可能因为删字而滚得太靠右了：往前收一收看能否贴回左端。
        editing.offset = start;
        return;
    }
    let mut start = cursor;
    while let Some(prev) = prev_boundary(&buffer, start) {
        if width_between(&buffer, prev, cursor, scale) > avail {
            break;
        }
        start = prev;
    }
    editing.offset = start;
}

/// 当前选区（字节区间，左闭右开）。没选区返回 `None`。
pub fn selection(editing: &Editing) -> Option<(usize, usize)> {
    let anchor = clamp_boundary(&editing.buffer, editing.anchor);
    let cursor = clamp_boundary(&editing.buffer, editing.cursor);
    if anchor == cursor {
        None
    } else {
        Some((anchor.min(cursor), anchor.max(cursor)))
    }
}

/// 有没有选中一段文本。
pub fn has_selection(model: &Model) -> bool {
    model
        .editing
        .as_ref()
        .is_some_and(|editing| selection(editing).is_some())
}

/// 把鼠标 x 换算成光标位置。
///
/// `extend = true`（拖拽、Shift+点击）时保留锚点，于是结果就是一段选区；
/// `extend = false`（普通单击）时锚点跟到新位置，选区收起来。
pub fn set_caret_from_x(model: &mut Model, inner: Rect, x: f32, extend: bool, scale: f32) {
    let Some(editing) = model.editing.as_mut() else {
        return;
    };
    // 点进来时顺手把视口量下来：光标换算要用“当前这次”的宽度与 DPI，
    // 用上一次的会让 125% 下每点一次都可能偏半个字。
    editing.width = inner.w.max(1.0);
    editing.scale = scale;
    let start = clamp_boundary(&editing.buffer, editing.offset);
    let local = (x - inner.x).max(0.0) + width_between(&editing.buffer, 0, start, scale);
    // 严格照字符边界走：多字节字符（中文、emoji）不能被切一半。
    let mut index = 0;
    let mut acc = 0.0;
    while let Some(next) = next_boundary(&editing.buffer, index) {
        let w = measure_text(&editing.buffer[index..next], scale);
        if acc + w * 0.5 > local {
            break;
        }
        acc += w;
        index = next;
    }
    if !extend {
        editing.anchor = index;
    }
    editing.cursor = index;
    ensure_caret_visible(editing);
}

/// 全选（Ctrl+A）。
pub fn select_all(model: &mut Model) {
    if let Some(editing) = model.editing.as_mut() {
        editing.anchor = 0;
        editing.cursor = editing.buffer.len();
    }
    sync_edit_viewport(model);
}

/// 告诉模型“输入框有多宽、现在什么 DPI”。布局每算一次就调一次。
pub fn set_edit_viewport(model: &mut Model, inner: Rect, scale: f32) {
    if let Some(editing) = model.editing.as_mut() {
        editing.width = inner.w.max(1.0);
        editing.scale = scale;
        ensure_caret_visible(editing);
    }
}

/// 让当前编辑框的视口跟布局（布局层自己不知道鼠标，只能由窗口层转告）。
///
/// `layout()` 不往 `Model` 里写（它只读）：所以这里单独按目标字段去 `build_rows`
/// 里找一遍行矩形。找不到（比如已经切到别的标签页）就什么都不改。
pub fn sync_edit_viewport(model: &mut Model) {
    // 没量过宽度就等下一次点击/布局：单测里不该凭空造一个视口。
    let Some(width) = model.editing.as_ref().map(|e| e.width) else {
        return;
    };
    if !width.is_finite() {
        return;
    }
    if let Some(editing) = model.editing.as_mut() {
        ensure_caret_visible(editing);
    }
}

/// 输入框里的内容是否宽到放不下（放不下才画滚动条）。
pub fn text_overflow(editing: &Editing) -> bool {
    editing.width.is_finite() && text_width(&editing.buffer, editing.scale) > editing.width + 0.5
}

/// 单行框自己的水平滚动条（压在内框下沿，只有 5px 高，不抢正文的点击区）。
pub fn text_scroll_track(inner: Rect) -> Rect {
    Rect::new(inner.x, inner.bottom() - 5.0, inner.w, 5.0)
}

/// 水平滚动条的 thumb 矩形（与绘制用同一个公式）。
///
/// `pub` 是因为绘制在 `draw_row` 里（那里手边只有 `Painter` 和调色板），
/// 而命中判定在窗口层；两边必须拿同一个矩形，所以只算一次。
pub fn text_thumb(editing: &Editing, inner: Rect) -> Option<Rect> {
    if !text_overflow(editing) {
        return None;
    }
    let total = text_width(&editing.buffer, editing.scale);
    if total <= 0.0 {
        return None;
    }
    let ratio = (editing.width / total).clamp(0.0, 1.0);
    // 注意不能写 `.clamp(24.0, inner.w)`：内框比 24px 还窄时
    // `clamp` 会因为 min > max 直接 panic（窄窗口就是这种情形）。
    let thumb_w = (inner.w * ratio).clamp(inner.w.min(24.0), inner.w.max(0.0));
    let start = clamp_boundary(&editing.buffer, editing.offset);
    let scrolled = width_between(&editing.buffer, 0, start, editing.scale);
    let max_scroll = (total - editing.width).max(1.0);
    let x = inner.x + (inner.w - thumb_w) * (scrolled / max_scroll).clamp(0.0, 1.0);
    Some(Rect::new(x, inner.bottom() - 5.0, thumb_w, 5.0))
}

/// 拖动水平滚动条：把 viewport 的**左沿**放到鼠标那里（拖动的是那段 thumb）。
pub fn set_offset_from_x(model: &mut Model, inner: Rect, x: f32) {
    let Some(editing) = model.editing.as_mut() else {
        return;
    };
    if !text_overflow(editing) {
        return;
    }
    let total = text_width(&editing.buffer, editing.scale);
    let max_scroll = (total - editing.width).max(1.0);
    let ratio = ((x - inner.x) / inner.w.max(1.0)).clamp(0.0, 1.0);
    let want = (ratio * max_scroll).clamp(0.0, max_scroll);
    // 找一个“宽度刚好等于 want”的字符边界：只能滚到字缝上，滚到字中间会让字被切一半。
    let mut index = 0;
    let mut acc = 0.0;
    while let Some(next) = next_boundary(&editing.buffer, index) {
        let w = measure_text(&editing.buffer[index..next], editing.scale);
        if acc + w > want {
            break;
        }
        acc += w;
        index = next;
    }
    editing.offset = index;
    // 不碰光标与选区：拖滚动条只是“看看别处”，不是“把光标挪过去”，
    // 直接改 cursor 会把用户刚选好的选区当场抹掉。
    // 光标真在可视区外时由绘制层把它限到框边。
}

/// 鼠标滚轮在输入框上时横着滚。
pub fn scroll_edit(model: &mut Model, dx: f32) {
    let Some(editing) = model.editing.as_mut() else {
        return;
    };
    if !text_overflow(editing) || dx.abs() < 0.01 {
        return;
    }
    let start = clamp_boundary(&editing.buffer, editing.offset);
    let mut index = start;
    if dx > 0.0 {
        let mut left = dx;
        while left > 0.0 {
            let Some(next) = next_boundary(&editing.buffer, index) else {
                break;
            };
            left -= measure_text(&editing.buffer[index..next], editing.scale);
            index = next;
        }
    } else {
        let mut left = -dx;
        while left > 0.0 {
            let Some(prev) = prev_boundary(&editing.buffer, index) else {
                break;
            };
            left -= measure_text(&editing.buffer[prev..index], editing.scale);
            index = prev;
        }
    }
    editing.offset = index;
}

/// 拖动滚动条 / 点滚动条时，这个点是不是落在 thumb 上（窗口层判命中用）。
pub fn text_scrollbar_at(editing: &Editing, inner: Rect, p: Point) -> bool {
    text_thumb(editing, inner)
        .is_some_and(|thumb| Rect::new(thumb.x, inner.bottom() - 7.0, thumb.w, 9.0).contains(p))
}

/// 双击选词。中文按“连续同类别”分（汉字一串、字母数字一串），
/// 路径/命令这类用分割符划开：双击 `C:\work\a.txt` 应该选中整条路径，
/// 而不是只选中 `work`（护眼宝那种“双击选一截”在这里没用）。
pub fn select_word(model: &mut Model, inner: Rect, x: f32, scale: f32) {
    let Some(editing) = model.editing.as_mut() else {
        return;
    };
    editing.width = inner.w.max(1.0);
    editing.scale = scale;
    let start = clamp_boundary(&editing.buffer, editing.offset);
    let local = (x - inner.x).max(0.0) + width_between(&editing.buffer, 0, start, scale);
    let mut index = 0;
    let mut acc = 0.0;
    while let Some(next) = next_boundary(&editing.buffer, index) {
        let w = measure_text(&editing.buffer[index..next], scale);
        if acc + w * 0.5 > local {
            break;
        }
        acc += w;
        index = next;
    }
    let word = |editing: &Editing, i: usize| -> Option<char> { editing.buffer[i..].chars().next() };
    // 点到字缝右边（index 已在下一个字上）：往左看一个字。
    let mut lo = index;
    if lo == editing.buffer.len() {
        lo = prev_boundary(&editing.buffer, lo).unwrap_or(0);
    }
    let Some(ch) = word(editing, lo) else {
        return;
    };
    if ch.is_whitespace() {
        // 点空白：选中这段空白（跟系统输入框一样，双击空白选空白）。
        let mut a = lo;
        while let Some(prev) = prev_boundary(&editing.buffer, a) {
            if !word(editing, prev).is_some_and(|c| c.is_whitespace()) {
                break;
            }
            a = prev;
        }
        let mut b = next_boundary(&editing.buffer, lo).unwrap_or(lo);
        while word(editing, b).is_some_and(|c| c.is_whitespace()) {
            b = next_boundary(&editing.buffer, b).unwrap_or(b);
        }
        editing.anchor = a;
        editing.cursor = b;
        ensure_caret_visible(editing);
        return;
    }
    // 路径/命令分隔符也开新词（但分隔符本身不算词）。
    let is_break = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '/' | '\\'
                    | ':'
                    | ';'
                    | ','
                    | '|'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '"'
                    | '\''
                    | '='
                    | '<'
                    | '>'
                    | '&'
            )
    };
    let lo = if is_break(ch) {
        lo
    } else {
        let mut a = lo;
        while let Some(prev) = prev_boundary(&editing.buffer, a) {
            if word(editing, prev).is_none_or(is_break) {
                break;
            }
            a = prev;
        }
        a
    };
    let mut hi = next_boundary(&editing.buffer, lo).unwrap_or(lo);
    if !is_break(ch) {
        while word(editing, hi).is_some_and(|c| !is_break(c)) {
            hi = next_boundary(&editing.buffer, hi).unwrap_or(hi);
        }
    }
    editing.anchor = lo;
    editing.cursor = hi;
    ensure_caret_visible(editing);
}

/// 复制选区（没选区就复制整段）。返回是否真拷到了。
pub fn copy_selection(model: &Model) -> bool {
    let Some(editing) = model.editing.as_ref() else {
        return false;
    };
    let text = match selection(editing) {
        Some((a, b)) => &editing.buffer[a..b],
        None => editing.buffer.as_str(),
    };
    if text.is_empty() {
        return false;
    }
    host_clipboard_set(text)
}

/// 剪切选区（没选区就什么都不干：整段剪切等于“清空”，太危险）。
pub fn cut_selection(model: &mut Model) -> bool {
    let Some(editing) = model.editing.as_mut() else {
        return false;
    };
    let Some((a, b)) = selection(editing) else {
        return false;
    };
    if !host_clipboard_set(&editing.buffer[a..b]) {
        return false;
    }
    delete_selection(editing);
    ensure_caret_visible(editing);
    true
}

/// 粘贴。多个换行会被折成空格（单行框）：直接插 `\n` 进去会让 frame_text
/// 画出两行，而内框只裁上下，字符会溢出到面板别的地方。
pub fn paste(model: &mut Model, raw: &str) -> bool {
    let mut cleaned = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\r' => {}
            '\n' | '\t' => cleaned.push(' '),
            c if c.is_control() => {}
            c => cleaned.push(c),
        }
    }
    if cleaned.is_empty() {
        return false;
    }
    edit_text(model, &cleaned);
    true
}

/// 从系统剪贴板粘贴。
pub fn paste_clipboard(model: &mut Model) -> bool {
    let Some(text) = host_clipboard_get() else {
        return false;
    };
    paste(model, &text)
}

/// 写入系统剪贴板。
fn host_clipboard_set(text: &str) -> bool {
    zouwu_platform::clipboard::set_text(text)
}

/// 读系统剪贴板。
fn host_clipboard_get() -> Option<String> {
    zouwu_platform::clipboard::get_text()
}

/// 删掉选区；返回是否真的删了。
fn delete_selection(editing: &mut Editing) -> bool {
    let Some((start, end)) = selection(editing) else {
        return false;
    };
    editing.buffer.replace_range(start..end, "");
    editing.anchor = start;
    editing.cursor = start;
    true
}

fn edit_value(model: &Model, target: EditTarget) -> String {
    match target {
        EditTarget::ItemLabel(i) => model.config.ring.items.get(i).map(|it| it.label.clone()),
        EditTarget::ItemCommand(i) => model.config.ring.items.get(i).map(|it| it.command.clone()),
    }
    .unwrap_or_default()
}

fn set_edit_value(model: &mut Model, target: EditTarget, value: String) {
    match target {
        EditTarget::ItemLabel(i) => {
            if let Some(item) = model.config.ring.items.get_mut(i) {
                item.label = value;
            }
        }
        EditTarget::ItemCommand(i) => {
            if let Some(item) = model.config.ring.items.get_mut(i) {
                item.command = value;
            }
        }
    }
}

/// 写回正在编辑的文本。返回 `Some(kinds)` 表示真的变了（宿主需要落盘）。
pub fn commit_edit(model: &mut Model) -> Option<Vec<ChangeKind>> {
    let editing = model.editing.take()?;
    let old = edit_value(model, editing.target);
    if old == editing.buffer {
        return None;
    }
    set_edit_value(model, editing.target, editing.buffer);
    Some(vec![ChangeKind::Ring])
}

/// 把一段文本插到光标处（输入法上屏、逐字符输入都走这里）。
/// 有选区时先删选区再插（和系统输入框一致：选中后打字 = 替换）。
pub fn edit_text(model: &mut Model, text: &str) {
    let Some(editing) = model.editing.as_mut() else {
        return;
    };
    delete_selection(editing);
    let cursor = clamp_boundary(&editing.buffer, editing.cursor);
    editing.buffer.insert_str(cursor, text);
    editing.cursor = cursor + text.len();
    editing.anchor = editing.cursor;
    ensure_caret_visible(editing);
}

/// 光标移动（方向键 / Home / End）。`extend` 即按住 Shift 拉选区。
pub fn edit_move(model: &mut Model, motion: EditMotion, extend: bool) {
    let Some(editing) = model.editing.as_mut() else {
        return;
    };
    editing.cursor = clamp_boundary(&editing.buffer, editing.cursor);
    editing.anchor = clamp_boundary(&editing.buffer, editing.anchor);
    if extend {
        editing.cursor = match motion {
            EditMotion::Left => prev_boundary(&editing.buffer, editing.cursor).unwrap_or(0),
            EditMotion::Right => {
                next_boundary(&editing.buffer, editing.cursor).unwrap_or(editing.buffer.len())
            }
            EditMotion::Home => 0,
            EditMotion::End => editing.buffer.len(),
        };
        ensure_caret_visible(editing);
        return;
    }
    // 没按 Shift 时方向键先“收起”选区（← 去左端、→ 去右端），与系统输入框一致。
    let target = if let Some((start, end)) = selection(editing) {
        match motion {
            EditMotion::Left => start,
            EditMotion::Right => end,
            EditMotion::Home => 0,
            EditMotion::End => editing.buffer.len(),
        }
    } else {
        match motion {
            EditMotion::Left => prev_boundary(&editing.buffer, editing.cursor).unwrap_or(0),
            EditMotion::Right => {
                next_boundary(&editing.buffer, editing.cursor).unwrap_or(editing.buffer.len())
            }
            EditMotion::Home => 0,
            EditMotion::End => editing.buffer.len(),
        }
    };
    editing.cursor = target;
    editing.anchor = target;
    ensure_caret_visible(editing);
}

/// 编辑框的按键处理。`Commit` / `Cancel` 返回 `Some`，其余返回 `None`。
pub fn edit_key(model: &mut Model, key: EditKey) -> Option<Vec<ChangeKind>> {
    match key {
        EditKey::Commit => return commit_edit(model),
        EditKey::Cancel => {
            model.editing = None;
            return None;
        }
        _ => {}
    }
    let Some(editing) = model.editing.as_mut() else {
        return None;
    };
    editing.cursor = clamp_boundary(&editing.buffer, editing.cursor);
    editing.anchor = clamp_boundary(&editing.buffer, editing.anchor);
    let cursor = editing.cursor;
    match key {
        EditKey::Backspace => {
            if !delete_selection(editing) {
                if let Some(prev) = prev_boundary(&editing.buffer, cursor) {
                    editing.buffer.replace_range(prev..cursor, "");
                    editing.cursor = prev;
                    editing.anchor = prev;
                }
            }
        }
        EditKey::Delete => {
            if !delete_selection(editing) {
                if let Some(next) = next_boundary(&editing.buffer, cursor) {
                    editing.buffer.replace_range(cursor..next, "");
                }
            }
        }
        EditKey::Commit | EditKey::Cancel => unreachable!(),
    }
    ensure_caret_visible(editing);
    None
}

/// 把下标夹到最近的字符边界上（防止多字节字符被切半个）。
fn clamp_boundary(text: &str, index: usize) -> usize {
    let mut i = index.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn prev_boundary(text: &str, index: usize) -> Option<usize> {
    if index == 0 {
        return None;
    }
    let mut i = index - 1;
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    Some(i)
}

fn next_boundary(text: &str, index: usize) -> Option<usize> {
    if index >= text.len() {
        return None;
    }
    let mut i = index + 1;
    while i < text.len() && !text.is_char_boundary(i) {
        i += 1;
    }
    Some(i)
}

fn reset(model: &mut Model) {
    let defaults = Config::default();
    // 语言、热键、自启、日志属于"运行方式"，重置外观不该把用户的语言也改回英文。
    model.config.appearance = defaults.appearance;
    model.config.interaction = defaults.interaction;
    // 环的几何回默认，但**气泡球清单保留**：
    // 「重置外观」不该把用户一颗一颗加好、排好序、配好热键的球全删了。
    let kept = std::mem::take(&mut model.config.ring.items);
    model.config.ring = defaults.ring;
    model.config.ring.items = kept;
    model.config.buddy = defaults.buddy;
    model.config.window = defaults.window;
    // 护眼是“屏幕观感”：重置外观时一并回到默认（关掉、强度 40）。
    model.config.eye_care = defaults.eye_care;
    // Alt 拖动窗口是“操作方式”而不是外观，重置不动它：
    // 把用户的鼠标习惯改回去，比“重置得不彻底”更讨厌。
    model.notice = Some(model.i18n.t("settings.reset.done"));
}

// ── 热键捕获 ──────────────────────────────────────────────

/// 返回 `Some(Ok(()))` 表示热键已更新，`Some(Err(msg))` 表示这组键不能用。
/// 录到一个组合键：按 `CaptureTarget` 写回对应位置。
///
/// 调用方（窗口层）只负责“用户按了什么”，知道“写给谁”的是这里 ——
/// 两面各管一半，比把下标/字段名散在窗口层可靠。
pub fn capture_hotkey(model: &mut Model, hotkey: Option<Hotkey>) -> Option<Vec<ChangeKind>> {
    let target = model.capturing?;
    let hotkey = hotkey?;
    model.capturing = None;
    match target {
        CaptureTarget::Summon => {
            // 直接存捕获到的组合。不再回写字符串再解析一遍——那只是给"字符串格式"
            // 制造第二次出错的机会。
            model.config.general.hotkey = hotkey.to_string();
            model.notice = None;
            Some(vec![ChangeKind::Hotkey])
        }
        CaptureTarget::Item(index) => {
            let text = hotkey.to_string();
            let id = model.config.ring.items.get(index)?.id.clone();
            // 撞车当场拒：两个球绑同一个键，按下之后只能有一个响应，
            // 而用户完全无从得知是哪个引胜。
            if !text.trim().is_empty() {
                if let Some(other) = model
                    .config
                    .ring
                    .items
                    .iter()
                    .position(|it| it.id != id && it.hotkey.trim() == text.trim())
                {
                    let name = model
                        .config
                        .ring
                        .items
                        .get(other)
                        .map(|it| it.display_name().to_string())
                        .unwrap_or_default();
                    let notice = model
                        .i18n
                        .t_args("settings.bubbles.hotkey.taken", &[("name", &name)]);
                    model.notice = Some(notice);
                    return None;
                }
            }
            if let Some(item) = model.config.ring.items.get_mut(index) {
                item.hotkey = text;
            }
            model.notice = None;
            Some(vec![ChangeKind::Ring])
        }
    }
}

/// 清掉正在录的那个热键（用户在录键框里按了回退/删）。
pub fn clear_hotkey(model: &mut Model) -> Option<Vec<ChangeKind>> {
    let target = model.capturing.take()?;
    match target {
        CaptureTarget::Summon => {
            model.config.general.hotkey.clear();
            Some(vec![ChangeKind::Hotkey])
        }
        CaptureTarget::Item(index) => {
            let item = model.config.ring.items.get_mut(index)?;
            item.hotkey.clear();
            Some(vec![ChangeKind::Ring])
        }
    }
}

/// 捕获失败时给用户一句人话（`Hotkey` 的 Display 已经是这个口径）。
pub fn describe_hotkey_error(err: &HotkeyError) -> String {
    err.to_string()
}

// ── 绘制 ──────────────────────────────────────────────────

struct Palette {
    bg: Rgba,
    raised: Rgba,
    text: Rgba,
    dim: Rgba,
    muted: Rgba,
    line: Rgba,
    accent: Rgba,
    on_accent: Rgba,
}

fn palette(theme: &Theme) -> Palette {
    Palette {
        bg: theme.bubble_bg,
        raised: theme.bubble_bg_active,
        text: theme.glyph,
        dim: theme.glyph.with_alpha(0.72),
        muted: theme.glyph.with_alpha(0.45),
        line: theme.bubble_edge.with_alpha(0.55),
        accent: theme.accent,
        on_accent: theme.bubble_bg,
    }
}

pub fn draw(pixmap: &mut tiny_skia::Pixmap, layout: &Layout, model: &Model) {
    let theme = model.theme();
    let pal = palette(&theme);
    let scale = pixmap.width() as f32 / layout.w.max(1.0);
    let mut p = Painter::new(pixmap, scale);

    // 背景
    p.fill(Rect::new(0.0, 0.0, layout.w, layout.h), 0.0, pal.bg);
    // 标签页
    for (tab, rect) in &layout.tabs {
        let active = *tab == model.tab;
        // 选中的标签用**实心强调色**：面板内容差异很大，一个浅底不足以说明
        // "你现在在哪一页"，尤其对色弱用户。
        if active {
            p.fill(*rect, 8.0, pal.accent);
        }
        let label = model.i18n.t(tab.key());
        p.text_center(
            &label,
            *rect,
            13.0,
            if active { pal.on_accent } else { pal.dim },
            if active {
                Weight::Bold
            } else {
                Weight::Regular
            },
        );
    }
    p.hline(MARGIN, layout.w - MARGIN, HEADER_H - 1.0, pal.line);

    // 内容（裁剪到可视带）
    p.set_clip(layout.content);
    for (i, (row, rl)) in layout.rows.iter().enumerate() {
        // 只画基本完整的行：被内容带边缘切掉一半的行，宁可整行不画
        // （画一半的文字比留白更像"渲染坏了"）。滚动已吸附到行边界，
        // 正常情况下这里最多有一行被跳过。
        let visible = rl
            .rect
            .intersection(&layout.content)
            .map(|r| r.h)
            .unwrap_or(0.0);
        // 整行可见才画：半行会被内容带的裁剪线从字中间切开。
        // 顶部边缘已经被 `snap_scroll` 吸附到行边界，所以正常情况下
        // 只有最下面那一行会因此不画。
        if visible + 0.5 < rl.rect.h {
            continue;
        }
        let focused = model.focus == Some(i) && model.keyboard_used;
        if focused {
            p.fill(rl.rect.inset(0.0, 2.0), 8.0, pal.accent.with_alpha(0.12));
        } else if model.hover == Some(i) && row.focusable() {
            // 悬停只给一点点底色，别和"选中"抢注意力。
            p.fill(rl.rect.inset(0.0, 2.0), 8.0, pal.raised.with_alpha(0.55));
        }
        draw_row(&mut p, &pal, model, row, rl, layout.notes.get(i));
    }

    // 页脚与滚动条不受内容裁剪影响。
    p.clear_clip();

    // 滚动条
    if let Some((track, thumb)) = &layout.scrollbar {
        p.fill(*track, 2.0, pal.line);
        p.fill(*thumb, 2.0, pal.accent.with_alpha(0.6));
    }

    // 底部：说明 + 临时提示
    p.hline(MARGIN, layout.w - MARGIN, layout.h - FOOTER_H, pal.line);
    let footer_text = model
        .notice
        .clone()
        .unwrap_or_else(|| model.i18n.t("settings.auto.saved"));
    let footer_text = p.truncate(&footer_text, 12.0, layout.footer.w - 200.0);
    p.text_in(
        &footer_text,
        layout.footer,
        12.0,
        pal.muted,
        Weight::Regular,
    );
    let hint = model.i18n.t("settings.footer.keys");
    p.text_right(&hint, layout.footer, 11.0, pal.muted, Weight::Regular);
}

fn draw_row(
    p: &mut Painter<'_>,
    pal: &Palette,
    model: &Model,
    row: &Row,
    rl: &RowLayout,
    note: Option<&Vec<String>>,
) {
    let label_rect = Rect::new(
        rl.rect.x + 4.0,
        rl.rect.y,
        (rl.control.x - rl.rect.x - 16.0).max(40.0),
        rl.rect.h,
    );
    if let Control::Section { text } = &row.control {
        // 文字活在一段明确的带子里（上留白之下、分隔线之上），居中；
        // 这样"字压线"就不会再因为行高调整而回归。
        let rule_y = rl.rect.bottom() - SECTION_RULE_INSET;
        let band = Rect::new(
            rl.rect.x + 4.0,
            rl.rect.y + SECTION_TOP_GAP,
            (rl.rect.w - 8.0).max(40.0),
            (rule_y - rl.rect.y - SECTION_TOP_GAP - 4.0).max(p.line_height(11.0)),
        );
        let text = p.truncate(text, 11.0, band.w);
        p.text_in(&text, band, 11.0, pal.muted, Weight::Bold);
        p.hline(
            rl.rect.x + 4.0,
            rl.rect.right(),
            rule_y,
            pal.line.with_alpha(0.3),
        );
        return;
    }

    match (row.label.is_empty(), row.hint.as_deref()) {
        // 只有提示的行（删除按钮等）：提示自己就是这一行的说明文字。
        (true, Some(hint)) if matches!(row.control, Control::Gallery { .. }) => {
            // 缩略图墙占满了整行，说明只能待在墙脚。
            let h = p.line_height(11.0);
            let rect = Rect::new(label_rect.x, rl.rect.bottom() - h - 2.0, label_rect.w, h);
            let hint = p.truncate(hint, 11.0, rect.w);
            p.text_in(&hint, rect, 11.0, pal.muted, Weight::Regular);
        }
        (true, Some(hint)) => {
            let hint = p.truncate(hint, 11.0, label_rect.w);
            p.text_in(&hint, label_rect, 11.0, pal.muted, Weight::Regular);
        }
        (true, None) => {}
        (false, None) => {
            let label = p.truncate(&row.label, 14.0, label_rect.w);
            p.text_in(&label, label_rect, 14.0, pal.text, Weight::Regular);
        }
        (false, Some(hint)) => {
            // 标签上、提示下，两行作为一个整体在行内垂直居中。
            //
            // 关键是"按行高算"而不是按行高的百分比算：字体度量一变（换字体、
            // 换 DPI、中英文混排），百分比布局就会让两行字互相压住，
            // 而按行高叠起来时，间隙始终是 HINT_GAP 那么多。
            let label_h = p.line_height(14.0);
            let hint_h = p.line_height(11.0);
            let block = label_h + HINT_GAP + hint_h;
            let top_y = rl.rect.y + (rl.rect.h - block) * 0.5;
            let top = Rect::new(label_rect.x, top_y, label_rect.w, label_h);
            let bottom = Rect::new(
                label_rect.x,
                top_y + label_h + HINT_GAP,
                label_rect.w,
                hint_h,
            );
            let label = p.truncate(&row.label, 14.0, top.w);
            p.text_in(&label, top, 14.0, pal.text, Weight::Regular);
            let hint = p.truncate(hint, 11.0, bottom.w);
            p.text_in(&hint, bottom, 11.0, pal.muted, Weight::Regular);
        }
    }

    match &row.control {
        Control::Section { .. } => {}
        Control::Note { .. } => {
            // 说明是"读的东西"，不是"点的东西"：正常字色、正常行距、不高亮。
            // 行已经在布局阶段折好，这里只负责按 `NOTE_LINE_H` 逐行落地 ——
            // 再折一次会让行数与行高对不上，正是要避免的那类错位。
            let lines = note.map(|l| l.as_slice()).unwrap_or(&[]);
            let mut y = rl.rect.y + NOTE_PAD;
            for line in lines {
                let band = Rect::new(rl.rect.x + 4.0, y, rl.rect.w - 8.0, NOTE_LINE_H);
                p.text_in(line, band, 11.0, pal.text.with_alpha(0.86), Weight::Regular);
                y += NOTE_LINE_H;
            }
        }
        Control::Toggle { on } => {
            let r = rl.hit;
            let fill = if *on { pal.accent } else { pal.line };
            p.fill(r, r.h * 0.5, fill);
            let knob = (r.h - 6.0) * 0.5;
            let cx = if *on {
                r.right() - 3.0 - knob
            } else {
                r.x + 3.0 + knob
            };
            p.circle(Point::new(cx, r.y + r.h * 0.5), knob, pal.text);
        }
        Control::Checkbox { on, .. } => {
            let r = rl.hit;
            let radius = 6.0;
            if *on {
                p.fill(r, radius, pal.accent);
                // 对勾：两段折线。比例按框宽的 0.24/0.52/0.78 取，
                // 换 DPI / 换字号都落在视觉中心。
                let (x0, y0) = (r.x + r.w * 0.24, r.y + r.h * 0.52);
                let (x1, y1) = (r.x + r.w * 0.43, r.y + r.h * 0.70);
                let (x2, y2) = (r.x + r.w * 0.78, r.y + r.h * 0.30);
                p.line(x0, y0, x1, y1, pal.on_accent, 2.0);
                p.line(x1, y1, x2, y2, pal.on_accent, 2.0);
            } else {
                p.fill(r, radius, pal.raised);
                p.stroke(r, radius, 1.0, pal.line);
            }
        }
        Control::Step {
            text,
            at_min,
            at_max,
        } => {
            p.fill(rl.control, 8.0, pal.raised);
            p.stroke(rl.control, 8.0, 1.0, pal.line.with_alpha(0.5));
            let minus = if *at_min { pal.muted } else { pal.dim };
            let plus = if *at_max { pal.muted } else { pal.dim };
            p.text_center("−", rl.dec, 15.0, minus, Weight::Bold);
            p.text_center("+", rl.inc, 15.0, plus, Weight::Bold);
            let value_rect = Rect::new(
                rl.dec.right(),
                rl.control.y,
                (rl.inc.x - rl.dec.right()).max(10.0),
                rl.control.h,
            );
            let shown = p.truncate(text, 13.0, value_rect.w - 8.0);
            p.text_center(&shown, value_rect, 13.0, pal.text, Weight::Regular);
        }
        Control::Choice { text } => {
            p.fill(rl.control, 8.0, pal.raised);
            p.stroke(rl.control, 8.0, 1.0, pal.line.with_alpha(0.5));
            p.text_center("‹", rl.dec, 15.0, pal.dim, Weight::Bold);
            p.text_center("›", rl.inc, 15.0, pal.dim, Weight::Bold);
            let value_rect = Rect::new(
                rl.dec.right(),
                rl.control.y,
                (rl.inc.x - rl.dec.right()).max(10.0),
                rl.control.h,
            );
            let shown = p.truncate(text, 13.0, value_rect.w - 6.0);
            p.text_center(&shown, value_rect, 13.0, pal.text, Weight::Regular);
        }
        Control::Hotkey { text, capturing } => {
            let r = rl.hit;
            let (fill, edge, fg) = if *capturing {
                (pal.accent.with_alpha(0.22), pal.accent, pal.text)
            } else {
                (pal.raised, pal.line.with_alpha(0.5), pal.text)
            };
            p.fill(r, 8.0, fill);
            p.stroke(r, 8.0, 1.0, edge);
            let shown = p.truncate(text, 13.0, r.w - 16.0);
            p.text_center(&shown, r, 13.0, fg, Weight::Bold);
        }
        Control::Button { text, .. } => {
            let r = rl.hit;
            p.fill(r, 8.0, pal.accent.with_alpha(0.16));
            p.stroke(r, 8.0, 1.0, pal.accent.with_alpha(0.6));
            let shown = p.truncate(text, 13.0, r.w - 16.0);
            p.text_center(&shown, r, 13.0, pal.text, Weight::Bold);
        }
        Control::RingItem {
            enabled,
            hotkey,
            capturing,
            ..
        } => {
            // 关掉的球不上环：整行控件一起变淡，而不是只把开关拨到暗色 ——
            // 否则用户会以为"开关关了但热键还在生效"。
            let dim = !*enabled;
            for (rect, up) in [(rl.up, true), (rl.down, false)] {
                let bg = pal.raised.with_alpha(if dim { 0.4 } else { 1.0 });
                p.fill(rect, 6.0, bg);
                p.stroke(rect, 6.0, 1.0, pal.line.with_alpha(0.5));
                let cx = rect.x + rect.w * 0.5;
                let cy = rect.y + rect.h * 0.5;
                let d = 4.0;
                let color = if dim { pal.dim } else { pal.text };
                // 箭头用两段线拼：不依赖字体里有没有 ▲▼（缺字时会是豆腐块）。
                if up {
                    p.line(cx - d, cy + d * 0.6, cx, cy - d * 0.6, color, 1.8);
                    p.line(cx, cy - d * 0.6, cx + d, cy + d * 0.6, color, 1.8);
                } else {
                    p.line(cx - d, cy - d * 0.6, cx, cy + d * 0.6, color, 1.8);
                    p.line(cx, cy + d * 0.6, cx + d, cy - d * 0.6, color, 1.8);
                }
            }
            let r = rl.hotkey;
            let (fill, edge, fg) = if *capturing {
                (pal.accent.with_alpha(0.22), pal.accent, pal.text)
            } else {
                (
                    pal.raised,
                    pal.line.with_alpha(0.5),
                    if dim { pal.dim } else { pal.text },
                )
            };
            p.fill(r, 8.0, fill);
            p.stroke(r, 8.0, 1.0, edge);
            let shown = p.truncate(hotkey, 12.0, r.w - 14.0);
            p.text_center(&shown, r, 12.0, fg, Weight::Bold);
            let t = rl.hit;
            p.fill(t, t.h * 0.5, if dim { pal.line } else { pal.accent });
            let knob = (t.h - 6.0) * 0.5;
            let cx = if dim {
                t.x + 3.0 + knob
            } else {
                t.right() - 3.0 - knob
            };
            p.circle(Point::new(cx, t.y + t.h * 0.5), knob, pal.text);
        }
        Control::Text {
            text,
            editing,
            placeholder,
            target,
        } => {
            let r = rl.hit;
            p.fill(r, 8.0, pal.raised);
            p.stroke(
                r,
                8.0,
                1.0,
                if *editing {
                    pal.accent
                } else {
                    pal.line.with_alpha(0.5)
                },
            );
            let inner = text_inner(r);
            // 行里只存了“是不是我在编辑”，光标/锚点要去 model 里取：
            // 同一个字段在另一行（另一个球）上编辑时不能把它的选区画到这里。
            let state = model.editing.as_ref().filter(|e| e.target == *target);
            if text.trim().is_empty() && !*editing {
                let shown = p.truncate(placeholder, TEXT_PX, inner.w);
                p.text_in(&shown, inner, TEXT_PX, pal.dim, Weight::Regular);
            } else if let Some(state) = state {
                let scale = p.scale();
                let start = clamp_boundary(&state.buffer, state.offset);
                // 选区先用底色铺一层，再在上面画字（先字后底色会把字闷掉）。
                if let Some((a, b)) = selection(state) {
                    let (a, b) = (a.max(start), b.max(start));
                    let x0 = (inner.x + width_between(&state.buffer, start, a, scale))
                        .max(inner.x + 1.0);
                    let x1 = (inner.x + width_between(&state.buffer, start, b, scale))
                        .min(inner.right() - 1.0);
                    if x1 > x0 {
                        let bar = Rect::new(x0, r.y + 5.0, x1 - x0, r.h - 10.0);
                        p.fill(bar, 3.0, pal.accent.with_alpha(0.32));
                    }
                }
                let shown = p.truncate(&state.buffer[start..], TEXT_PX, inner.w);
                p.text_in(&shown, inner, TEXT_PX, pal.text, Weight::Regular);
                // 水平滚动条压在内框下沿：只有在“在编辑 + 真的放不下”时才画。
                if let Some(thumb) = text_thumb(state, inner) {
                    p.fill(text_scroll_track(inner), 2.5, pal.line.with_alpha(0.28));
                    p.fill(thumb, 2.5, pal.accent.with_alpha(0.75));
                }
                let caret_x = inner.x + width_between(&state.buffer, start, state.cursor, scale);
                let x = caret_x.clamp(inner.x, inner.right() - 2.0);
                p.line(x, r.y + 8.0, x, r.bottom() - 8.0, pal.accent, 1.6);
            } else {
                // 不编辑时看头部（命令开头更重要）。
                let shown = p.truncate(text, TEXT_PX, inner.w);
                p.text_in(&shown, inner, TEXT_PX, pal.text, Weight::Regular);
            }
        }
        Control::Gallery {
            count,
            selected,
            hover,
        } => {
            let (cols, cell_w) = gallery_geom(rl.hit, *count);
            for dot in 0..*count {
                let cell = gallery_cell(rl.hit, dot, cols, cell_w);
                let is_selected = *selected == Some(dot);
                let is_hover = *hover == Some(dot);
                let bg = if is_selected {
                    pal.accent.with_alpha(0.16)
                } else if is_hover {
                    pal.raised
                } else {
                    pal.raised.with_alpha(0.5)
                };
                p.fill(cell, 10.0, bg);
                if is_selected {
                    p.stroke(cell, 10.0, 2.0, pal.accent);
                } else {
                    p.stroke(cell, 10.0, 1.0, pal.line.with_alpha(0.5));
                }

                // 缩略图区：上方留 8px、下方 26px 给名字。
                let box_ = Rect::new(cell.x + 8.0, cell.y + 8.0, cell.w - 16.0, cell.h - 34.0);
                match model.thumbs.get(dot) {
                    Some(Some(thumb)) => {
                        // 正方形居中：皮肤图是方的，格子不是，硬拉伸会让人"变胖"。
                        let side = box_.w.min(box_.h);
                        let rect = Rect::new(
                            box_.x + (box_.w - side) * 0.5,
                            box_.y + (box_.h - side) * 0.5,
                            side,
                            side,
                        );
                        p.image(rect, thumb);
                    }
                    _ => {
                        let msg = model.i18n.t("settings.skin.broken");
                        let shown = p.truncate(&msg, 10.0, box_.w);
                        p.text_center(&shown, box_, 10.0, pal.dim, Weight::Regular);
                    }
                }

                let name = model
                    .skins
                    .get(dot)
                    .map(|(_, name)| name.as_str())
                    .unwrap_or_default();
                let shown = p.truncate(name, 11.0, cell.w - 8.0);
                let fg = if is_selected { pal.text } else { pal.dim };
                p.text_center(
                    &shown,
                    Rect::new(cell.x, cell.bottom() - 22.0, cell.w, 16.0),
                    11.0,
                    fg,
                    Weight::Regular,
                );
            }
        }
        Control::Value { text } => {
            let r = Rect::new(rl.control.x, rl.rect.y, rl.control.w, rl.rect.h);
            let shown = p.truncate_left(text, 12.0, r.w);
            p.text_right(&shown, r, 12.0, pal.dim, Weight::Regular);
        }
        Control::Swatches => {
            let colors = [
                model.theme().body,
                model.theme().body_shade,
                model.theme().mane,
                model.theme().muzzle,
                model.theme().bubble_bg,
                model.theme().accent,
            ];
            let r = 9.0;
            let gap = 10.0;
            let total = colors.len() as f32 * (r * 2.0 + gap) - gap;
            let mut x = rl.rect.right() - total + r;
            let cy = rl.rect.y + rl.rect.h * 0.5;
            for color in colors {
                p.circle(Point::new(x, cy), r, color);
                p.stroke(
                    Rect::new(x - r, cy - r, r * 2.0, r * 2.0),
                    r,
                    1.0,
                    pal.line.with_alpha(0.4),
                );
                x += r * 2.0 + gap;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gallery_cells_tile_without_overlap_and_all_hit() {
        let rect = Rect::new(24.0, 100.0, 712.0, 220.0);
        for count in [1usize, 2, 5, 6, 12] {
            let (cols, cell_w) = gallery_geom(rect, count);
            assert!(cols >= 1, "count={count}");
            let mut cells: Vec<Rect> = Vec::new();
            for i in 0..count {
                let cell = gallery_cell(rect, i, cols, cell_w);
                assert!(cell.w >= 48.0 && cell.h > 0.0, "count={count} i={i}");
                for other in cells.iter() {
                    assert!(
                        !cell.inset(0.5, 0.5).intersects(other),
                        "count={count} i={i} overlaps"
                    );
                }
                cells.push(cell);
            }
        }
    }

    #[test]
    fn gallery_geometry_uses_width_for_columns() {
        let wide = Rect::new(0.0, 0.0, 712.0, 220.0);
        let narrow = Rect::new(0.0, 0.0, 300.0, 220.0);
        let (cols_wide, _) = gallery_geom(wide, 8);
        let (cols_narrow, _) = gallery_geom(narrow, 8);
        assert!(
            cols_wide > cols_narrow,
            "wide={cols_wide} narrow={cols_narrow}"
        );
        // 窄到只放得下一个格子也不能算成 0 列
        let (cols_tiny, _) = gallery_geom(Rect::new(0.0, 0.0, 40.0, 100.0), 3);
        assert_eq!(cols_tiny, 1);
    }

    /// "点开才展开的长说明"必须真的点得开、量得出高度、再点收得回去。
    ///
    /// 这里不测像素，只测三件容易写崩的事：行数（+1 行 Note）、
    /// Note 的高度（0 是占位，真实高度由换行决定）、以及 `Layout::notes`
    /// 里到底有没有内容（漏填的话屏幕上是一片空白，而不是报错）。
    #[test]
    fn overwrite_help_expands_into_a_measured_note_row() {
        let mut m = model();
        m.set_tab(Tab::Skins);

        let collapsed = layout(&m, 760.0, 600.0);
        let idx = collapsed
            .rows
            .iter()
            .position(|(row, _)| {
                matches!(
                    &row.control,
                    Control::Button {
                        action: Action::ToggleOverwriteHelp,
                        ..
                    }
                )
            })
            .expect("the 'explain' button should exist on the skins tab");

        activate(&mut m, &collapsed, idx);
        assert!(m.overwrite_help_open);

        let expanded = layout(&m, 760.0, 600.0);
        assert_eq!(expanded.rows.len(), collapsed.rows.len() + 1);
        let note_i = expanded
            .rows
            .iter()
            .position(|(row, _)| matches!(row.control, Control::Note { .. }))
            .expect("expanded layout should carry a note row");
        let lines = &expanded.notes[note_i];
        assert!(!lines.is_empty(), "note must be wrapped into lines");
        let (_, rl) = &expanded.rows[note_i];
        assert!(
            rl.rect.h > NOTE_PAD * 2.0 + NOTE_LINE_H * 0.5,
            "note row needs real height, got {}",
            rl.rect.h
        );
        // 说明行不参与键盘焦点：它不是可操作项。
        assert!(!expanded.rows[note_i].0.focusable());

        // 再点一次回到原状。
        activate(&mut m, &expanded, idx);
        assert!(!m.overwrite_help_open);
        let again = layout(&m, 760.0, 600.0);
        assert_eq!(again.rows.len(), collapsed.rows.len());
    }

    #[test]
    fn selecting_a_thumbnail_changes_appearance_once() {
        let mut m = model();
        m.skins = vec![
            ("procedural".into(), "Procedural".into()),
            ("zouwu-ink".into(), "水墨驺吾".into()),
        ];
        // 点已经选中的那套 → 不产生变化（避免无意义地重载图片）
        assert!(select_skin(&mut m, 0).is_empty());
        let kinds = select_skin(&mut m, 1);
        assert_eq!(kinds, vec![ChangeKind::Appearance]);
        assert_eq!(m.config.appearance.skin, "zouwu-ink");
        // 越界点击不能 panic
        assert!(select_skin(&mut m, 99).is_empty());
    }

    fn model() -> Model {
        Model::new(
            Config::default(),
            vec![Theme::fallback()],
            vec![("procedural".into(), "Procedural".into())],
            Paths {
                config: "C:/x/config.toml".into(),
                log: "C:/x/zouwu.log".into(),
            },
        )
    }

    #[test]
    fn every_tab_has_rows_and_fits_a_window() {
        let mut m = model();
        for tab in Tab::ALL {
            m.set_tab(tab);
            let l = layout(&m, 760.0, 600.0);
            assert!(!l.rows.is_empty());
            assert!(l.rows_h > 0.0);
        }
    }

    #[test]
    fn alt_drag_row_toggles_and_is_translated() {
        let mut m = model();
        let l = layout(&m, 760.0, 600.0);
        let index = l
            .rows
            .iter()
            .position(|(row, _)| row.field == Some(Field::AltDragEnabled))
            .expect("alt drag toggle row");

        // 标签必须真的翻译过：`t()` 找不到 key 时会原样回吐 key。
        let label = l.rows[index].0.label.clone();
        assert_eq!(label, m.i18n.t("settings.alt-drag"));
        assert_ne!(label, "settings.alt-drag", "ftl 里缺 settings-alt-drag");
        assert!(matches!(
            l.rows[index].0.control,
            Control::Toggle { on: true }
        ));

        // 关掉 -> 报“该卸钩子”；再开回来 -> 报“该重新装”。
        assert_eq!(activate(&mut m, &l, index), vec![ChangeKind::AltDrag]);
        assert!(!m.config.alt_drag.enabled);
        let l = layout(&m, 760.0, 600.0);
        let index = l
            .rows
            .iter()
            .position(|(row, _)| row.field == Some(Field::AltDragEnabled))
            .expect("alt drag toggle row");
        assert!(matches!(
            l.rows[index].0.control,
            Control::Toggle { on: false }
        ));
        assert_eq!(activate(&mut m, &l, index), vec![ChangeKind::AltDrag]);
        assert!(m.config.alt_drag.enabled);
    }

    #[test]
    fn reset_keeps_alt_drag_alone() {
        let mut m = model();
        m.config.alt_drag.enabled = false;
        reset(&mut m);
        // 重置的是外观，不该把用户的鼠标习惯也一并改回去。
        assert!(!m.config.alt_drag.enabled);
    }

    #[test]
    fn eye_care_rows_toggle_and_step() {
        let mut m = model();
        let l = layout(&m, 760.0, 600.0);

        // 标签必须真的翻译过：`t()` 找不到 key 时会原样回吐 key。
        let index = l
            .rows
            .iter()
            .position(|(row, _)| row.field == Some(Field::EyeCareEnabled))
            .expect("eye care toggle row");
        let label = l.rows[index].0.label.clone();
        assert_eq!(label, m.i18n.t("settings.eye-care"));
        assert_ne!(label, "settings.eye-care", "ftl 里缺 settings-eye-care");

        // 默认关着，一次点击就开，并且报出“该重新贴遮罩”。
        assert!(!m.config.eye_care.enabled);
        assert_eq!(activate(&mut m, &l, index), vec![ChangeKind::EyeCare]);
        assert!(m.config.eye_care.enabled);

        // 强度行：默认 40，步长 5，顶到上限后不再动。
        let strength = l
            .rows
            .iter()
            .position(|(row, _)| row.field == Some(Field::EyeCareStrength))
            .expect("eye care strength row");
        assert!(matches!(l.rows[strength].0.control, Control::Step { .. }));
        assert_eq!(m.config.eye_care.strength, 40);
        assert_eq!(
            step(&mut m, Field::EyeCareStrength, 1),
            vec![ChangeKind::EyeCare]
        );
        assert_eq!(m.config.eye_care.strength, 45);
        for _ in 0..20 {
            step(&mut m, Field::EyeCareStrength, 1);
        }
        assert_eq!(
            m.config.eye_care.strength,
            zouwu_core::config::EyeCare::MAX_STRENGTH
        );
        // 关掉也是“该重新贴遮罩”（这次是撤掉）。行模型要重建：`activate`
        // 读的是布局里缓存的状态，真实 UI 每次变化后都会重建布局。
        let l = layout(&m, 760.0, 600.0);
        let index = l
            .rows
            .iter()
            .position(|(row, _)| row.field == Some(Field::EyeCareEnabled))
            .expect("eye care toggle row");
        assert!(matches!(
            l.rows[index].0.control,
            Control::Toggle { on: true }
        ));
        assert_eq!(activate(&mut m, &l, index), vec![ChangeKind::EyeCare]);
        assert!(!m.config.eye_care.enabled);
    }

    #[test]
    fn the_ring_geometry_knobs_are_in_the_panel() {
        let mut m = model();
        m.set_tab(Tab::Interaction);
        let l = layout(&m, 760.0, 600.0);
        for field in [
            Field::RingRadius,
            Field::OrbitStart,
            Field::MaxPerRing,
            Field::RingGap,
        ] {
            let index = l
                .rows
                .iter()
                .position(|(row, _)| row.field == Some(field))
                .unwrap_or_else(|| panic!("{field:?} should have a row"));
            assert!(matches!(l.rows[index].0.control, Control::Step { .. }));
            let before = field.num(&m.config).expect("number field");
            let kinds = step(&mut m, field, 1);
            assert_eq!(kinds, vec![ChangeKind::Ring], "{field:?}");
            let step_by = field.num_spec().expect("spec").step;
            assert_eq!(field.num(&m.config), Some(before + step_by), "{field:?}");
        }
        // 落点检查：确认真的写到了 `[ring]` 那一段，而不是写回了交互参数。
        assert_eq!(m.config.ring.orbit_start_deg, -75.0);
        assert_eq!(m.config.ring.max_per_ring, 9);
        assert_eq!(m.config.ring.ring_gap, 74.0);
    }

    #[test]
    fn the_ring_knobs_stop_at_their_limits() {
        let mut m = model();
        for field in [Field::OrbitStart, Field::MaxPerRing, Field::RingGap] {
            let spec = field.num_spec().expect("spec");
            field.set_num(&mut m.config, spec.max);
            assert!(step(&mut m, field, 1).is_empty(), "{field:?} at max");
            assert_eq!(field.num(&m.config), Some(spec.max));
            field.set_num(&mut m.config, spec.min);
            assert!(step(&mut m, field, -1).is_empty(), "{field:?} at min");
            assert_eq!(field.num(&m.config), Some(spec.min));
        }
        // 角度能绕过 0：-180° 与 +180° 是同一个方向，不必在面板上手动“绕一圈”。
        Field::OrbitStart.set_num(&mut m.config, 0);
        step(&mut m, Field::OrbitStart, -1);
        assert_eq!(m.config.ring.orbit_start_deg, -15.0);
    }

    #[test]
    fn step_clamps_at_bounds() {
        let mut m = model();
        let field = Field::HoverDelay;
        let spec = field.num_spec().unwrap();
        field.set_num(&mut m.config, spec.max);
        let kinds = step(&mut m, field, 1);
        assert!(kinds.is_empty(), "should not move past max");
        assert_eq!(field.num(&m.config), Some(spec.max));
        step(&mut m, field, -1);
        assert_eq!(field.num(&m.config), Some(spec.max - spec.step));
    }

    #[test]
    fn toggle_returns_expected_change_kind() {
        let mut m = model();
        let l = layout(&m, 760.0, 600.0);
        // 找到"开机自启"所在行
        let index = l
            .rows
            .iter()
            .position(|(row, _)| row.field == Some(Field::StartOnBoot))
            .expect("start-on-boot row");
        let kinds = activate(&mut m, &l, index);
        assert_eq!(kinds, vec![ChangeKind::Autostart]);
        assert!(m.config.general.start_on_boot);
    }

    #[test]
    fn language_change_rebuilds_i18n() {
        let mut m = model();
        m.config.general.language = "en-US".into();
        step(&mut m, Field::Language, 1); // en-US → auto
        assert_eq!(m.config.general.language, AUTO);
        let before = m.i18n.language();
        step(&mut m, Field::Language, 1); // auto → zh-CN
        assert_eq!(m.i18n.language(), "zh-CN");
        assert_ne!(before, m.i18n.language());
    }

    #[test]
    fn hit_finds_the_right_control() {
        let mut m = model();
        m.set_tab(Tab::Interaction);
        let l = layout(&m, 760.0, 600.0);
        let (_, rl) = l
            .rows
            .iter()
            .find(|(row, _)| row.field == Some(Field::HoverDelay))
            .expect("hover delay row");
        assert_eq!(
            hit(&l, Point::new(rl.dec.x + 4.0, rl.dec.y + 4.0)),
            Some(Hit::Step(
                l.rows
                    .iter()
                    .position(|(row, _)| row.field == Some(Field::HoverDelay))
                    .unwrap(),
                -1
            ))
        );
        // 内容带之外不命中
        assert_eq!(hit(&l, Point::new(10.0, 4.0)), None);
    }

    #[test]
    fn capture_hotkey_stores_normalised_combo() {
        let mut m = model();
        m.capturing = Some(CaptureTarget::Summon);
        let combo = Hotkey {
            ctrl: true,
            shift: true,
            alt: false,
            meta: false,
            key: zouwu_core::hotkey::Key::Letter('q'),
        };
        assert!(capture_hotkey(&mut m, Some(combo)).is_some());
        assert!(m.capturing.is_none(), "capture mode must end after a combo");
        assert_eq!(m.config.general.hotkey, "Ctrl+Shift+Q");
        assert!(m.notice.is_none());
    }

    #[test]
    fn capture_hotkey_ignores_events_when_not_capturing() {
        let mut m = model();
        assert!(capture_hotkey(&mut m, Some(Hotkey::default_summon())).is_none());
        assert_eq!(m.config.general.hotkey, Config::default().general.hotkey);
    }

    #[test]
    fn reset_keeps_language() {
        let mut m = model();
        m.config.general.language = "en-US".into();
        m.config.buddy.radius = 99.0;
        reset(&mut m);
        assert_eq!(m.config.buddy.radius, Config::default().buddy.radius);
        assert_eq!(m.config.general.language, "en-US");
    }
    // ── 气泡球页（ADR-042） ──
    //
    // 每条断言都对着用户看得见的一件事：一颗球一行、关掉的球不上环、
    // 上下箭头改顺序、每颗球的热键互不相干。

    fn bubbles(m: &mut Model) -> Layout {
        m.set_tab(Tab::Bubbles);
        layout(m, 760.0, 600.0)
    }

    /// 第 `item` 颗球那一行的 **行下标**（不是控制类型下标）。
    fn row_of_item(l: &Layout, item: usize) -> usize {
        l.rows
            .iter()
            .position(
                |(row, _)| matches!(row.control, Control::RingItem { index, .. } if index == item),
            )
            .expect("ring item row")
    }

    fn row_of_action(l: &Layout, want: Action) -> usize {
        l.rows
            .iter()
            .position(
                |(row, _)| matches!(row.control, Control::Button { action, .. } if action == want),
            )
            .expect("action button row")
    }

    #[test]
    fn bubbles_tab_lists_one_row_per_configured_item() {
        let mut m = model();
        let l = bubbles(&mut m);

        let items: Vec<(usize, String, bool)> = l
            .rows
            .iter()
            .filter_map(|(row, _)| match &row.control {
                Control::RingItem {
                    index,
                    name,
                    enabled,
                    ..
                } => Some((*index, name.clone(), *enabled)),
                _ => None,
            })
            .collect();
        assert_eq!(items.len(), 2, "出厂配置就是两颗球");
        assert_eq!(items[0].0, 0);
        assert_eq!(items[1].0, 1);
        // 名字必须是翻译过的（`t()` 找不到 key 会原样吐 key）。
        for (_, name, _) in &items {
            assert!(!name.is_empty());
            assert!(!name.starts_with("action."), "untranslated label: {name}");
        }
        assert!(items[0].1 != items[1].1, "两颗球的名字不该一样");
        assert!(items.iter().all(|(_, _, on)| *on));

        // 自定义工具行（名称/命令）只该出现在自定义球上：内置球没有命令可填。
        assert!(
            !l.rows
                .iter()
                .any(|(row, _)| matches!(row.control, Control::Text { .. })),
            "内置球不该出现命令输入框"
        );
    }

    #[test]
    fn bubbles_toggle_removes_from_ring_but_keeps_the_row() {
        let mut m = model();
        let l = bubbles(&mut m);
        let index = row_of_item(&l, 0);

        let kinds = activate(&mut m, &l, index);
        assert!(kinds.contains(&ChangeKind::Ring));
        assert!(!m.config.ring.items[0].enabled, "关掉的球要留在配置里");

        // 关掉之后环上少一颗：这是"配置了几个功能就几个球"的实际效果。
        let i18n = I18n::new(&m.config.general.language);
        let shown = zouwu_core::action::resolve(&m.config.ring.items, &i18n);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].id, m.config.ring.items[1].id);

        // 行还在（否则用户没法再打开），但开关显示为关。
        let l = layout(&m, 760.0, 600.0);
        let index = row_of_item(&l, 0);
        assert!(matches!(
            l.rows[index].0.control,
            Control::RingItem { enabled: false, .. }
        ));
    }

    #[test]
    fn bubbles_up_down_reorders_the_ring() {
        let mut m = model();
        let before: Vec<String> = m.config.ring.items.iter().map(|i| i.id.clone()).collect();
        let l = bubbles(&mut m);

        // 第 1 颗按 ↑：应换到最前面（展开顺序就是这里的顺序）。
        let kinds = ring_move(&mut m, 1, -1);
        assert_eq!(kinds, vec![ChangeKind::Ring]);
        let after: Vec<String> = m.config.ring.items.iter().map(|i| i.id.clone()).collect();
        assert_eq!(after, vec![before[1].clone(), before[0].clone()]);

        // 已经在最前面还按 ↑：什么都不做，不该冒出一个多余的变更。
        assert!(ring_move(&mut m, 0, -1).is_empty());
        // ↓ 移到末尾（两颗球时 = 换回原位）。
        assert_eq!(ring_move(&mut m, 0, 1), vec![ChangeKind::Ring]);
        let back: Vec<String> = m.config.ring.items.iter().map(|i| i.id.clone()).collect();
        assert_eq!(back, before);

        // 越界下标不该 panic（行刚被删掉时点旧位置就是这个情形）。
        let _ = &l;
        assert!(ring_move(&mut m, 99, 1).is_empty());
        assert!(ring_move(&mut m, 99, -1).is_empty());
    }

    #[test]
    fn bubbles_hotkeys_are_per_item() {
        let mut m = model();
        let combo = Hotkey {
            ctrl: true,
            shift: true,
            alt: false,
            meta: false,
            key: zouwu_core::hotkey::Key::Letter('k'),
        };

        begin_capture(&mut m, CaptureTarget::Item(1));
        assert_eq!(m.capturing, Some(CaptureTarget::Item(1)));
        let kinds = capture_hotkey(&mut m, Some(combo)).expect("combo accepted");
        assert_eq!(kinds, vec![ChangeKind::Ring]);
        assert_eq!(m.config.ring.items[1].hotkey, "Ctrl+Shift+K");
        // 呼出热键不能被球的捕获改掉 —— 那是另一个入口。
        assert_eq!(m.config.general.hotkey, Config::default().general.hotkey);
        assert_eq!(
            m.config.ring.items[0].hotkey,
            zouwu_core::action::default_hotkey(&m.config.ring.items[0].id)
        );

        // 同一个组合再绑给另一颗球：拒绝，且不写坏原有配置。
        begin_capture(&mut m, CaptureTarget::Item(0));
        assert!(capture_hotkey(&mut m, Some(combo)).is_none());
        assert_eq!(
            m.config.ring.items[0].hotkey,
            zouwu_core::action::default_hotkey(&m.config.ring.items[0].id)
        );
        assert!(m.notice.is_some(), "撞车必须有提示");

        // 清除：留空 = 这颗球没有热键。
        begin_capture(&mut m, CaptureTarget::Item(1));
        let kinds = clear_hotkey(&mut m).expect("clear");
        assert_eq!(kinds, vec![ChangeKind::Ring]);
        assert!(m.config.ring.items[1].hotkey.is_empty());
        assert!(m.capturing.is_none());
    }

    #[test]
    fn bubbles_add_and_remove_items() {
        let mut m = model();
        let l = bubbles(&mut m);
        let index = row_of_action(&l, Action::AddItem);

        let kinds = activate(&mut m, &l, index);
        assert_eq!(kinds, vec![ChangeKind::Ring]);
        assert_eq!(m.config.ring.items.len(), 3);
        let added = m.config.ring.items[2].clone();
        assert!(added.id.starts_with("custom."), "custom id: {}", added.id);
        assert!(added.enabled);
        assert!(added.command.is_empty());
        // id 唯一是硬要求：`[[ring.items]]` 里重复 id 会被 `normalized` 丢掉。
        let ids: Vec<&String> = m.config.ring.items.iter().map(|i| &i.id).collect();
        for (a, b) in ids.iter().zip(ids.iter().skip(1)) {
            assert_ne!(a, b);
        }

        // 连加两个也不能撞 id。
        let l = layout(&m, 760.0, 600.0);
        let index = row_of_action(&l, Action::AddItem);
        activate(&mut m, &l, index);
        assert_eq!(m.config.ring.items.len(), 4);
        let mut ids: Vec<&String> = m.config.ring.items.iter().map(|i| &i.id).collect();
        let len = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), len, "custom id 撞车了");

        // 移除：行变少，配置也少。
        // （`activate` 里那一步会弹系统确认框，测试直接走底层函数。）
        assert_eq!(remove_item(&mut m, 2), vec![ChangeKind::Ring]);
        assert_eq!(m.config.ring.items.len(), 3);
        // 越界下标（行刚被删掉又点了一次）不该 panic。
        assert!(remove_item(&mut m, 99).is_empty());

        // 到上限就不再加了（否则环上会摆出一圈看不清的球）。
        while m.config.ring.items.len() < RingCfg::MAX_ITEMS {
            let l = layout(&m, 760.0, 600.0);
            let index = row_of_action(&l, Action::AddItem);
            activate(&mut m, &l, index);
        }
        let l = layout(&m, 760.0, 600.0);
        let index = row_of_action(&l, Action::AddItem);
        assert!(activate(&mut m, &l, index).is_empty(), "到上限后不该再变");
        assert_eq!(m.config.ring.items.len(), RingCfg::MAX_ITEMS);
        assert!(m.notice.is_some(), "到上限要有提示");
    }

    #[test]
    fn bubbles_label_and_command_edit_round_trip() {
        let mut m = model();
        // 先加一颗自定义球（内置球不需要命令，但名字也能改）。
        let l = bubbles(&mut m);
        let index = row_of_action(&l, Action::AddItem);
        activate(&mut m, &l, index);

        begin_edit(&mut m, EditTarget::ItemLabel(2));
        assert!(m.editing.is_some());
        edit_text(&mut m, "记事本");
        let kinds = commit_edit(&mut m).expect("label changed");
        assert_eq!(kinds, vec![ChangeKind::Ring]);
        assert_eq!(m.config.ring.items[2].label, "记事本");
        assert!(m.editing.is_none());

        begin_edit(&mut m, EditTarget::ItemCommand(2));
        edit_text(&mut m, "notepad.exe");
        let kinds = commit_edit(&mut m).expect("command changed");
        assert_eq!(kinds, vec![ChangeKind::Ring]);
        assert_eq!(m.config.ring.items[2].command, "notepad.exe");

        // Esc 取消：改了一半不写回，也不能报变更。
        begin_edit(&mut m, EditTarget::ItemCommand(2));
        edit_text(&mut m, "typo.exe");
        assert!(edit_key(&mut m, EditKey::Cancel).is_none());
        assert_eq!(m.config.ring.items[2].command, "notepad.exe");

        // ⏎ 提交但内容没变：同样不算变更（否则每次点开再关都会写盘）。
        begin_edit(&mut m, EditTarget::ItemCommand(2));
        assert!(edit_key(&mut m, EditKey::Commit).is_none());

        // 退格真的删字（编辑功能最容易写成"能打不能删"）。
        begin_edit(&mut m, EditTarget::ItemLabel(2));
        edit_text(&mut m, "X");
        edit_key(&mut m, EditKey::Backspace);
        commit_edit(&mut m);
        assert_eq!(m.config.ring.items[2].label, "记事本");
    }

    // ── 输入框里的鼠标选中（ADR-043）───────────────────────

    /// 当前布局里某个输入框的内框（绘制与命中用的是同一份几何）。
    fn text_inner_of(m: &mut Model, target: EditTarget) -> Rect {
        let l = bubbles(m);
        let (_, rl) = l
            .rows
            .iter()
            .find(
                |(row, _)| matches!(&row.control, Control::Text { target: t, .. } if *t == target),
            )
            .expect("text row");
        text_inner(rl.hit)
    }

    fn width_of(text: &str) -> f32 {
        measure_text(text, 1.0)
    }

    /// 某个字节下标对应的“光标 x”（相对内框左沿）。
    ///
    /// 逐字累加（不是整段量一次）：`set_caret_from_x` 也是逐字累加的，
    /// 两边算法必须一致，否则字距差异会让断言在 1px 上忽红忽绿。
    fn caret_x_of(text: &str, index: usize) -> f32 {
        let mut acc = 0.0;
        let mut i = 0;
        for ch in text.chars() {
            if i >= index {
                break;
            }
            let mut buf = [0u8; 4];
            acc += width_of(ch.encode_utf8(&mut buf));
            i += ch.len_utf8();
        }
        acc
    }

    /// 造一颗自定义球并打开它的名字输入框（内容是 `text`）。
    fn editing_label(text: &str) -> Model {
        let mut m = model();
        let l = bubbles(&mut m);
        let index = row_of_action(&l, Action::AddItem);
        activate(&mut m, &l, index);
        begin_edit(&mut m, EditTarget::ItemLabel(2));
        edit_text(&mut m, text);
        m
    }

    fn buffer_and_cursor(m: &Model) -> (&str, usize) {
        let e = m.editing.as_ref().expect("still editing");
        (e.buffer.as_str(), e.cursor)
    }

    #[test]
    fn clicking_a_text_box_puts_the_caret_under_the_pointer() {
        let mut m = editing_label("hello world");
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));

        // 点到框外左侧 = 行首；点到框外右侧 = 行尾。
        set_caret_from_x(&mut m, inner, inner.x - 20.0, false, 1.0);
        assert_eq!(buffer_and_cursor(&m), ("hello world", 0));
        set_caret_from_x(&mut m, inner, inner.right() + 50.0, false, 1.0);
        assert_eq!(buffer_and_cursor(&m), ("hello world", 11));
        assert!(!has_selection(&m), "普通点击不该留下选区");

        // 点在 "hello" 右边沿 = 下标 5；再往右越过空格的中点 = 6。
        let five = inner.x + caret_x_of("hello world", 5);
        set_caret_from_x(&mut m, inner, five, false, 1.0);
        assert_eq!(buffer_and_cursor(&m).1, 5);
        set_caret_from_x(&mut m, inner, five + width_of(" ") * 0.5 + 1.0, false, 1.0);
        assert_eq!(buffer_and_cursor(&m).1, 6);
    }

    #[test]
    fn dragging_inside_a_text_box_selects_a_range() {
        let mut m = editing_label("hello world");
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));

        // 在行首按下（只是光标，不是选区），再拖到行尾：选住整段。
        set_caret_from_x(&mut m, inner, inner.x, false, 1.0);
        assert!(!has_selection(&m));
        set_caret_from_x(&mut m, inner, inner.right() + 10.0, true, 1.0);
        assert_eq!(selection(m.editing.as_ref().unwrap()), Some((0, 11)));
        assert!(has_selection(&m));

        // 往回拖：锚点（按下那点）不动，选区只是变小（左右端与拖拽方向无关）。
        let three = inner.x + caret_x_of("hello world", 3);
        set_caret_from_x(&mut m, inner, three, true, 1.0);
        assert_eq!(selection(m.editing.as_ref().unwrap()), Some((0, 3)));
    }

    #[test]
    fn typing_and_backspace_replace_the_selection() {
        let mut m = editing_label("hello world");
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));

        // 选中 "hello" 后打字 = 整体替换（不是插在选区旁边）。
        set_caret_from_x(&mut m, inner, inner.x, false, 1.0);
        let five = inner.x + caret_x_of("hello world", 5);
        set_caret_from_x(&mut m, inner, five, true, 1.0);
        edit_text(&mut m, "hi");
        assert_eq!(buffer_and_cursor(&m), ("hi world", 2));
        assert!(!has_selection(&m), "上屏后选区应该收起来");

        // 选中 " world" 后退格 = 删掉选区。
        let two = inner.x + caret_x_of("hi world", 2);
        set_caret_from_x(&mut m, inner, two, false, 1.0);
        set_caret_from_x(&mut m, inner, inner.right() + 10.0, true, 1.0);
        edit_key(&mut m, EditKey::Backspace);
        assert_eq!(buffer_and_cursor(&m), ("hi", 2));

        // 全选后打字 = 整段换掉，提交后写回配置。
        select_all(&mut m);
        assert_eq!(selection(m.editing.as_ref().unwrap()), Some((0, 2)));
        edit_text(&mut m, "记事本");
        commit_edit(&mut m).expect("label changed");
        assert_eq!(m.config.ring.items[2].label, "记事本");
    }

    #[test]
    fn shift_arrows_extend_and_plain_arrows_collapse_the_selection() {
        let mut m = editing_label("hello");
        edit_move(&mut m, EditMotion::Home, false);
        assert_eq!(buffer_and_cursor(&m).1, 0);

        // Shift+→ 拉出选区，锚点不动。
        edit_move(&mut m, EditMotion::Right, true);
        edit_move(&mut m, EditMotion::Right, true);
        assert_eq!(selection(m.editing.as_ref().unwrap()), Some((0, 2)));

        // 不带 Shift 的 ← 先“收起”选区到左端（而不是再往左跳一格）。
        edit_move(&mut m, EditMotion::Left, false);
        assert_eq!(buffer_and_cursor(&m).1, 0);
        assert!(!has_selection(&m));

        // 不带 Shift 的 → 从没选区的地方正常前进一格。
        edit_move(&mut m, EditMotion::Right, false);
        assert_eq!(buffer_and_cursor(&m).1, 1);
    }

    #[test]
    fn a_long_line_scrolls_so_the_caret_stays_visible() {
        // 单行输入框放不下时 `offset` 往右跟，光标始终在框内（ADR-044）。
        let long: String = "abcdefghij".repeat(12);
        let mut m = editing_label(&long);
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));
        let narrow = Rect::new(inner.x, inner.y, 80.0, inner.h);
        set_edit_viewport(&mut m, narrow, 1.0);
        edit_move(&mut m, EditMotion::End, false);
        let e = m.editing.as_ref().expect("still editing");
        assert!(e.offset > 0, "长行应该已经左移（offset={}）", e.offset);
        assert!(e.buffer.is_char_boundary(e.offset));
        assert!(
            width_between(&e.buffer, e.offset, e.cursor, 1.0) <= 80.0 + 0.01,
            "光标跑出框外了"
        );

        // 短行不滚（否则开头两个字会莫名消失）。
        let mut m = editing_label("abc");
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));
        set_edit_viewport(&mut m, Rect::new(inner.x, inner.y, 400.0, inner.h), 1.0);
        edit_move(&mut m, EditMotion::Home, false);
        assert_eq!(m.editing.as_ref().unwrap().offset, 0);
        edit_move(&mut m, EditMotion::End, false);
        assert_eq!(m.editing.as_ref().unwrap().offset, 0);
    }

    // ── 输入框：双击选词、剪贴板、水平滚动条（ADR-044） ──

    #[test]
    fn double_clicking_selects_one_path_segment() {
        let text = "C:\\work\\tools\\run.exe";
        let mut m = editing_label(text);
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));

        // 双击 “tools” 中间：只选这一段，不是整条路径。
        select_word(&mut m, inner, inner.x + caret_x_of(text, 10), 1.0);
        let picked = selection(m.editing.as_ref().unwrap()).expect("selection");
        assert_eq!(&text[picked.0..picked.1], "tools");

        // 双击分隔符本身：只选它一个字符（再双击下一个词才不会“跨过去”）。
        select_word(&mut m, inner, inner.x + caret_x_of(text, 7), 1.0);
        let picked = selection(m.editing.as_ref().unwrap()).expect("selection");
        assert_eq!(&text[picked.0..picked.1], "\\");

        // 中文连续一串算一个词。
        let mut m = editing_label("打开记事本");
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));
        select_word(&mut m, inner, inner.x + caret_x_of("打开记事本", 6), 1.0);
        assert_eq!(selection(m.editing.as_ref().unwrap()), Some((0, 15)));
    }

    #[test]
    fn selecting_everything_then_typing_replaces_the_buffer() {
        let mut m = editing_label("abc");
        select_all(&mut m);
        assert_eq!(selection(m.editing.as_ref().unwrap()), Some((0, 3)));
        edit_text(&mut m, "xyz");
        assert_eq!(buffer_and_cursor(&m), ("xyz", 3));
    }

    #[test]
    fn paste_flattens_multiline_and_drops_control_chars() {
        let text = "C:\\work\\a.txt";
        let mut m = editing_label(text);
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));
        // 选中 “work” 再粘贴 = 替换选区。
        select_word(&mut m, inner, inner.x + caret_x_of(text, 5), 1.0);
        assert_eq!(selection(m.editing.as_ref().unwrap()), Some((3, 7)));
        assert!(paste(&mut m, "bin\r\nx\ty\u{7}"));
        assert_eq!(buffer_and_cursor(&m).0, "C:\\bin x y\\a.txt");

        // 全是控制字符（没有换行） = 什么也没粘进来。
        assert!(!paste(&mut m, "\r\u{7}\u{1b}"));
        // 只有换行/制表符 = 两个空格：单行框里空格是合法输入，不能当“空”丢掉。
        assert!(paste(&mut m, "\n\t"));
        assert_eq!(buffer_and_cursor(&m).0, "C:\\bin x y  \\a.txt");
    }

    #[test]
    fn cut_and_copy_do_nothing_without_something_to_act_on() {
        // 没在编辑：不碰剪贴板（也不 panic）。
        let m = model();
        assert!(!copy_selection(&m));
        // 在编辑但没选区：剪切什么都不干（整段剪切等于清空，太危险）。
        let mut m = editing_label("abc");
        assert!(!cut_selection(&mut m));
        assert_eq!(buffer_and_cursor(&m), ("abc", 3));
    }

    #[test]
    fn scrollbar_appears_only_when_the_text_overflows() {
        let mut m = editing_label("abc");
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));
        // 量下宽度（点一下就记下内框宽）。
        set_caret_from_x(&mut m, inner, inner.x + 1.0, false, 1.0);
        {
            let e = m.editing.as_ref().unwrap();
            assert!(!text_overflow(e));
            assert!(text_thumb(e, inner).is_none(), "放得下就不该有滚动条");
        }

        // 压窄视口 → 出现滚动条，thumb 不会跑出内框。
        let long: String = "abcdefghij".repeat(12);
        let mut m = editing_label(&long);
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));
        let narrow = Rect::new(inner.x, inner.y, 120.0, inner.h);
        set_edit_viewport(&mut m, narrow, 1.0);
        let e = m.editing.as_ref().unwrap();
        assert!(text_overflow(e));
        let thumb = text_thumb(e, narrow).expect("overflow -> thumb");
        assert!(thumb.w > 0.0 && thumb.w <= narrow.w + 0.01);
        assert!(thumb.x >= narrow.x - 0.01 && thumb.right() <= narrow.right() + 0.01);
        // 刚好点在 thumb 上才算命中滚动条，压在正文上不算。
        let on = Point::new(thumb.x + thumb.w * 0.5, thumb.bottom() - 1.0);
        assert!(text_scrollbar_at(e, narrow, on));
        assert!(!text_scrollbar_at(
            e,
            narrow,
            Point::new(narrow.x + 1.0, narrow.y + 2.0)
        ));
    }

    #[test]
    fn dragging_the_scrollbar_moves_the_viewport() {
        let long: String = "abcdefghij".repeat(12);
        let mut m = editing_label(&long);
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));
        let narrow = Rect::new(inner.x, inner.y, 120.0, inner.h);
        set_edit_viewport(&mut m, narrow, 1.0);
        edit_move(&mut m, EditMotion::Home, false);
        assert_eq!(m.editing.as_ref().unwrap().offset, 0);

        // 拖到中间：offset 往右、且停在字符边界上。
        set_offset_from_x(&mut m, narrow, narrow.x + narrow.w * 0.5);
        let e = m.editing.as_ref().unwrap();
        assert!(e.offset > 0, "拖到中间应该往右滚");
        assert!(e.buffer.is_char_boundary(e.offset));

        // 拖滚动条不能挪光标（否则用户刚选好的选区会被当场抹掉）。
        assert_eq!(e.cursor, 0, "拖滚动条不该把光标带走");

        // 拖回最左：回到开头。
        set_offset_from_x(&mut m, narrow, narrow.x - 50.0);
        assert_eq!(m.editing.as_ref().unwrap().offset, 0);
    }

    #[test]
    fn wheel_scrolls_the_edit_horizontally() {
        let long: String = "abcdefghij".repeat(12);
        let mut m = editing_label(&long);
        let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));
        let narrow = Rect::new(inner.x, inner.y, 120.0, inner.h);
        set_edit_viewport(&mut m, narrow, 1.0);
        edit_move(&mut m, EditMotion::Home, false);
        assert_eq!(m.editing.as_ref().unwrap().offset, 0);

        scroll_edit(&mut m, 48.0);
        assert!(m.editing.as_ref().unwrap().offset > 0, "滚轮应该往右带");
        scroll_edit(&mut m, -10_000.0);
        assert_eq!(m.editing.as_ref().unwrap().offset, 0, "往左滚到底就是 0");
    }

    /// 真的走一遍系统剪贴板（所以默认 `#[ignore]`，且会把用户剪贴板原样写回）。
    ///
    /// 单元测试不碰剪贴板：并行跑时两个 `OpenClipboard()` 会互相抢，还可能是别的进程占着。
    /// 但"我们写的 `set_text` 和 `get_text` 能不能对上"必须有人验，手工验一次就够 ——
    /// 要跑就 `cargo test -p zouwu-ui --lib the_clipboard_bridge_round_trips -- --ignored`。
    #[test]
    #[ignore = "会读写系统剪贴板，需要交互式桌面"]
    fn the_clipboard_bridge_round_trips() {
        let saved = host_clipboard_get();
        let text = "C:\\work\\tools\\run.exe --flag";
        if !host_clipboard_set(text) {
            // 剪贴板被别的程序长期占着（或远程会话没重定向剪贴板）时
            // `OpenClipboard` 会一直 `ACCESS_DENIED`。别把“环境没剪贴板”改成红测试，
            // 但也别假装通过 —— 用 eprintln 说清楚跳过。
            eprintln!("clipboard unavailable; skipping the bridge check");
            return;
        }
        assert_eq!(
            host_clipboard_get().as_deref(),
            Some(text),
            "读回来的不一样"
        );

        // 再走一遍窗口层用的入口：Ctrl+V = `paste_clipboard()`。
        let mut m = editing_label("");
        assert!(paste_clipboard(&mut m), "空框里粘不进来");
        assert_eq!(buffer_and_cursor(&m).0, text);

        if let Some(saved) = saved {
            host_clipboard_set(&saved);
        }
    }

    #[test]
    fn the_scrollbar_is_actually_painted() {
        // 几何对不代表画出来了（ADR-043 就抓过“命中对了但没画”）。这里直接渲染看像素：
        // 同样位置，长文本比短文本多出来的东西就该是那条滚动条。
        let band_diff = |short: &Model, long: &Model, inner: Rect| -> usize {
            let mut a = tiny_skia::Pixmap::new(760, 600).expect("pixmap");
            let mut b = tiny_skia::Pixmap::new(760, 600).expect("pixmap");
            draw(&mut a, &layout(short, 760.0, 600.0), short);
            draw(&mut b, &layout(long, 760.0, 600.0), long);
            let mut diff = 0;
            let y0 = (inner.bottom() as u32).saturating_sub(5);
            let y1 = (inner.bottom() as u32).saturating_sub(1);
            let x0 = inner.x as u32 + 3;
            let x1 = (inner.x + inner.w) as u32 - 3;
            for y in y0..y1 {
                for x in x0..x1 {
                    let (Some(pa), Some(pb)) = (a.pixel(x, y), b.pixel(x, y)) else {
                        continue;
                    };
                    if (pa.red(), pa.green(), pa.blue()) != (pb.red(), pb.green(), pb.blue()) {
                        diff += 1;
                    }
                }
            }
            diff
        };

        let setup = |text: &str| -> (Model, Rect) {
            let mut m = editing_label(text);
            let inner = text_inner_of(&mut m, EditTarget::ItemLabel(2));
            set_edit_viewport(&mut m, inner, 1.0);
            set_offset_from_x(&mut m, inner, inner.x + inner.w * 0.5);
            (m, inner)
        };
        let (short, inner) = setup("abc");
        let (long, _) = setup(&"abcdefghij".repeat(30));
        let painted = band_diff(&short, &long, inner);
        assert!(
            painted > 40,
            "内框下沿没看到滚动条（差异像素 painted={painted}）"
        );
    }

    #[test]
    fn bubbles_row_controls_hit_their_own_rects() {
        let mut m = model();
        let l = bubbles(&mut m);
        let index = row_of_item(&l, 0);
        let rl = &l.rows[index].1;
        let mid = |r: Rect| Point::new(r.x + r.w * 0.5, r.y + r.h * 0.5);

        assert_eq!(hit(&l, mid(rl.up)), Some(Hit::RingMove(0, -1)));
        assert_eq!(hit(&l, mid(rl.down)), Some(Hit::RingMove(0, 1)));
        assert_eq!(hit(&l, mid(rl.hotkey)), Some(Hit::RingHotkey(0)));
        assert_eq!(
            hit(&l, mid(rl.hit)),
            Some(Hit::Press(index)),
            "开关本身点下去要能切换"
        );

        // 两个箭头不能重叠（重叠会导致"按 ↑ 结果往下走"）。
        assert!(rl.up.right() <= rl.down.x + 0.01);
        assert!(rl.down.right() <= rl.hotkey.x + 0.01);
        assert!(rl.hotkey.right() <= rl.hit.x + 0.01);
    }

    #[test]
    fn bubbles_rows_fit_the_minimum_window() {
        // 这页控件最多（箭头+热键+开关），最小宽度下不能被挤到负宽。
        let mut m = model();
        m.set_tab(Tab::Bubbles);
        let l = layout(&m, MIN_SIZE.0, MIN_SIZE.1);
        for (row, rl) in &l.rows {
            if matches!(row.control, Control::RingItem { .. }) {
                assert!(rl.up.w > 10.0 && rl.up.h > 10.0);
                assert!(rl.hotkey.w >= 80.0, "热键框被挤扁了: {}", rl.hotkey.w);
                assert!(rl.hit.w > 20.0);
                assert!(rl.hotkey.x >= rl.rect.x, "控件跑到行外面了");
                assert!(rl.hit.right() <= rl.rect.right() + 0.01);
            }
        }
    }
}
