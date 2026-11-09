//! 改 Windows 自己的界面字体（noMeiryoUI 那一套做法）。
//!
//! 这一层只做**系统侧的搬运**：读当前值、写新值、拍快照、导出 `.reg`。
//! 界面在 `zouwu-ui` 的独立小窗里，选择逻辑不在这里。
//!
//! 本文件是**平台中立**的那一半：数据结构、`LOGFONT` 编码、`.reg` 导出、
//! 快照读写、pt↔像素换算。真正的 FFI 在 `sysfont_sys.rs`（非 Windows 用
//! `sysfont_stub.rs`），只有它需要跑在 Windows 上。
//!
//! ## 为什么用 `SystemParametersInfoW` 而不是直接写注册表
//!
//! `HKCU\Control Panel\Desktop\WindowMetrics` 下面存的是 `LOGFONT` 的裸二进制
//! （92 字节、UTF-16 定长数组）。手搓那串字节当然可行，但 `SPI_SETNONCLIENTMETRICS`
//! 做的是同一件事**外加校验与广播**：它把值写进同一个位置，还顺手通知所有窗口。
//! 自己写注册表等于把"格式对不对""要不要广播"全押在自己身上，收益为零。
//!
//! ## 为什么没有标题栏
//!
//! `lfCaptionFont` 是唯一**改坏了可能回不来**的一项：Windows 11 22H2 之后，
//! 标题栏字体被改过、再开着透明效果时标题文字会整体消失，noMeiryoUI 自己的文档
//! 都在劝用户别碰。所以 `Target` 里干脆没有它。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(target_os = "windows")]
#[path = "sysfont_sys.rs"]
mod sys;
#[cfg(not(target_os = "windows"))]
#[path = "sysfont_stub.rs"]
mod sys;

pub use sys::{
    apply, apply_with, caption_metrics, caption_supported, chrome_needs_repair, current,
    installed_faces, is_admin, os_build, restore, restore_chrome, set_substitute, substitute_face,
};

/// 96 DPI = 100% 缩放。所有像素换算的基准。
pub(crate) const DEFAULT_DPI: u32 = 96;

/// 能改的那几项。枚举顺序 = 界面顺序 = 快照顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Menu,
    Message,
    Status,
    /// 调色板标题（`lfSmCaptionFont`）。
    Palette,
    /// 标题栏（`lfCaptionFont`）。只在 Windows 11 22H2 之前的系统上生效，
    /// 之后的系统改它会让标题文字整体消失——见 `caption_supported`。
    Caption,
    /// 桌面图标文字（`SPI_SETICONTITLELOGFONT`，不属于 `NONCLIENTMETRICS`）。
    Icon,
}

impl Target {
    pub const ALL: [Target; 6] = [
        Target::Menu,
        Target::Message,
        Target::Status,
        Target::Palette,
        Target::Caption,
        Target::Icon,
    ];

    /// 短名：写快照、写 `.reg`、记日志都用它（**保持英文**，中文只出现在界面上）。
    pub fn id(self) -> &'static str {
        match self {
            Target::Menu => "menu",
            Target::Message => "message",
            Target::Status => "status",
            Target::Palette => "palette",
            Target::Caption => "caption",
            Target::Icon => "icon",
        }
    }

    /// `WindowMetrics` 里的值名；图标那一项不在那儿，返回 `None`。
    pub fn value_name(self) -> Option<&'static str> {
        match self {
            Target::Menu => Some("MenuFont"),
            Target::Message => Some("MessageFont"),
            Target::Status => Some("StatusFont"),
            Target::Palette => Some("SmCaptionFont"),
            Target::Caption => Some("CaptionFont"),
            Target::Icon => None,
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Target::ALL.into_iter().find(|t| t.id() == id)
    }
}

/// 一项字体：家族名 + 字高（`lfHeight`，负 = 字符高度）+ 字重（400 常规 / 700 粗）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub face: String,
    pub height: i32,
    pub weight: i32,
}

impl Entry {
    /// 常规体。`height` 用 `height_for_pt()` 算，不要手写像素值。
    pub fn new(face: impl Into<String>, height: i32) -> Self {
        Self {
            face: face.into(),
            height,
            weight: 400,
        }
    }

    /// 粗体版本。
    pub fn bold(mut self, bold: bool) -> Self {
        self.weight = if bold { 700 } else { 400 };
        self
    }

    pub fn is_bold(&self) -> bool {
        self.weight >= 600
    }

    /// 按系统 DPI 系数把字体高度换到“写进系统”的量纲。
    ///
    /// 我们读到的 `lfHeight` 是 96 dpi 名义像素，而 `SPI_SETNONCLIENTMETRICS`
    /// **会按 `AppliedDPI/96` 把整块除掉才存**。所以想让它读回来正好是 `height`，
    /// 写进去的必须是 `height * factor`——见 `scale_factor_from_dpi`。
    pub fn scaled(&self, factor: f32) -> Self {
        Self {
            face: self.face.clone(),
            height: scale_height(self.height, factor),
            weight: self.weight,
        }
    }
}

/// 单个高度按系数缩放（四舍五入，0 保持 0——0 在 `LOGFONT` 里是“默认值”而不是零高）。
pub fn scale_height(height: i32, factor: f32) -> i32 {
    if height == 0 || (factor - 1.0).abs() < 0.01 {
        return height;
    }
    (height as f32 * factor).round() as i32
}

/// `AppliedDPI / 96`：写进系统之前的缩放系数。
///
/// 为什不能用实测值反推：`written / got` 只能得到一个 4 % 宽的区间（写 -14 读回 -11，
/// 真实系数可能是 1.21..1.33）——1.25 和 1.2857 都能解释那一次读数，而它们对
/// “标题栏字体”这种只做原样回写的字段刚好差一次四舍五入：用错的那个写两遍，
/// -14 就会变成 -11、再变成 -10（真实的 1.25 是固定点）。
pub fn scale_factor_from_dpi(dpi: u32) -> f32 {
    if dpi < 96 {
        return 1.0;
    }
    let factor = dpi as f32 / 96.0;
    // 能取到的 DPI 就这么几档（100 %..=250 %）；超出这个范围的是脏数据，
    // 宁可当作 100 %——写错了会把用户的字体缩成什么样子是没法预料的。
    if (1.0..=2.5).contains(&factor) {
        factor
    } else {
        1.0
    }
}

/// 动手之前的样子。`items` 用 `Target::id()` 做键。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// `YYYY-MM-DD HH:MM:SS.mmm`，只给人看（排序靠文件名）。
    pub taken: String,
    pub items: Vec<(String, Entry)>,
    /// 边框/滚动条/标题栏宽度这些「装饰尺寸」。老快照没有这一项（`None`）。
    #[serde(default)]
    pub chrome: Option<Chrome>,
}

/// `NONCLIENTMETRICS` 里除字体之外的那些整数：边框、滚动条、标题栏/菜单的宽度。
///
/// 它们**不参与 DPI 换算**（写什么读回什么），但会被“整块乘系数”那种写法连累：
/// ADR-066 的第一版就是这么把用户的边框写到 36 px、滚动条写到 39 px 的。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chrome {
    pub border: i32,
    pub scroll_width: i32,
    pub scroll_height: i32,
    pub caption_width: i32,
    pub sm_caption_width: i32,
    pub menu_width: i32,
}

/// Windows 在 96 dpi 下的标准装饰尺寸（高 DPI 由系统自己放大，不要乘系数）。
pub const STANDARD_CHROME: Chrome = Chrome {
    border: 1,
    scroll_width: 17,
    scroll_height: 17,
    caption_width: 22,
    sm_caption_width: 16,
    menu_width: 16,
};

impl Chrome {
    /// 按 DPI 系数换成“写进系统”的量纲（SPI 会把值除掉才存，与 `lfHeight` 同理）。
    pub fn scaled(&self, factor: f32) -> Self {
        let scale = |value: i32| {
            if value == 0 || (factor - 1.0).abs() < 0.01 {
                value
            } else {
                ((value as f32) * factor).round() as i32
            }
        };
        Self {
            border: scale(self.border),
            scroll_width: scale(self.scroll_width),
            scroll_height: scale(self.scroll_height),
            caption_width: scale(self.caption_width),
            sm_caption_width: scale(self.sm_caption_width),
            menu_width: scale(self.menu_width),
        }
    }

    /// 看上去已经不像是人调出来的尺寸。用来在**写盘前**把被写坏的值接住。
    ///
    /// 上界都留了余量（没有哪个正常桌面会把滚动条调到 32 px 宽），下界卡 0 与负数。
    /// 标题栏宽度只要为 0 就是“不管”，而放大过的值会一路涨到几十。
    pub fn is_broken(&self) -> bool {
        let out = |value: i32, low: i32, high: i32| !(low..=high).contains(&value);
        out(self.border, 1, 8)
            || out(self.scroll_width, 9, 32)
            || out(self.scroll_height, 9, 32)
            // 宽度这几项 Win10+ 基本不看（标题按钮尺寸走主题），留宽一点，
            // 只抓“被乘过系数”的那种成倍放大。
            || out(self.caption_width, 0, 32)
            || out(self.sm_caption_width, 0, 28)
            || out(self.menu_width, 0, 28)
    }
}

impl Snapshot {
    pub fn get(&self, target: Target) -> Option<&Entry> {
        self.items
            .iter()
            .find(|(id, _)| id == target.id())
            .map(|(_, entry)| entry)
    }

    /// 快照里所有项的家族名去重（界面用来提示"原来是 XX"）。
    pub fn faces(&self) -> Vec<String> {
        let mut faces: Vec<String> = self.items.iter().map(|(_, e)| e.face.clone()).collect();
        faces.dedup();
        faces
    }
}

/// `LOGFONTW`：定长整型 + 64 字节 UTF-16 家族名，没有指针，32/64 位布局一致。
///
/// 是**平台中立**的：它只是个 `#[repr(C)]` 数据块，`NONCLIENTMETRICS` 与 `.reg`
/// 导出都用它拼字节，不需要 Windows 才能构造。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct LogFontW {
    pub(crate) height: i32,
    width: i32,
    escapement: i32,
    orientation: i32,
    pub(crate) weight: i32,
    pub(crate) italic: u8,
    pub(crate) underline: u8,
    pub(crate) strike_out: u8,
    pub(crate) char_set: u8,
    out_precision: u8,
    clip_precision: u8,
    quality: u8,
    pitch_and_family: u8,
    pub(crate) face_name: [u16; 32],
}

/// `DEFAULT_CHARSET`。亚洲字体靠它才被枚举出来（见 `LogFontW::default`）。
pub(crate) const DEFAULT_CHARSET: u8 = 1;

impl Default for LogFontW {
    fn default() -> Self {
        // 全零不是合法字体：`char_set` 至少给 `DEFAULT_CHARSET`，否则
        // `EnumFontFamiliesExW` 会按 `ANSI_CHARSET` 过滤，中日韩字体全看不见。
        Self {
            height: 0,
            width: 0,
            escapement: 0,
            orientation: 0,
            weight: 400,
            italic: 0,
            underline: 0,
            strike_out: 0,
            char_set: DEFAULT_CHARSET,
            out_precision: 0,
            clip_precision: 0,
            quality: 0,
            pitch_and_family: 0,
            face_name: [0; 32],
        }
    }
}

impl LogFontW {
    pub(crate) fn face(&self) -> String {
        let end = self.face_name.iter().position(|u| *u == 0).unwrap_or(32);
        String::from_utf16_lossy(&self.face_name[..end])
    }

    pub(crate) fn set_face(&mut self, face: &str) {
        self.face_name = [0; 32];
        // 最后一个 `u16` 必须留给结尾的 0：`LF_FACESIZE = 32` 是**含**终止符的容量，
        // 写满 32 个字符 Windows 会读越界（它按 C 字符串读）。
        for (slot, unit) in self.face_name.iter_mut().take(31).zip(face.encode_utf16()) {
            *slot = unit;
        }
    }
}

/// `NONCLIENTMETRICSW`（Vista 之后的完整版，含 `iPaddedBorderWidth`）。
///
/// `size` 必须由调用方填成本结构体大小，否则 SPI 直接失败。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct NonClientMetricsW {
    pub(crate) size: u32,
    border_width: i32,
    scroll_width: i32,
    scroll_height: i32,
    caption_width: i32,
    pub(crate) caption_height: i32,
    pub(crate) caption_font: LogFontW,
    sm_caption_width: i32,
    pub(crate) sm_caption_height: i32,
    pub(crate) sm_caption_font: LogFontW,
    menu_width: i32,
    pub(crate) menu_height: i32,
    pub(crate) menu_font: LogFontW,
    pub(crate) status_font: LogFontW,
    pub(crate) message_font: LogFontW,
    pub(crate) padded_border_width: i32,
}

impl Default for NonClientMetricsW {
    fn default() -> Self {
        Self {
            size: std::mem::size_of::<NonClientMetricsW>() as u32,
            border_width: 0,
            scroll_width: 0,
            scroll_height: 0,
            caption_width: 0,
            caption_height: 0,
            caption_font: LogFontW::default(),
            sm_caption_width: 0,
            sm_caption_height: 0,
            sm_caption_font: LogFontW::default(),
            menu_width: 0,
            menu_height: 0,
            menu_font: LogFontW::default(),
            status_font: LogFontW::default(),
            message_font: LogFontW::default(),
            padded_border_width: 0,
        }
    }
}

impl NonClientMetricsW {
    /// 只把**五个 `LOGFONT` 的字号**乘同一个系数（行高像素与 `size` 都不动）。
    ///
    /// 为什么只动字号：`SPI_SETNONCLIENTMETRICS` 写入时对 `lfHeight` 做了 DPI 换算
    /// （本机 125 %：写 -14 读回 -11），但**`iMenuHeight` / `iCaptionHeight` 这些
    /// 行高像素是原样存的**。把行高也乘上去，就会每写一次长高 25 %——实测把标题栏
    /// 从 23 px 顶到 263 px，`SM_CYCAPTION` 264，屏幕上是一条半屏高的标题栏。
    /// 字号不乘则是反过来的老毛病（ADR-066 前半段记的 -14 → -2）。
    ///
    /// 系数由调用方实测/推算得出（`scale_factor_from_dpi`），不在这里猜。
    pub(crate) fn scale_fonts(&mut self, factor: f32) {
        if (factor - 1.0).abs() < 0.01 {
            return;
        }
        let scale = |value: &mut i32| {
            if *value != 0 {
                *value = ((*value as f32) * factor).round() as i32;
            }
        };
        for font in [
            &mut self.caption_font,
            &mut self.sm_caption_font,
            &mut self.menu_font,
            &mut self.status_font,
            &mut self.message_font,
        ] {
            scale(&mut font.height);
            scale(&mut font.width);
        }
    }

    /// `(标题栏字体高度, 标题栏高度)`：自检与护栏都用它。
    pub(crate) fn caption_pair(&self) -> (i32, i32) {
        (self.caption_font.height, self.caption_height)
    }

    /// 把「装饰尺寸」拆出来（快照用）。
    pub(crate) fn chrome(&self) -> Chrome {
        Chrome {
            border: self.border_width,
            scroll_width: self.scroll_width,
            scroll_height: self.scroll_height,
            caption_width: self.caption_width,
            sm_caption_width: self.sm_caption_width,
            menu_width: self.menu_width,
        }
    }

    pub(crate) fn set_chrome(&mut self, chrome: Chrome) {
        self.border_width = chrome.border;
        self.scroll_width = chrome.scroll_width;
        self.scroll_height = chrome.scroll_height;
        self.caption_width = chrome.caption_width;
        self.sm_caption_width = chrome.sm_caption_width;
        self.menu_width = chrome.menu_width;
    }
}

/// 从系统读回来的 `LOGFONT` 摘出我们关心的三个字段。
pub(crate) fn entry_of(font: &LogFontW) -> Entry {
    Entry {
        face: font.face(),
        height: font.height,
        weight: font.weight,
    }
}

/// 把我们关心的三个字段写进 `LOGFONT`，其余字段保持原样。
///
/// 斜体/下划线/删除线一律清掉：这些位留着会让新字体莫名其妙地歪着。
pub(crate) fn write_entry(font: &mut LogFontW, entry: &Entry) {
    font.set_face(&entry.face);
    font.height = entry.height;
    font.weight = entry.weight;
    font.char_set = DEFAULT_CHARSET;
    font.italic = 0;
    font.underline = 0;
    font.strike_out = 0;
}

/// pt → `lfHeight`（负的像素高度）。`dpi` 是参数而不是全局，才能写测试。
pub fn height_for_pt_dpi(pt: f32, dpi: u32) -> i32 {
    -((pt * dpi as f32 / 72.0).round() as i32).max(1)
}

/// 字号（pt）→ `lfHeight`。
///
/// **固定按 96 dpi 换算**（`-pt*96/72`），不跟进程 DPI 走。理由：
/// `NONCLIENTMETRICS` 里的 `lfHeight` 是物理像素，而 Windows 自己的经典字体对话框
/// 也是拿 96 dpi 当基准给用户看"9 pt"的。跟着 `GetDpiForSystem()` 走会让
/// "9 pt" 在 125% 屏幕上变成 -15，与系统对话框里同一个 9 pt 对不上。
pub fn height_for_pt(pt: f32) -> i32 {
    height_for_pt_dpi(pt, DEFAULT_DPI)
}

/// 从 `lfHeight` 反推 pt（界面显示"现在是多少 pt"用）。与 `height_for_pt` 互逆。
pub fn pt_for_height(height: i32) -> f32 {
    height.abs() as f32 * 72.0 / DEFAULT_DPI as f32
}

/// 写进去的值和系统读回来的值对不上时，推算下一次该写多少。
///
/// 为什么需要这个：`SPI_SETNONCLIENTMETRICS` 会把 `lfHeight` **按系统 DPI 缩放**
/// 后再存（本机 125% 下写 -14 读回 -11），而且这个换算与 `GetDpiForSystem()`
/// 在进程里返回什么无关——实测它返回 96，系统照样除以 1.25。写死了缩放规则的代码
/// 会在换一台机器后开始"每次应用都小一圈"，所以这里不猜规则，只测：
/// 写一次、读回来、按实测比例修正一次。
///
/// `desired` 是**最终想要的**，`written` 是这次实际写下去的，`got` 是读回来的。
/// 三者分开是因为第一次尝试时 `written == desired`，第二次就不是了：修正量必须
/// 相对"想要的"算，拿"上次写的"再乘一遍会越修越离谱。
///
/// 返回 `None` 表示已经对上（或无法判断），不需要再写。
pub fn calibrate(desired: i32, written: i32, got: i32) -> Option<i32> {
    if desired == 0 || written == 0 || got == 0 || desired == got {
        return None;
    }
    let factor = written as f32 / got as f32;
    let corrected = ((desired as f32) * factor).round() as i32;
    // 修正后跟这次写的没差别（四舍五入撞回来了）就没有意义了。
    (corrected != written && corrected != 0).then_some(corrected)
}

/// 标题栏的“字体高度 / 栏高”是不是已经不成样子了。
///
/// 我们**不提供**改标题栏字体的入口（Win11 22H2+ 改过之后标题文字会整体消失），
/// 所以这一对值要么是系统默认，要么是别处写坏的。判据就两条：字体小于 6 px，
/// 或者栏高塞不下字（不到字高 + 2）。坏了的症状很吓人：拦不住字的标题栏会缩成
/// 3~5 px，看起来就是“Windows 的标题栏消失了”——实测过，就是 `CaptionFont = -2`、
/// `iCaptionHeight = 3px`。
///
/// 两个参数都是 `SPI_GETNONCLIENTMETRICS` 返回的**像素**值（负数表示字符高度）。
///
/// 上限也得有：栏高比字高三倍多还没人这么设，那是垃圾值了（实测手抖写进一个
/// 10 位整数，`SM_CYCAPTION` 不变但经典风格的对话框会变形）。
pub fn caption_is_broken(font_height: i32, caption_height: i32, reference_height: i32) -> bool {
    let font = font_height.abs();
    let band = caption_height.abs();
    // 还有一条：标题栏字体比状态栏字体小一截就说明它被压过了。这两者系统默认是
    // 同号的，而我们每次写盘都会把“只做回写”的字段缩一点——查出来时已经 -2 了，
    // 上一版只拦“小到看不见”，结果留在 -10 上：栏还在，但比原来矮一截。
    font < 6 || band < font + 2 || band > 3 * font + 8 || font + 2 < reference_height.abs()
}

/// `%APPDATA%\zouwu\sysfont`。与用户数据放在一起，`--uninstall --purge` 时一并清掉。
pub fn snapshot_dir() -> PathBuf {
    crate::config_dir().join("sysfont")
}

/// 存一份快照，返回写出的路径。文件名带时间戳，所以**历史都在**：
/// 连着改三次，三次各自能还原。
pub fn save_snapshot(snapshot: &Snapshot) -> Option<PathBuf> {
    let dir = snapshot_dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        log::warn!("sysfont: cannot create {}: {err}", dir.display());
        return None;
    }
    let stamp = snapshot.taken.replace(':', "").replace(' ', "T");
    let path = dir.join(format!("backup-{stamp}.toml"));
    let body = toml::to_string_pretty(snapshot).ok()?;
    match std::fs::write(&path, body) {
        Ok(()) => {
            log::info!("sysfont: snapshot -> {}", path.display());
            Some(path)
        }
        Err(err) => {
            log::warn!("sysfont: cannot write {}: {err}", path.display());
            None
        }
    }
}

/// 最新一份快照（文件名里的时间戳可按字典序比较，所以不用解析时间）。
pub fn latest_snapshot() -> Option<(PathBuf, Snapshot)> {
    let mut best: Option<(String, PathBuf)> = None;
    for entry in std::fs::read_dir(snapshot_dir()).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("backup-") || !name.ends_with(".toml") {
            continue;
        }
        if best
            .as_ref()
            .map_or(true, |(best_name, _)| name > *best_name)
        {
            best = Some((name, entry.path()));
        }
    }
    let (_, path) = best?;
    let snapshot = read_snapshot(&path)?;
    Some((path, snapshot))
}

/// 读一份快照文件（`latest_snapshot` 与"从指定文件还原"共用）。
pub fn read_snapshot(path: &Path) -> Option<Snapshot> {
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

/// `LOGFONT` → 注册表里那串裸字节（92 字节，小端）。
fn logfont_bytes(entry: &Entry) -> Vec<u8> {
    let mut font = LogFontW::default();
    write_entry(&mut font, entry);
    let mut out = Vec::with_capacity(std::mem::size_of::<LogFontW>());
    for v in [
        font.height,
        font.width,
        font.escapement,
        font.orientation,
        font.weight,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.push(font.italic);
    out.push(font.underline);
    out.push(font.strike_out);
    out.push(font.char_set);
    out.push(font.out_precision);
    out.push(font.clip_precision);
    out.push(font.quality);
    out.push(font.pitch_and_family);
    for unit in font.face_name {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// 导出一份可双击的 `.reg`（`HKCU` 部分，不需要管理员）。
///
/// **只包含我们管的那几项**，不含标题栏——手滑双击一个带 `CaptionFont` 的 `.reg`
/// 正是这个功能最该防的事故。
pub fn to_reg(entries: &[(Target, Entry)]) -> String {
    let mut text = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
    text.push_str("[HKEY_CURRENT_USER\\Control Panel\\Desktop\\WindowMetrics]\r\n");
    for (target, entry) in entries {
        let Some(name) = target.value_name() else {
            continue;
        };
        let bytes = logfont_bytes(entry);
        let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
        text.push_str(&format!("\"{name}\"=hex:{}\r\n", hex.join(",")));
    }
    text.push_str("\r\n; 图标文字不在 WindowMetrics 里，本文件不覆盖它。\r\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logfont_keeps_the_whole_face_name() {
        let mut font = LogFontW::default();
        // `LF_FACESIZE = 32` 是含终止符的容量，所以最多 31 个字符。
        let full = "Source Han Sans SC Very Long Na";
        assert_eq!(full.encode_utf16().count(), 31);
        font.set_face(full);
        assert_eq!(font.face(), full);
        // 多一个字符就被截断，而且不会丢掉结尾的 0（丢了就是读越界）。
        font.set_face("Source Han Sans SC Very Long Nam");
        assert_eq!(font.face(), full);
        assert_eq!(font.face_name[31], 0);
        font.set_face("Sarasa Gothic SC");
        assert_eq!(font.face(), "Sarasa Gothic SC");
    }

    #[test]
    fn height_is_negative_and_scales_with_dpi() {
        assert_eq!(height_for_pt_dpi(9.0, 96), -12);
        assert_eq!(height_for_pt_dpi(9.0, 120), -15);
        assert_eq!(height_for_pt_dpi(12.0, 144), -24);
        // 任何字号都不会算出 0（0 在 LOGFONT 里 = "默认大小"，是个陷阱）
        assert!(height_for_pt_dpi(0.1, 96) < 0);
    }

    #[test]
    fn every_target_has_its_own_id_and_value_name() {
        let mut ids: Vec<&str> = Target::ALL.iter().map(|t| t.id()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count);

        // 图标不在 WindowMetrics 里；其余四项各有值名，且互不相同。
        let names: Vec<&str> = Target::ALL.iter().filter_map(|t| t.value_name()).collect();
        assert_eq!(names.len(), Target::ALL.len() - 1);
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        assert!(names.contains(&"CaptionFont"), "标题栏现在也是我们的一项");
        for id in ids {
            assert_eq!(Target::from_id(id).map(|t| t.id()), Some(id));
        }
        assert_eq!(Target::from_id("caption"), Some(Target::Caption));
        // 老快照里没有 caption 这一项，读出来是 None 而不是报错。
        assert_eq!(Target::from_id("caption2"), None);
    }

    #[test]
    fn reg_export_covers_only_what_we_own() {
        let entries = vec![
            (Target::Menu, Entry::new("Sarasa Gothic SC", -12)),
            (Target::Icon, Entry::new("Sarasa Gothic SC", -12)),
        ];
        let reg = to_reg(&entries);
        assert!(reg.starts_with("Windows Registry Editor Version 5.00"));
        assert!(reg.contains("\\WindowMetrics]\r\n"));
        assert!(reg.contains("\"MenuFont\"=hex:"));
        // 图标那一项不在 WindowMetrics 里，导出时自然缺席
        assert!(!reg.contains("IconFont"));
        assert!(!reg.contains("CaptionFont"));
        // 家族名以 UTF-16 小端出现在 hex 里
        let sarasa = "Sarasa Gothic SC"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(reg.contains(&sarasa));
    }

    #[test]
    fn logfont_bytes_match_the_documented_layout() {
        let bytes = logfont_bytes(&Entry::new("Segoe UI", -12));
        assert_eq!(bytes.len(), 92, "5*i32 + 8*u8 + 32*u16");
        assert_eq!(&bytes[0..4], &(-12i32).to_le_bytes());
        assert_eq!(&bytes[16..20], &400i32.to_le_bytes()); // lfWeight
        assert_eq!(bytes[23], DEFAULT_CHARSET); // lfCharSet
        assert_eq!(&bytes[28..30], &(u16::from(b'S')).to_le_bytes()); // lfFaceName
    }

    #[test]
    fn bold_sets_the_weight_the_win32_way() {
        let entry = Entry::new("Segoe UI", -12).bold(true);
        assert_eq!(entry.weight, 700);
        assert!(entry.is_bold());
        assert!(!Entry::new("Segoe UI", -12).is_bold());
    }

    #[test]
    fn calibration_compensates_the_dpi_scaling_of_spi() {
        // 本机实测：125% 缩放下写 -14 读回 -11。
        assert_eq!(calibrate(-14, -14, -11), Some(-18));
        // 拿 -18 再写一次，读回就是想要的 -14（实测确认过）→ 不用再动。
        assert_eq!(calibrate(-14, -18, -14), None, "已经对上了就别再动");
        // 完全一致时不动。
        assert_eq!(calibrate(-12, -12, -12), None);
        // 脏数据不猜。
        assert_eq!(calibrate(0, 0, -12), None);
        assert_eq!(calibrate(-12, -12, 0), None);
    }

    #[test]
    fn scaling_touches_font_sizes_but_never_band_heights() {
        let mut metrics = NonClientMetricsW::default();
        metrics.menu_font.height = -14;
        metrics.status_font.height = -14;
        metrics.caption_font.height = -14;
        metrics.sm_caption_height = 19;
        metrics.caption_height = 19;
        metrics.menu_height = 19;
        metrics.border_width = 4;
        let size = metrics.size;
        metrics.scale_fonts(1.25);
        // 字号要乘：系统写入时会按 DPI 除掉，不乘就是每写一次小一圈。
        assert_eq!(metrics.menu_font.height, -18);
        assert_eq!(metrics.status_font.height, -18);
        assert_eq!(metrics.caption_font.height, -18);
        // 行高像素**不能**乘：它们是原样存的，乘了就每写一次长高 25 %
        // （实测把标题栏从 23 px 顶到 263 px）。
        assert_eq!(metrics.menu_height, 19);
        assert_eq!(metrics.sm_caption_height, 19);
        assert_eq!(metrics.caption_height, 19);
        assert_eq!(metrics.border_width, 4);
        assert_eq!(metrics.size, size);
    }

    #[test]
    fn scaled_chrome_is_recognised_as_broken() {
        // 标准值本身必须健康——否则护栏会把每个用户的边框都“修”一遍。
        assert!(!STANDARD_CHROME.is_broken());
        // 实测的坏状态（被整块乘系数乘了四五次）：边框 36 px、滚动条 39 px。
        let broken = Chrome {
            border: 36,
            scroll_width: 39,
            scroll_height: 39,
            caption_width: 31,
            sm_caption_width: 49,
            menu_width: 49,
        };
        assert!(broken.is_broken());
        // 哪一项单独超界都算，因为写盘时是一整块一起写。
        for bad in [
            Chrome {
                border: 0,
                ..STANDARD_CHROME
            },
            Chrome {
                scroll_width: 60,
                ..STANDARD_CHROME
            },
            Chrome {
                scroll_height: 3,
                ..STANDARD_CHROME
            },
            Chrome {
                caption_width: 40,
                ..STANDARD_CHROME
            },
            Chrome {
                sm_caption_width: 49,
                ..STANDARD_CHROME
            },
            Chrome {
                menu_width: -8,
                ..STANDARD_CHROME
            },
        ] {
            assert!(bad.is_broken(), "{bad:?} 应该被判为坏");
        }
    }

    #[test]
    fn old_snapshots_without_chrome_still_load() {
        // 1.0.5 早期的快照没有 `chrome` 那一项，不能因此读不回来。
        let old = r#"
            taken = "2026-11-09 11:49:51.696"
            items = [["menu", { face = "Sarasa Gothic SC", height = -14, weight = 400 }]]
        "#;
        let snapshot: Snapshot = toml::from_str(old).expect("老快照要能读");
        assert_eq!(snapshot.chrome, None);
        assert_eq!(
            snapshot.get(Target::Menu).map(|e| e.face.as_str()),
            Some("Sarasa Gothic SC")
        );
    }

    #[test]
    fn broken_captions_are_recognised() {
        // 实测的坏状态：字体 2 px、标题栏 3 px —— 就是“标题栏消失”那个样子。
        assert!(caption_is_broken(-2, -48, -14));
        // 字体正常但栏高塞不下字，同样不能留。
        assert!(caption_is_broken(-18, 12, -14));
        // 健康：字体 18 px、栏高 23 px。
        assert!(!caption_is_broken(-18, 23, -14));
        assert!(!caption_is_broken(-14, 24, -14));
        // 边界：正好等于字高 + 2 就够了。
        assert!(!caption_is_broken(-18, 20, -14));
        assert!(caption_is_broken(-18, 19, -14));
        // 栏高远远超过字高（垃圾值）也算坏。
        assert!(caption_is_broken(-14, 1040088936, -14));
        assert!(!caption_is_broken(-14, 44, -14));
        // 栏高正常、字体却被压小了：栏还在但矮一截，也算坏。
        assert!(caption_is_broken(-10, 18, -14));
        assert!(!caption_is_broken(-13, 18, -14));
    }

    #[test]
    fn snapshot_survives_a_toml_roundtrip() {
        let snapshot = Snapshot {
            taken: "2026-11-09 10:15:00.000".to_string(),
            items: vec![
                ("menu".to_string(), Entry::new("Microsoft YaHei UI", -12)),
                ("icon".to_string(), Entry::new("Segoe UI", -15)),
            ],
            chrome: Some(STANDARD_CHROME),
        };
        let text = toml::to_string_pretty(&snapshot).expect("serialize");
        let back: Snapshot = toml::from_str(&text).expect("parse");
        assert_eq!(back, snapshot);
        assert_eq!(
            back.get(Target::Menu).map(|e| e.face.as_str()),
            Some("Microsoft YaHei UI")
        );
        assert_eq!(back.get(Target::Status), None);
        assert_eq!(back.faces().len(), 2);
    }
}

/// 一行数字，给人看/给日志用：六个字号 + 几个行高 + `iPaddedBorderWidth`。
///
/// 只读。排错时“到底哪个字段在变”比“读不出来”有用得多——标题栏那次事故里，
/// 肉眼能看到的只有标题栏变矮，实际在变的是 `lfCaptionFont.lfHeight`。
pub fn metrics_dump() -> Option<String> {
    sys::dump()
}
