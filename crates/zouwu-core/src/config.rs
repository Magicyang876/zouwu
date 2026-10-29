//! 配置模型（纯数据 + 序列化，不含 IO）。
//!
//! IO 由 `zouwu-platform` 负责，保证核心层可在任意环境（含测试）下使用。

use crate::ring::{RingConfig, DEG};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 7;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    /// 放在最前面：这是用户最常手改的一段（语言、热键、自启）。
    pub general: General,
    pub appearance: Appearance,
    /// 护眼（蓝光过滤）：一层暖色半透明遮罩，强度可调。
    pub eye_care: EyeCare,
    /// Alt 拖动窗口（移动 / 双击最大化 / 滚轮最大最小化）。
    pub alt_drag: AltDrag,
    pub clipboard: ClipboardCfg,
    pub log: LogCfg,
    pub interaction: Interaction,
    pub ring: RingCfg,
    pub buddy: BuddyCfg,
    pub window: WindowCfg,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            general: General::default(),
            appearance: Appearance::default(),
            eye_care: EyeCare::default(),
            alt_drag: AltDrag::default(),
            clipboard: ClipboardCfg::default(),
            log: LogCfg::default(),
            interaction: Interaction::default(),
            ring: RingCfg::default(),
            buddy: BuddyCfg::default(),
            window: WindowCfg::default(),
        }
    }
}

/// 剪贴板历史。
///
/// 默认**开着**：这是一个小工具的典型用法——复制过的链接、地址、片段
/// 需要回头再拿一次。关掉的入口在设置面板与配置文件里都有。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardCfg {
    /// 是否记录剪贴板历史。
    pub enabled: bool,
    /// 最多保留多少条。
    pub limit: usize,
}

impl Default for ClipboardCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            limit: crate::history::DEFAULT_LIMIT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interaction {
    /// 悬停多久后展开（毫秒）。0 = 立即展开。
    pub hover_delay_ms: u64,
    /// 指针离开环带后多久收起（毫秒）。
    pub collapse_delay_ms: u64,
    /// 点击小兽是否立即展开（绕过延时）。
    pub expand_on_click: bool,
    /// **悬停热区**：指针在"小兽半径 × 该系数"以内算"正摸着小兽"，
    /// 才开始累计 `hover_delay_ms`。只管"要不要展开"，与收起无关。
    pub hover_radius_factor: f32,
    /// **收起边界**：环带外沿 = "(环半径 + 气泡直径) × 该系数"，
    /// 指针越过这条线并持续 `collapse_delay_ms` 才收起。只管"要不要收起"。
    ///
    /// 早先这一个系数同时管两件事，于是调它必然顾此失彼：调大让气泡好点，
    /// 却把悬停热区撑成两倍大；调小反之。这是两个独立的问题，就该有两个旋钮。
    pub collapse_radius_factor: f32,
    /// 命中容差（气泡半径的倍数）。
    pub hit_tolerance: f32,
}

impl Interaction {
    /// 悬停热区半径（逻辑像素）：超过它就不再算"摸着小兽"。
    pub fn hover_radius(&self, buddy_radius: f32) -> f32 {
        buddy_radius * self.hover_radius_factor
    }

    /// 收起边界半径（逻辑像素，未考虑窗口可达范围）。
    pub fn collapse_radius(&self, ring_radius: f32, bubble_radius: f32) -> f32 {
        (ring_radius + bubble_radius * 2.0) * self.collapse_radius_factor
    }
}

impl Default for Interaction {
    fn default() -> Self {
        Self {
            hover_delay_ms: 150,
            collapse_delay_ms: 250,
            expand_on_click: true,
            hover_radius_factor: 1.1,
            collapse_radius_factor: 1.1,
            hit_tolerance: 1.15,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RingCfg {
    pub radius: f32,
    pub bubble_radius: f32,
    pub orbit_start_deg: f32,
    pub max_per_ring: usize,
    pub ring_gap: f32,
    /// 轨道浮动幅度（像素）
    pub float_amplitude: f32,
    /// 轨道浮动周期（秒）
    pub float_period_s: f32,
    /// 展开时每颗气泡的错峰间隔（毫秒）
    pub expand_stagger_ms: u64,
}

impl Default for RingCfg {
    fn default() -> Self {
        Self {
            radius: 112.0,
            bubble_radius: 24.0,
            orbit_start_deg: -90.0,
            max_per_ring: 8,
            ring_gap: 70.0,
            float_amplitude: 2.0,
            float_period_s: 3.0,
            expand_stagger_ms: 24,
        }
    }
}

impl RingCfg {
    pub fn to_ring_config(&self) -> RingConfig {
        RingConfig {
            radius: self.radius,
            bubble_radius: self.bubble_radius,
            orbit_start: self.orbit_start_deg * DEG,
            max_per_ring: self.max_per_ring,
            ring_gap: self.ring_gap,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuddyCfg {
    /// 小兽半径（逻辑像素）
    pub radius: f32,
    /// 是否开启匀速呼吸（Idle 时低频缩放）
    pub breathe: bool,
}

impl Default for BuddyCfg {
    fn default() -> Self {
        Self {
            radius: 56.0,
            breathe: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowCfg {
    /// 窗口边长（逻辑像素）。M0 固定，保证气泡环展开不用改窗口尺寸。
    pub size: f32,
    pub always_on_top: bool,
    pub remember_position: bool,
}

impl Default for WindowCfg {
    fn default() -> Self {
        Self {
            size: 340.0,
            always_on_top: true,
            remember_position: true,
        }
    }
}

/// 常驻形态相关的开关：托盘、热键、语言、开机自启。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// 界面语言：`"auto"` = 跟随系统，或 `"zh-CN"` / `"en-US"`。
    pub language: String,
    /// 全局热键，形如 `"Alt+Space"`；空串 = 不注册。
    pub hotkey: String,
    /// 扫屏幕二维码的热键；空串 = 不注册。
    ///
    /// 单独一条而不是复用呼出热键：二维码是"想到才按"的动作，
    /// 不该逼用户先把小兽叫出来、再点气泡。
    pub qr_hotkey: String,
    /// 打开剪贴板历史的热键；空串 = 不注册。
    pub clipboard_hotkey: String,
    /// 开机自启（Windows 注册表 Run 键）。
    pub start_on_boot: bool,
    /// 启动后是否直接显示小兽（false = 先待在托盘里）。
    pub show_on_start: bool,
    /// 呼出时是否把窗口带到最前。
    pub bring_to_front: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: "auto".to_string(),
            hotkey: "Alt+Space".to_string(),
            qr_hotkey: "Ctrl+Alt+Q".to_string(),
            clipboard_hotkey: "Ctrl+Alt+V".to_string(),
            start_on_boot: false,
            show_on_start: true,
            bring_to_front: true,
        }
    }
}

/// 外观：主题与皮肤。两者正交——皮肤决定"长什么样"，主题决定"什么颜色"。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// 主题 id：内置的 `zouwu-warm` / `zouwu-dark`，或
    /// `%APPDATA%/zouwu/themes/<id>.toml` 里声明的 id。
    pub theme: String,
    /// 皮肤 id：`procedural`（代码绘制）或
    /// `%APPDATA%/zouwu/skins/<id>/skin.toml` 里声明的 id。
    pub skin: String,
    /// 字体包 id：空串 = 用系统默认字体（`text_gdi::DEFAULT_FAMILY`），
    /// 非空时指向 `%APPDATA%/zouwu/fonts/<id>/font.toml` 里声明的 id。
    ///
    /// 没选字体包时不去写死一个 id：用户可能一辈子不换字体，
    /// 而"空 = 系统"让老配置天然等价于"从未设置过"。
    pub font_pack: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: "zouwu-warm".to_string(),
            skin: crate::skin::PROCEDURAL_SKIN_ID.to_string(),
            font_pack: String::new(),
        }
    }
}

/// 护眼（蓝光过滤）。
///
/// 实现是**一层暖色半透明遮罩**贴在所有屏幕内容之上（点击穿透，不影响操作），
/// 和 f.lux / 护眼宝那类工具的做法一致：不做 ICC/伽马改写，
/// 因为改伽马会影响整个桌面且难以可靠恢复。
///
/// 默认**关着**：它是有感的功能，不该在用户没要求时就把屏幕染黄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EyeCare {
    /// 是否开启遮罩。
    pub enabled: bool,
    /// 过滤强度，`0..=100`。0 = 只有一点点暖色，100 = 最重（约 55% 不透明度）。
    pub strength: u8,
}

impl Default for EyeCare {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: 40,
        }
    }
}

impl EyeCare {
    /// 上限：再重就看不清内容了，护眼宝一类工具也停在这个量级。
    pub const MAX_STRENGTH: u8 = 100;

    /// 强度 -> 遮罩不透明度（`0..=255`）。
    ///
    /// 线性映射到 0..=140：`strength=40` 约 22%，肉眼刚好“黄一点但不刺眼”。
    pub fn alpha(self) -> u8 {
        let s = self.strength.min(Self::MAX_STRENGTH) as u32;
        (s * 140 / 100) as u8
    }

    /// 越界的强度就地收敛（配置文件是手改的，不能信任）。
    pub fn clamped(mut self) -> Self {
        self.strength = self.strength.min(Self::MAX_STRENGTH);
        self
    }
}

/// Alt 拖动窗口（AltSnap 那一套的最小集）。
///
/// * 按住 `Alt` + 左键拖动 = 移动光标下的窗口；
/// * 按住 `Alt` + 双击 = 最大化 / 还原；
/// * 按住 `Alt` + 滚轮上 = 最大化，滚轮下 = 最小化。
///
/// 默认**开着**：这个功能只在“按住 Alt 并动了鼠标”时才介入，不抢普通点击，
/// 而且是用户明确要的东西；开关留在设置面板里（“通用 → 窗口拖动”）。
///
/// 修饰键固定是 Alt，不做可配置：这条需求的全部价值就是"和 AltSnap 一个手感"，
/// 再多一个可选项只会多一种“为什么我按了没反应”的配置错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AltDrag {
    pub enabled: bool,
}

impl Default for AltDrag {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// 日志落盘。写在文件里的一行应该让人**不用开着控制台**也能定位问题。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogCfg {
    /// `off` / `error` / `warn` / `info` / `debug` / `trace`。
    /// 环境变量 `RUST_LOG` 优先（临时排查时不想改用户的持久配置）。
    pub level: String,
    /// 是否写文件（关掉后不保留普通日志）。
    pub to_file: bool,
    /// 单个日志文件的上限（KB），超过就在下次启动时轮转。
    pub max_size_kb: u64,
    /// 保留多少个历史文件（`zouwu.1.log` …）。
    pub keep_files: u32,
}

impl Default for LogCfg {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            to_file: true,
            max_size_kb: 2048,
            keep_files: 3,
        }
    }
}

impl LogCfg {
    /// 合法的级别名（也是设置面板里的选项顺序）。
    pub const LEVELS: [&'static str; 6] = ["off", "error", "warn", "info", "debug", "trace"];

    pub fn is_valid_level(level: &str) -> bool {
        Self::LEVELS.contains(&level)
    }
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// 配置迁移入口：v1 没有 `[general]`，v2 没有 `[appearance]`，
    /// 两者都靠 `#[serde(default)]` 补全，这里只把版本号推进到最新。
    pub fn migrated(mut self) -> Self {
        if self.schema_version < SCHEMA_VERSION {
            self.schema_version = SCHEMA_VERSION;
        }
        self.eye_care = self.eye_care.clamped();
        self
    }

    /// 从 `self`（运行中的配置）到 `next`（磁盘上新读到的配置）需要重新作用于
    /// 运行期对象的维度，按固定顺序返回。
    ///
    /// 这里**只列真的能热应用的东西**。`general.show_on_start` 只在启动时读一次，
    /// 改了它不该产生任何副作用 —— 把它放进 diff 只会让人误以为已经生效。
    /// 返回空 `Vec` 不等于"没变化"（例如只改了 `remember_position`），
    /// 调用方仍应把新配置整体接管，只是没有额外动作要做。
    pub fn diff(&self, next: &Config) -> Vec<ChangeKind> {
        use ChangeKind::*;
        let mut out = Vec::new();
        if self.general.language != next.general.language {
            out.push(Language);
        }
        if self.general.hotkey != next.general.hotkey {
            out.push(Hotkey);
        }
        if self.general.start_on_boot != next.general.start_on_boot {
            out.push(Autostart);
        }
        if self.appearance != next.appearance {
            out.push(Appearance);
        }
        if self.eye_care != next.eye_care {
            out.push(EyeCare);
        }
        if self.alt_drag != next.alt_drag {
            out.push(AltDrag);
        }
        if self.clipboard != next.clipboard {
            out.push(Clipboard);
        }
        if self.log != next.log {
            out.push(Log);
        }
        if self.interaction != next.interaction {
            out.push(Interaction);
        }
        if self.ring != next.ring {
            out.push(Ring);
        }
        if self.buddy != next.buddy {
            out.push(Buddy);
        }
        if self.window.size != next.window.size {
            out.push(WindowSize);
        }
        if self.window.always_on_top != next.window.always_on_top {
            out.push(WindowLevel);
        }
        out
    }
}

/// 配置热重载时需要重新作用的维度。
///
/// 存在的意义是把"哪些字段能热生效"变成**可测试的领域知识**，而不是散落在 UI
/// 层的一堆 `if old.field != new.field`。UI 只需要 `match` 这些名字。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    /// 界面语言（托盘菜单 + 文案）
    Language,
    /// 全局热键（重新注册）
    Hotkey,
    /// 开机自启（写注册表）
    Autostart,
    /// 主题 / 皮肤
    Appearance,
    /// 护眼遮罩的开关与强度
    EyeCare,
    /// Alt 拖动窗口的开关（装 / 卸低层鼠标钩子）
    AltDrag,
    /// 剪贴板历史开关与容量
    Clipboard,
    /// 日志级别与落盘策略
    Log,
    /// 悬停延时 / 命中容差 / 收起半径
    Interaction,
    /// 气泡环几何（半径、气泡数、浮动幅度）
    Ring,
    /// 小兽半径 / 呼吸开关
    Buddy,
    /// 窗口边长（要改窗口尺寸）
    WindowSize,
    /// 置顶与否（窗口层级）
    WindowLevel,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::Language => "language",
            ChangeKind::Hotkey => "hotkey",
            ChangeKind::Autostart => "autostart",
            ChangeKind::Appearance => "appearance",
            ChangeKind::EyeCare => "eye_care",
            ChangeKind::AltDrag => "alt_drag",
            ChangeKind::Clipboard => "clipboard",
            ChangeKind::Log => "log",
            ChangeKind::Interaction => "interaction",
            ChangeKind::Ring => "ring",
            ChangeKind::Buddy => "buddy",
            ChangeKind::WindowSize => "window.size",
            ChangeKind::WindowLevel => "window.always_on_top",
        }
    }
}

/// 窗口位置等运行期状态，单独存盘，避免与用户配置互相覆盖。
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrip() {
        let cfg = Config::default();
        let text = cfg.to_toml();
        let back = Config::from_toml(&text).expect("parse");
        assert_eq!(back.interaction.hover_delay_ms, 150);
        assert_eq!(back.ring.radius, 112.0);
    }

    #[test]
    fn partial_toml_uses_defaults() {
        let cfg = Config::from_toml("[interaction]\nhover_delay_ms = 0\n").expect("parse");
        assert_eq!(cfg.interaction.hover_delay_ms, 0);
        assert_eq!(cfg.interaction.collapse_delay_ms, 250);
        assert_eq!(cfg.window.size, 340.0);
    }

    #[test]
    fn general_defaults_and_roundtrip() {
        let cfg = Config::default();
        assert_eq!(cfg.general.language, "auto");
        assert_eq!(cfg.general.hotkey, "Alt+Space");
        let back = Config::from_toml(&cfg.to_toml()).expect("parse");
        assert_eq!(back.general.hotkey, "Alt+Space");
        assert_eq!(back.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn v1_config_migrates_to_latest() {
        let cfg = Config::from_toml(
            "[general]
",
        )
        .expect("parse");
        assert_eq!(cfg.general.language, "auto");
        assert_eq!(cfg.migrated().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn v3_config_without_font_pack_uses_the_system_font() {
        // 老配置（schema 3）里没有 font_pack。它不能被解析成"字体包叫空串"之外的东西，
        // 而且必须显式等于空串 —— 空串在我们的语义里就是"用系统默认字体"。
        let cfg = Config::from_toml(
            r#"
            schema_version = 3
            [appearance]
            theme = "zouwu-dark"
            skin = "zouwu-ink"
            "#,
        )
        .expect("parse");
        assert_eq!(cfg.appearance.theme, "zouwu-dark");
        assert_eq!(cfg.appearance.skin, "zouwu-ink");
        assert!(cfg.appearance.font_pack.is_empty());
        assert_eq!(cfg.migrated().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn diff_reports_only_actionable_changes() {
        let base = Config::default();
        assert!(base.diff(&base).is_empty());

        let mut next = base.clone();
        next.general.language = "en-US".into();
        next.general.hotkey = "Ctrl+Shift+Z".into();
        next.ring.radius = 130.0;
        next.window.size = 360.0;
        assert_eq!(
            base.diff(&next),
            vec![
                ChangeKind::Language,
                ChangeKind::Hotkey,
                ChangeKind::Ring,
                ChangeKind::WindowSize
            ]
        );
    }

    #[test]
    fn diff_ignores_startup_only_fields() {
        let base = Config::default();
        let mut next = base.clone();
        // 启动期开关：改了它不该有任何运行期副作用
        next.general.show_on_start = !base.general.show_on_start;
        next.window.remember_position = !base.window.remember_position;
        assert!(base.diff(&next).is_empty());
        // 但内容确实变了，调用方仍要接管新配置
        assert_ne!(base, next);
    }

    #[test]
    fn appearance_defaults_and_roundtrip() {
        let cfg = Config::default();
        assert_eq!(cfg.appearance.theme, "zouwu-warm");
        assert_eq!(cfg.appearance.skin, "procedural");

        let text = cfg.to_toml();
        let back = Config::from_toml(&text).expect("roundtrip");
        assert_eq!(back.appearance, cfg.appearance);
        assert!(
            text.contains("[appearance]"),
            "appearance must be serialized, got:
{text}"
        );
    }

    #[test]
    fn v2_config_gets_appearance_defaults() {
        let cfg = Config::from_toml(
            "[general]
language = \"en-US\"
",
        )
        .expect("parse");
        assert_eq!(cfg.general.language, "en-US");
        assert_eq!(cfg.appearance.theme, "zouwu-warm");
        assert_eq!(cfg.migrated().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn log_section_defaults_and_roundtrip() {
        let cfg = Config::default();
        assert_eq!(cfg.log.level, "info");
        assert!(cfg.log.to_file);
        let text = cfg.to_toml();
        assert!(text.contains("[log]"), "log section must be serialized");
        let back = Config::from_toml(&text).expect("roundtrip");
        assert_eq!(back.log, cfg.log);
    }

    #[test]
    fn log_levels_are_validated() {
        assert!(LogCfg::is_valid_level("debug"));
        assert!(!LogCfg::is_valid_level("verbose"));
        assert_eq!(LogCfg::LEVELS[0], "off");
    }

    #[test]
    fn diff_reports_appearance_and_log() {
        let base = Config::default();
        let mut next = base.clone();
        next.appearance.theme = "zouwu-dark".into();
        next.log.level = "debug".into();
        assert_eq!(
            base.diff(&next),
            vec![ChangeKind::Appearance, ChangeKind::Log]
        );
    }

    #[test]
    fn eye_care_defaults_alpha_and_clamp() {
        let cfg = Config::default();
        assert!(!cfg.eye_care.enabled, "必须默认关着，不能擅自把屏幕染黄");
        assert_eq!(cfg.eye_care.strength, 40);
        assert_eq!(cfg.eye_care.alpha(), 56, "40% 强度 -> 56/255 不透明度");
        assert_eq!(
            EyeCare {
                enabled: true,
                strength: 0
            }
            .alpha(),
            0,
            "强度 0 不该有任何颜色"
        );
        assert_eq!(
            EyeCare {
                enabled: true,
                strength: EyeCare::MAX_STRENGTH
            }
            .alpha(),
            140,
            "最强也只到 ~55%"
        );
        // 手改 toml 能把强度写成 240（u8 装得下），加载时要收敛。
        let cfg = Config::from_toml("[eye_care]\nenabled = true\nstrength = 240\n").expect("parse");
        assert_eq!(cfg.eye_care.strength, 240);
        let cfg = cfg.migrated();
        assert_eq!(cfg.eye_care.strength, EyeCare::MAX_STRENGTH);
        assert_eq!(cfg.eye_care.alpha(), 140);
    }

    #[test]
    fn eye_care_section_roundtrips() {
        let mut cfg = Config::default();
        cfg.eye_care = EyeCare {
            enabled: true,
            strength: 72,
        };
        let text = cfg.to_toml();
        assert!(text.contains("[eye_care]"), "got:\n{text}");
        assert_eq!(
            Config::from_toml(&text).expect("roundtrip").eye_care,
            cfg.eye_care
        );
    }

    #[test]
    fn old_config_without_eye_care_stays_off() {
        let cfg = Config::from_toml("[general]\nlanguage = \"en-US\"\n").expect("parse");
        assert_eq!(cfg.eye_care, EyeCare::default());
    }

    #[test]
    fn diff_reports_eye_care() {
        let base = Config::default();
        let mut next = base.clone();
        next.eye_care.enabled = true;
        assert_eq!(base.diff(&next), vec![ChangeKind::EyeCare]);
        next.eye_care.strength = 80;
        assert_eq!(base.diff(&next), vec![ChangeKind::EyeCare]);
    }

    #[test]
    fn alt_drag_is_on_by_default_and_roundtrips() {
        let cfg = Config::default();
        assert!(cfg.alt_drag.enabled, "用户要的就是“装上就能用”");

        let text = cfg.to_toml();
        assert!(text.contains("[alt_drag]"), "got:\n{text}");
        assert_eq!(
            Config::from_toml(&text).expect("roundtrip").alt_drag,
            cfg.alt_drag
        );

        // 老配置里没有 `[alt_drag]` 这一段：补全成默认值（开着），不是关掉。
        let old = Config::from_toml("[general]\nlanguage = \"en-US\"\n").expect("parse");
        assert_eq!(old.alt_drag, AltDrag::default());
    }

    #[test]
    fn diff_reports_alt_drag() {
        let base = Config::default();
        let mut next = base.clone();
        next.alt_drag.enabled = false;
        assert_eq!(base.diff(&next), vec![ChangeKind::AltDrag]);
    }

    #[test]
    fn ring_cfg_converts_degrees() {
        let rc = RingCfg::default().to_ring_config();
        assert!((rc.orbit_start + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn hover_and_collapse_radius_are_independent() {
        let i = Interaction::default();
        // 悬停只看本体：56 × 1.1 = 61.6
        assert!((i.hover_radius(56.0) - 61.6).abs() < 1e-4);
        // 收起只看环带外沿：(112 + 24*2) × 1.1 = 176
        assert!((i.collapse_radius(112.0, 24.0) - 176.0).abs() < 1e-4);

        // 调大悬停热区不该把收起边界也放大
        let wider_hover = Interaction {
            hover_radius_factor: 2.0,
            ..Default::default()
        };
        assert!((wider_hover.hover_radius(56.0) - 112.0).abs() < 1e-4);
        assert!((wider_hover.collapse_radius(112.0, 24.0) - 176.0).abs() < 1e-4);

        // 调大收起边界也不该影响悬停热区
        let wider_band = Interaction {
            collapse_radius_factor: 1.5,
            ..Default::default()
        };
        assert!((wider_band.collapse_radius(112.0, 24.0) - 240.0).abs() < 1e-4);
        assert!((wider_band.hover_radius(56.0) - 61.6).abs() < 1e-4);
    }

    #[test]
    fn old_config_without_hover_factor_still_loads() {
        // 旧配置里只有 collapse_radius_factor，新字段靠 serde default 补 1.1。
        let cfg = Config::from_toml("[interaction]\ncollapse_radius_factor = 1.4\n").unwrap();
        assert!((cfg.interaction.collapse_radius_factor - 1.4).abs() < 1e-6);
        assert!((cfg.interaction.hover_radius_factor - 1.1).abs() < 1e-6);
    }
}
