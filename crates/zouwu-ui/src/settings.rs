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

use zouwu_core::config::{ChangeKind, Config, LogCfg};
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
    Interaction,
    Appearance,
    Skins,
    About,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::General,
        Tab::Interaction,
        Tab::Appearance,
        Tab::Skins,
        Tab::About,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Tab::General => "settings.tab.general",
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
    HoverFactor,
    CollapseFactor,
    HitTolerance,
    BuddyRadius,
    Breathe,
    RingRadius,
    BubbleRadius,
    FloatAmplitude,
    ExpandStagger,
    Theme,
    Skin,
    FontPack,
    ClipboardEnabled,
    ClipboardLimit,
    LogLevel,
    LogToFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Ms,
    Px,
    Percent,
    Count,
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
            Field::HoverFactor => "settings.hover-factor",
            Field::CollapseFactor => "settings.collapse-factor",
            Field::HitTolerance => "settings.hit-tolerance",
            Field::BuddyRadius => "settings.buddy-radius",
            Field::Breathe => "settings.breathe",
            Field::RingRadius => "settings.ring-radius",
            Field::BubbleRadius => "settings.bubble-radius",
            Field::FloatAmplitude => "settings.float-amplitude",
            Field::ExpandStagger => "settings.expand-stagger",
            Field::Theme => "settings.theme",
            Field::Skin => "settings.skin",
            Field::FontPack => "settings.font",
            Field::ClipboardEnabled => "settings.clipboard",
            Field::ClipboardLimit => "settings.clipboard.limit",
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
            Field::HoverFactor => (cfg.interaction.hover_radius_factor * 100.0).round() as i64,
            Field::CollapseFactor => {
                (cfg.interaction.collapse_radius_factor * 100.0).round() as i64
            }
            Field::HitTolerance => (cfg.interaction.hit_tolerance * 100.0).round() as i64,
            Field::BuddyRadius => cfg.buddy.radius.round() as i64,
            Field::RingRadius => cfg.ring.radius.round() as i64,
            Field::BubbleRadius => cfg.ring.bubble_radius.round() as i64,
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
            Field::HoverFactor => cfg.interaction.hover_radius_factor = v as f32 / 100.0,
            Field::CollapseFactor => cfg.interaction.collapse_radius_factor = v as f32 / 100.0,
            Field::HitTolerance => cfg.interaction.hit_tolerance = v as f32 / 100.0,
            Field::BuddyRadius => cfg.buddy.radius = v as f32,
            Field::RingRadius => cfg.ring.radius = v as f32,
            Field::BubbleRadius => cfg.ring.bubble_radius = v as f32,
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
            Field::ExpandOnClick => cfg.interaction.expand_on_click,
            Field::ClipboardEnabled => cfg.clipboard.enabled,
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
            Field::ExpandOnClick => cfg.interaction.expand_on_click = on,
            Field::ClipboardEnabled => cfg.clipboard.enabled = on,
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
                Control::Button { .. } | Control::Checkbox { .. }
            )
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
    pub capturing: bool,
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
            capturing: false,
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
            self.tab = tab;
            self.scroll = 0.0;
            self.focus = None;
            self.capturing = false;
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
                    capturing: model.capturing,
                },
            });
            rows.push(toggle_row(model, Field::StartOnBoot));
            rows.push(toggle_row(model, Field::ShowOnStart));
            rows.push(toggle_row(model, Field::BringToFront));
            rows.push(toggle_row(model, Field::AlwaysOnTop));
            rows.push(step_row(model, Field::WindowSize));

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
        Tab::Interaction => {
            rows.push(Row::section(i18n.t("settings.section.behavior")));
            rows.push(step_row(model, Field::HoverDelay));
            rows.push(step_row(model, Field::CollapseDelay));
            rows.push(toggle_row(model, Field::ExpandOnClick));
            rows.push(step_row(model, Field::HoverFactor));
            rows.push(step_row(model, Field::CollapseFactor));
            rows.push(step_row(model, Field::HitTolerance));
            rows.push(Row::section(i18n.t("settings.section.beast")));
            rows.push(step_row(model, Field::BuddyRadius));
            rows.push(toggle_row(model, Field::Breathe));
            rows.push(Row::section(i18n.t("settings.section.ring")));
            rows.push(step_row(model, Field::RingRadius));
            rows.push(step_row(model, Field::BubbleRadius));
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
    }
}

fn hotkey_text(model: &Model) -> String {
    if model.capturing {
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
    /// 按下就触发的区域（开关、热键框、按钮）。
    pub hit: Rect,
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

pub fn layout(model: &Model, w: f32, h: f32) -> Layout {
    let content = Rect::new(0.0, HEADER_H, w, (h - HEADER_H - FOOTER_H).max(0.0));

    // 标签页：等宽铺开，右对齐到内容宽度。
    let tab_w = 108.0;
    let tab_h = 30.0;
    // 垂直居中在 header 带里，而不是贴底：header 现在只有标签页一层的厚度。
    let tabs_y = (HEADER_H - tab_h) * 0.5;
    let mut tabs = Vec::with_capacity(4);
    for (i, tab) in Tab::ALL.iter().enumerate() {
        tabs.push((
            *tab,
            Rect::new(MARGIN + i as f32 * (tab_w + 8.0), tabs_y, tab_w, tab_h),
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
        let control = Rect::new(
            rect.right() - CONTROL_W,
            y + (rh - 30.0) * 0.5,
            CONTROL_W,
            30.0,
        );
        let dec = Rect::new(control.x, control.y, 30.0, control.h);
        let inc = Rect::new(control.right() - 30.0, control.y, 30.0, control.h);
        let hit = match row.control {
            Control::Toggle { .. } => {
                Rect::new(control.right() - 46.0, y + (rh - 26.0) * 0.5, 46.0, 26.0)
            }
            Control::Hotkey { .. } => Rect::new(control.x, y + (rh - 30.0) * 0.5, CONTROL_W, 30.0),
            Control::Button { .. } => Rect::new(
                rect.right() - BUTTON_W,
                y + (rh - 30.0) * 0.5,
                BUTTON_W,
                30.0,
            ),
            Control::Checkbox { .. } => {
                Rect::new(rect.right() - 24.0, y + (rh - 24.0) * 0.5, 24.0, 24.0)
            }
            // 缩略图墙占满整行宽度：格子要尽量大，缩略图才有辨识度。
            Control::Gallery { .. } => Rect::new(rect.x, y + 6.0, rect.w, rh - 12.0),
            _ => control,
        };
        out_rows.push((
            row,
            RowLayout {
                rect,
                control,
                dec,
                inc,
                hit,
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
        }
        return Vec::new();
    }
    let Some(field) = row.field else {
        return Vec::new();
    };
    match control {
        Control::Toggle { on } => {
            field.set_bool(&mut model.config, !on);
            changed(model, field)
        }
        Control::Hotkey { .. } => {
            model.capturing = !model.capturing;
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
        Field::WindowSize => kinds.push(ChangeKind::WindowSize),
        Field::BuddyRadius | Field::Breathe => kinds.push(ChangeKind::Buddy),
        Field::RingRadius | Field::BubbleRadius | Field::FloatAmplitude | Field::ExpandStagger => {
            kinds.push(ChangeKind::Ring)
        }
        _ => kinds.push(ChangeKind::Interaction),
    }
    kinds
}

fn reset(model: &mut Model) {
    let defaults = Config::default();
    // 语言、热键、自启、日志属于"运行方式"，重置外观不该把用户的语言也改回英文。
    model.config.appearance = defaults.appearance;
    model.config.interaction = defaults.interaction;
    model.config.ring = defaults.ring;
    model.config.buddy = defaults.buddy;
    model.config.window = defaults.window;
    model.notice = Some(model.i18n.t("settings.reset.done"));
}

// ── 热键捕获 ──────────────────────────────────────────────

/// 返回 `Some(Ok(()))` 表示热键已更新，`Some(Err(msg))` 表示这组键不能用。
pub fn capture_hotkey(model: &mut Model, hotkey: Option<Hotkey>) -> Option<Vec<ChangeKind>> {
    if !model.capturing {
        return None;
    }
    let hotkey = hotkey?;
    model.capturing = false;
    // 直接存捕获到的组合。不再回写字符串再解析一遍——那只是给"字符串格式"
    // 制造第二次出错的机会。
    model.config.general.hotkey = hotkey.to_string();
    model.notice = None;
    Some(vec![ChangeKind::Hotkey])
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
        m.capturing = true;
        let combo = Hotkey {
            ctrl: true,
            shift: true,
            alt: false,
            meta: false,
            key: zouwu_core::hotkey::Key::Letter('q'),
        };
        assert!(capture_hotkey(&mut m, Some(combo)).is_some());
        assert!(!m.capturing, "capture mode must end after a combo");
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
}
