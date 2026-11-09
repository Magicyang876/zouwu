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
    apply, current, installed_faces, is_admin, restore, set_substitute, substitute_face,
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
    /// 桌面图标文字（`SPI_SETICONTITLELOGFONT`，不属于 `NONCLIENTMETRICS`）。
    Icon,
}

impl Target {
    pub const ALL: [Target; 5] = [
        Target::Menu,
        Target::Message,
        Target::Status,
        Target::Palette,
        Target::Icon,
    ];

    /// 短名：写快照、写 `.reg`、记日志都用它（**保持英文**，中文只出现在界面上）。
    pub fn id(self) -> &'static str {
        match self {
            Target::Menu => "menu",
            Target::Message => "message",
            Target::Status => "status",
            Target::Palette => "palette",
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
}

/// 动手之前的样子。`items` 用 `Target::id()` 做键。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// `YYYY-MM-DD HH:MM:SS.mmm`，只给人看（排序靠文件名）。
    pub taken: String,
    pub items: Vec<(String, Entry)>,
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
        assert!(!names.contains(&"CaptionFont"), "标题栏不该被我们写");
        for id in ids {
            assert_eq!(Target::from_id(id).map(|t| t.id()), Some(id));
        }
        assert_eq!(Target::from_id("caption"), None);
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
    fn snapshot_survives_a_toml_roundtrip() {
        let snapshot = Snapshot {
            taken: "2026-11-09 10:15:00.000".to_string(),
            items: vec![
                ("menu".to_string(), Entry::new("Microsoft YaHei UI", -12)),
                ("icon".to_string(), Entry::new("Segoe UI", -15)),
            ],
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
