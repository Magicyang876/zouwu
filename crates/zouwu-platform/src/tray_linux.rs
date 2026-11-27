//! 系统托盘（Linux：StatusNotifierItem / `org.kde.StatusNotifierItem`）。
//!
//! 托盘是小工具"退到后台但不消失"的落点：关掉窗口 = 缩进托盘，托盘左键 =
//! 呼出，右键 = 菜单（显示/隐藏、设置、开机自启、语言、退出）。
//!
//! 三条和 Windows 版一致的产品约束：
//!
//! 1. **菜单项文字不能自己拼**：一律走 `I18n`，切换语言时原地改文字，
//!    而不是重建托盘（重建会闪一下，还可能丢图标）。
//! 2. **事件要主动轮询**：`poll()` 在 `App::about_to_wait` 里被定时调用，
//!    它也是唤醒源之一。
//! 3. 左键 = 呼出，右键 = 菜单（"小宠物"的心智，不是"系统服务的托盘图标"）。
//!
//! 实现上和 Windows 版的差别（都是平台逼的，不是偷懒）：
//!
//! * 用 **ksni**（纯 D-Bus）而不是 `tray-icon`/libappindicator：后者要求
//!   "在初始化了 GTK 的那个线程上建、并且那个线程要跑 GTK 主循环"，
//!   而我们的事件循环是 winit 的。SNI 是纯消息协议，没有线程亲缘关系，
//!   服务自己起一条线程跑，菜单内容由 `Tray` 里的一份状态生成。
//! * **菜单点击没有图标点击事件**：SNI 的 `Activate`（左键）我们有，
//!   但 AppIndicator 规范里右键菜单由面板负责弹出，客户端拿不到"点了图标"
//!   这件事之外的细节——所以这里只把 `Activate` 映射成 `Toggle`。
//! * 勾选/文字都要通过 `Handle::update` 改状态，菜单随之重建；
//!   面板侧看到的是一次 `layoutUpdated`，不会闪。

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use ksni::Icon;
use zouwu_core::i18n::{I18n, AUTO, LANGUAGES};

use zouwu_core::theme::Theme;

pub const ID_SHOW_HIDE: &str = "zouwu.show-hide";
pub const ID_SETTINGS: &str = "zouwu.settings";
pub const ID_CONFIG_DIR: &str = "zouwu.config-dir";
pub const ID_OPEN_LOGS: &str = "zouwu.open-logs";
pub const ID_RECENTER: &str = "zouwu.recenter";
pub const ID_AUTOSTART: &str = "zouwu.autostart";
pub const ID_QUIT: &str = "zouwu.quit";
pub const ID_LANG_AUTO: &str = "zouwu.lang.auto";
/// 主题项 id 形如 `zouwu.theme.zouwu-warm`；皮肤同理。
pub const ID_THEME_PREFIX: &str = "zouwu.theme.";
pub const ID_SKIN_PREFIX: &str = "zouwu.skin.";
pub const ID_SKIN_IMPORT: &str = "zouwu.skin-import";
pub const ID_SKIN_EXPORT: &str = "zouwu.skin-export";
pub const ID_FONT_DIR: &str = "zouwu.font-dir";
pub const ID_SYS_FONT: &str = "zouwu.sys-font";
pub const ID_SCAN_QR: &str = "zouwu.scan-qr";
pub const ID_CLIPBOARD: &str = "zouwu.clipboard";

/// 和 Windows 版同一个枚举（`zouwu-ui` 只认它，不看平台）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    Hide,
    Toggle,
    Settings,
    OpenConfigDir,
    OpenLogs,
    OpenFontsDir,
    SysFont,
    ScanQr,
    Clipboard,
    Recenter,
    SetAutostart(bool),
    SetLanguage(&'static str),
    SetTheme(String),
    SetSkin(String),
    ImportSkin,
    ExportSkin,
    Quit,
}

/// `Tray::new` 的其余入参（与 Windows 版同构）。
pub struct TrayOptions<'a> {
    /// 32×32 非预乘 RGBA，由 `zouwu-ui::paint::tray_icon` 用同一套画笔渲染。
    pub icon_rgba: Option<(Vec<u8>, u32, u32)>,
    pub language_setting: &'a str,
    pub autostart: bool,
    pub visible: bool,
    pub theme_id: &'a str,
    pub skin_id: &'a str,
}

/// 菜单要显示的一切都由这份状态生成：改一个字段，菜单跟着变。
struct TrayState {
    tx: Sender<TrayCommand>,
    /// i18n 的文案在构造时就取成字符串（ksni 的菜单是"快照式"的，
    /// 不持有 `I18n` 引用也能保持简单）。
    text: Texts,
    visible: bool,
    autostart: bool,
    language: String,
    theme_id: String,
    skin_id: String,
    themes: Vec<(String, String)>,
    skins: Vec<(String, String)>,
    icon: Vec<Icon>,
}

/// 所有会随语言变化的文字。集中在一处，`apply_language` 就只改这一个结构。
#[derive(Clone)]
struct Texts {
    show: String,
    hide: String,
    settings: String,
    config: String,
    open_logs: String,
    recenter: String,
    scan_qr: String,
    clipboard: String,
    autostart: String,
    quit: String,
    language: String,
    language_auto: String,
    theme: String,
    skin: String,
    skin_import: String,
    skin_export: String,
    font_dir: String,
    sys_font: String,
}

impl Texts {
    fn new(i18n: &I18n) -> Self {
        Self {
            show: i18n.t("tray.show"),
            hide: i18n.t("tray.hide"),
            settings: i18n.t("tray.settings"),
            config: i18n.t("tray.config"),
            open_logs: i18n.t("tray.open-logs"),
            recenter: i18n.t("tray.recenter"),
            scan_qr: i18n.t("tray.scan-qr"),
            clipboard: i18n.t("tray.clipboard"),
            autostart: i18n.t("tray.autostart"),
            quit: i18n.t("tray.quit"),
            language: i18n.t("tray.language"),
            language_auto: i18n.t("tray.language.auto"),
            theme: i18n.t("tray.theme"),
            skin: i18n.t("tray.skin"),
            skin_import: i18n.t("tray.skin.import"),
            skin_export: i18n.t("tray.skin.export"),
            font_dir: i18n.t("tray.font-dir"),
            sys_font: i18n.t("tray.sys-font"),
        }
    }
}

/// 非预乘 RGBA → SNI 要的 ARGB32（网络字节序）。
fn to_icon(rgba: &[u8], width: u32, height: u32) -> Icon {
    let mut data = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        data.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    Icon {
        width: width as i32,
        height: height as i32,
        data,
    }
}

fn standard(label: impl Into<String>, tx: &Sender<TrayCommand>, cmd: TrayCommand) -> MenuItem<TrayState> {
    let tx = tx.clone();
    MenuItem::Standard(StandardItem {
        label: label.into(),
        activate: Box::new(move |_tray: &mut TrayState| {
            let _ = tx.send(cmd.clone());
        }),
        ..Default::default()
    })
}

fn checkmark(
    label: impl Into<String>,
    checked: bool,
    tx: &Sender<TrayCommand>,
    cmd: TrayCommand,
) -> MenuItem<TrayState> {
    let tx = tx.clone();
    MenuItem::Checkmark(CheckmarkItem {
        label: label.into(),
        checked,
        activate: Box::new(move |_tray: &mut TrayState| {
            let _ = tx.send(cmd.clone());
        }),
        ..Default::default()
    })
}

impl ksni::Tray for TrayState {
    // 左键点图标 = 呼出/收起，不要让面板弹菜单。
    const MENU_ON_ACTIVATE: bool = false;

    fn id(&self) -> String {
        // 规范要求"应用级唯一、跨会话稳定"。
        "zouwu".into()
    }

    fn title(&self) -> String {
        "驺吾".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icon.clone()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayCommand::Toggle);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let tx = &self.tx;
        let show_hide = if self.visible {
            self.text.hide.clone()
        } else {
            self.text.show.clone()
        };
        let mut items = vec![
            standard(show_hide, tx, TrayCommand::Toggle),
            standard(&self.text.scan_qr, tx, TrayCommand::ScanQr),
            standard(&self.text.clipboard, tx, TrayCommand::Clipboard),
            standard(&self.text.recenter, tx, TrayCommand::Recenter),
            standard(&self.text.settings, tx, TrayCommand::Settings),
            MenuItem::Separator,
            standard(&self.text.config, tx, TrayCommand::OpenConfigDir),
            standard(&self.text.open_logs, tx, TrayCommand::OpenLogs),
            standard(&self.text.font_dir, tx, TrayCommand::OpenFontsDir),
            standard(&self.text.sys_font, tx, TrayCommand::SysFont),
            MenuItem::Separator,
            checkmark(
                &self.text.autostart,
                self.autostart,
                tx,
                // 勾选状态由面板在点击后翻转（ksni 会用回调改过的状态重建菜单），
                // 但真正的开关由 UI 层调 `set_enabled` 决定，这里只发"想要什么"。
                TrayCommand::SetAutostart(!self.autostart),
            ),
        ];

        // ---- 语言
        let mut langs = vec![checkmark(
            &self.text.language_auto,
            self.language == AUTO,
            tx,
            TrayCommand::SetLanguage(AUTO),
        )];
        for code in LANGUAGES {
            langs.push(checkmark(
                language_label(code),
                self.language == code,
                tx,
                TrayCommand::SetLanguage(code),
            ));
        }
        items.push(MenuItem::SubMenu(SubMenu {
            label: self.text.language.clone(),
            submenu: langs,
            ..Default::default()
        }));

        // ---- 主题
        let themes = self
            .themes
            .iter()
            .map(|(id, name)| {
                checkmark(
                    name,
                    *id == self.theme_id,
                    tx,
                    TrayCommand::SetTheme(id.clone()),
                )
            })
            .collect();
        items.push(MenuItem::SubMenu(SubMenu {
            label: self.text.theme.clone(),
            submenu: themes,
            ..Default::default()
        }));

        // ---- 皮肤（导入/导出放在最下面：它们改的是列表，不是选中项）
        let mut skins: Vec<MenuItem<Self>> = self
            .skins
            .iter()
            .map(|(id, name)| {
                checkmark(
                    name,
                    *id == self.skin_id,
                    tx,
                    TrayCommand::SetSkin(id.clone()),
                )
            })
            .collect();
        skins.push(MenuItem::Separator);
        skins.push(standard(&self.text.skin_import, tx, TrayCommand::ImportSkin));
        skins.push(standard(&self.text.skin_export, tx, TrayCommand::ExportSkin));
        items.push(MenuItem::SubMenu(SubMenu {
            label: self.text.skin.clone(),
            submenu: skins,
            ..Default::default()
        }));

        items.push(MenuItem::Separator);
        items.push(standard(&self.text.quit, tx, TrayCommand::Quit));
        items
    }
}

pub struct Tray {
    handle: Handle<TrayState>,
    rx: Receiver<TrayCommand>,
    /// 最新的勾选/文案状态在 ksni 那边；这里只留一份给 `poll` 用的镜像没必要，
    /// 但自启勾选要能被 UI 层读回来，所以留一个。
    state: Mutex<TrayMirror>,
}

#[derive(Default)]
struct TrayMirror {
    themes: Vec<String>,
    skins: Vec<String>,
}

impl Tray {
    pub fn new(i18n: &I18n, themes: &[Theme], opts: TrayOptions<'_>) -> Result<Self, String> {
        let TrayOptions {
            icon_rgba,
            language_setting,
            autostart,
            visible,
            theme_id,
            skin_id,
        } = opts;
        let (tx, rx) = mpsc::channel();
        let skins = skin_choices(i18n);
        let state = TrayState {
            tx,
            text: Texts::new(i18n),
            visible,
            autostart,
            language: language_setting.to_string(),
            theme_id: theme_id.to_string(),
            skin_id: skin_id.to_string(),
            themes: themes
                .iter()
                .map(|theme| (theme.id.clone(), theme.name.clone()))
                .collect(),
            skins,
            icon: icon_rgba
                .as_ref()
                .map(|(rgba, w, h)| vec![to_icon(rgba, *w, *h)])
                .unwrap_or_default(),
        };
        let mirror = TrayMirror {
            themes: state.themes.iter().map(|(id, _)| id.clone()).collect(),
            skins: state.skins.iter().map(|(id, _)| id.clone()).collect(),
        };
        // ksni 自己的服务线程；我们这边只拿一个可以 `update` 的句柄。
        let handle = state
            .spawn()
            .map_err(|err| format!("tray (StatusNotifierItem) unavailable: {err}"))?;
        log::info!(
            "tray icon installed (language `{language_setting}`, autostart {} , window {})",
            if autostart { "on" } else { "off" },
            if visible { "visible" } else { "hidden" }
        );
        Ok(Self {
            handle,
            rx,
            state: Mutex::new(mirror),
        })
    }

    /// 换主题/换皮肤后刷新托盘那一小格图标。
    pub fn set_icon(&self, icon_rgba: Option<(Vec<u8>, u32, u32)>) {
        let Some((rgba, w, h)) = icon_rgba else {
            return;
        };
        let icon = vec![to_icon(&rgba, w, h)];
        let _ = self.handle.update(|state| state.icon = icon.clone());
    }

    pub fn apply_theme(&self, theme_id: &str) {
        let id = theme_id.to_string();
        let _ = self.handle.update(move |state| state.theme_id = id.clone());
    }

    pub fn apply_skin(&self, skin_id: &str) {
        let id = skin_id.to_string();
        let _ = self.handle.update(move |state| state.skin_id = id.clone());
    }

    pub fn set_visible_state(&self, _i18n: &I18n, visible: bool) {
        let _ = self.handle.update(move |state| state.visible = visible);
    }

    pub fn set_autostart_checked(&self, on: bool) {
        let _ = self.handle.update(move |state| state.autostart = on);
    }

    pub fn apply_language(&self, i18n: &I18n, language_setting: &str, visible: bool) {
        let text = Texts::new(i18n);
        let language = language_setting.to_string();
        let _ = self.handle.update(move |state| {
            state.text = text.clone();
            state.language = language.clone();
            state.visible = visible;
        });
    }

    /// 排干菜单事件，返回第一条有意义的命令。
    pub fn poll(&self) -> Option<TrayCommand> {
        let mut fired = None;
        while let Ok(cmd) = self.rx.try_recv() {
            // 连点时只认最后一条：动作都是"立即执行一次"。
            fired = Some(cmd);
        }
        fired
    }

    /// 菜单里注册过的主题/皮肤 id（测试与调试用）。
    #[allow(dead_code)]
    fn ids(&self) -> (Vec<String>, Vec<String>) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (state.themes.clone(), state.skins.clone())
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        // 服务线程是常驻的；显式叫停，免得进程退出时面板上还挂一个幽灵图标。
        let _ = self.handle.shutdown();
    }
}

/// 语言菜单里用**该语言自己的名字**（中文显示"简体中文"，英文显示"English"）。
pub fn language_label(code: &str) -> String {
    match code {
        "zh-CN" => "简体中文".to_string(),
        "en-US" => "English".to_string(),
        other => other.to_string(),
    }
}

/// 皮肤菜单项：`(id, 显示名)`。
fn skin_choices(i18n: &I18n) -> Vec<(String, String)> {
    crate::assets::skin_entries()
        .into_iter()
        .map(|entry| {
            let name = if entry.id == zouwu_core::skin::PROCEDURAL_SKIN_ID {
                i18n.t("settings.skin.procedural")
            } else {
                entry.name
            };
            (entry.id, name)
        })
        .collect()
}

/// 未使用的常量在这里"声明式地"保留：它们是协议 id，
/// 将来面板侧要按 id 发动作时用得上。
#[allow(dead_code)]
const _IDS: [&str; 16] = [
    ID_SHOW_HIDE,
    ID_SETTINGS,
    ID_CONFIG_DIR,
    ID_OPEN_LOGS,
    ID_RECENTER,
    ID_AUTOSTART,
    ID_QUIT,
    ID_LANG_AUTO,
    ID_THEME_PREFIX,
    ID_SKIN_PREFIX,
    ID_SKIN_IMPORT,
    ID_SKIN_EXPORT,
    ID_FONT_DIR,
    ID_SYS_FONT,
    ID_SCAN_QR,
    ID_CLIPBOARD,
];