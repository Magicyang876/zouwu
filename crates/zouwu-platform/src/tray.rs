//! 系统托盘（Windows 通知区域）。
//!
//! 托盘是小工具"退到后台但不消失"的落点：关掉窗口 = 缩进托盘，托盘左键 =
//! 呼出，右键 = 菜单（显示/隐藏、设置、开机自启、语言、退出）。
//!
//! 三条硬性约束，都是踩过的：
//!
//! 1. **必须在主线程创建**：Windows 托盘图标挂在创建它的线程的窗口上，
//!    `tray-icon`/`muda` 的菜单对象也是线程绑定的（`Rc` 内部），跨线程用会崩。
//! 2. **菜单项文字不能自己拼**：一律走 `I18n`，切换语言时用 `set_text`
//!    原地改，而不是重建托盘（重建会闪一下，还可能丢图标）。
//! 3. **事件要主动轮询**：`muda` 把点击投进全局 channel，winit 不会替我们转，
//!    所以由 `App::about_to_wait` 里定时 `poll()`（同时它也是唤醒源之一）。

use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
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
/// 注意：这两个 id **故意不用** `zouwu.skin.` 前缀，否则会被上面的
/// `strip_prefix` 当成"皮肤 id 叫 import"。
pub const ID_SKIN_IMPORT: &str = "zouwu.skin-import";
pub const ID_SKIN_EXPORT: &str = "zouwu.skin-export";
/// 打开字体目录。字体包的选择放在设置面板里（那里能看见实际效果），
/// 托盘只留一个"把目录翻出来"的入口 —— 用户想自己塞字体进去时不至于找不着路。
pub const ID_FONT_DIR: &str = "zouwu.font-dir";
/// 扫描二维码。排在显示/隐藏与回到主屏之后的"动作区"。
pub const ID_SCAN_QR: &str = "zouwu.scan-qr";
/// 打开剪贴板历史。与扫二维码同属“动作区”。
pub const ID_CLIPBOARD: &str = "zouwu.clipboard";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    Hide,
    Toggle,
    Settings,
    /// 打开配置目录（托盘里的快捷入口，不改任何东西）。
    OpenConfigDir,
    /// 打开日志目录。
    OpenLogs,
    /// 打开字体目录。
    OpenFontsDir,
    /// 扫全屏找二维码（结果用一个独立小窗展示）。
    ScanQr,
    /// 打开剪贴板历史小窗。
    Clipboard,
    /// 把小工具搬回主屏。存在的理由很具体：多屏拔掉之后，
    /// 旧坐标落在不存在的桌面上，用户看不见、也拖不到它。
    Recenter,
    SetAutostart(bool),
    SetLanguage(&'static str),
    /// 换配色（id → `themes/<id>.toml` 或内置）。
    SetTheme(String),
    /// 换形象（id → `skins/<id>/` 或 `procedural`）。
    SetSkin(String),
    /// 导入 `.zwskin`（会弹系统文件对话框）。
    ImportSkin,
    /// 导出当前皮肤到 `exports/`（会弹"另存为"）。
    ExportSkin,
    Quit,
}

/// 托盘菜单里需要随语言/状态改文字的项。
struct Items {
    show_hide: MenuItem,
    settings: MenuItem,
    config_dir: MenuItem,
    open_logs: MenuItem,
    scan_qr: MenuItem,
    clipboard: MenuItem,
    autostart: CheckMenuItem,
    quit: MenuItem,
    language_menu: Submenu,
    lang_auto: CheckMenuItem,
    /// `(语言代码, 菜单项)`，顺序与 `LANGUAGES` 一致。
    langs: Vec<(&'static str, CheckMenuItem)>,
    theme_menu: Submenu,
    /// `(主题 id, 菜单项)`，启动时扫一次（托盘菜单不动态刷新列表）。
    themes: Vec<(String, CheckMenuItem)>,
    skin_menu: Submenu,
    /// `(皮肤 id, 菜单项)`。
    skins: Vec<(String, CheckMenuItem)>,
    skin_import: MenuItem,
    skin_export: MenuItem,
    font_dir: MenuItem,
}

/// `Tray::new` 的其余入参。
///
/// 凑成结构体是因为 8 个位置参数里有 4 个是 `bool`/`&str`，调用处读起来像猜灯谜
/// （clippy 的 `too_many_arguments` 也是同一个抱怨）。
pub struct TrayOptions<'a> {
    /// 32×32 非预乘 RGBA，由 `zouwu-ui::paint::tray_icon` 用同一套画笔渲染。
    pub icon_rgba: Option<(Vec<u8>, u32, u32)>,
    pub language_setting: &'a str,
    pub autostart: bool,
    pub visible: bool,
    /// 当前主题/皮肤 id：决定菜单里哪一项打勾。
    pub theme_id: &'a str,
    pub skin_id: &'a str,
}

pub struct Tray {
    // 必须一直活着：drop 掉托盘图标就没了。
    icon: TrayIcon,
    items: Items,
}

impl Tray {
    /// `icon_rgba` 是 32×32 的非预乘 RGBA（由 UI 层用同一套画笔渲染，见
    /// `zouwu-ui::paint::tray_icon`），保证托盘图标和桌面上的小兽长得一样。
    pub fn new(i18n: &I18n, themes: &[Theme], opts: TrayOptions<'_>) -> Result<Self, String> {
        let TrayOptions {
            icon_rgba,
            language_setting,
            autostart,
            visible,
            theme_id,
            skin_id,
        } = opts;
        let show_hide = MenuItem::with_id(ID_SHOW_HIDE, show_hide_text(i18n, visible), true, None);
        let settings = MenuItem::with_id(ID_SETTINGS, i18n.t("tray.settings"), true, None);
        let config_dir = MenuItem::with_id(ID_CONFIG_DIR, i18n.t("tray.config"), true, None);
        let open_logs = MenuItem::with_id(ID_OPEN_LOGS, i18n.t("tray.open-logs"), true, None);
        let recenter = MenuItem::with_id(ID_RECENTER, i18n.t("tray.recenter"), true, None);
        let scan_qr = MenuItem::with_id(ID_SCAN_QR, i18n.t("tray.scan-qr"), true, None);
        let clipboard = MenuItem::with_id(ID_CLIPBOARD, i18n.t("tray.clipboard"), true, None);
        let autostart_item = CheckMenuItem::with_id(
            ID_AUTOSTART,
            i18n.t("tray.autostart"),
            true,
            autostart,
            None,
        );
        let quit = MenuItem::with_id(ID_QUIT, i18n.t("tray.quit"), true, None);

        let language_menu = Submenu::with_id("zouwu.lang", i18n.t("tray.language"), true);
        let lang_auto = CheckMenuItem::with_id(
            ID_LANG_AUTO,
            i18n.t("tray.language.auto"),
            true,
            language_setting == AUTO,
            None,
        );
        let mut langs = Vec::with_capacity(LANGUAGES.len());
        for code in LANGUAGES {
            let item = CheckMenuItem::with_id(
                format!("zouwu.lang.{code}"),
                language_label(code),
                true,
                language_setting == code,
                None,
            );
            language_menu
                .append(&item)
                .map_err(|err| format!("menu append failed: {err}"))?;
            langs.push((code, item));
        }
        language_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(|err| format!("menu append failed: {err}"))?;
        language_menu
            .append(&lang_auto)
            .map_err(|err| format!("menu append failed: {err}"))?;

        // ---- 主题 / 皮肤：都是"启动时扫一次"的静态列表。
        // 新装的主题重启后出现——换主题不是高频操作，为此做目录监听不值得。
        let theme_menu = Submenu::with_id("zouwu.theme", i18n.t("tray.theme"), true);
        let mut theme_items = Vec::with_capacity(themes.len());
        for entry in themes {
            let item = CheckMenuItem::with_id(
                format!("{ID_THEME_PREFIX}{}", entry.id),
                entry.name.clone(),
                true,
                entry.id == theme_id,
                None,
            );
            theme_menu
                .append(&item)
                .map_err(|err| format!("menu append failed: {err}"))?;
            theme_items.push((entry.id.clone(), item));
        }

        let skin_menu = Submenu::with_id("zouwu.skin", i18n.t("tray.skin"), true);
        let skins = skin_choices(i18n);
        let mut skin_items = Vec::with_capacity(skins.len());
        for entry in &skins {
            let item = CheckMenuItem::with_id(
                format!("{ID_SKIN_PREFIX}{}", entry.0),
                entry.1.clone(),
                true,
                entry.0 == skin_id,
                None,
            );
            skin_menu
                .append(&item)
                .map_err(|err| format!("menu append failed: {err}"))?;
            skin_items.push((entry.0.clone(), item));
        }
        // 导入/导出放在皮肤菜单最下面：它们改的是"皮肤列表"，不是"当前选中项"。
        let skin_import = MenuItem::with_id(ID_SKIN_IMPORT, i18n.t("tray.skin.import"), true, None);
        let skin_export = MenuItem::with_id(ID_SKIN_EXPORT, i18n.t("tray.skin.export"), true, None);
        skin_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(|err| format!("menu append failed: {err}"))?;
        for item in [&skin_import, &skin_export] {
            skin_menu
                .append(item)
                .map_err(|err| format!("menu append failed: {err}"))?;
        }

        let font_dir = MenuItem::with_id(ID_FONT_DIR, i18n.t("tray.font-dir"), true, None);

        let menu = Menu::new();
        menu.append_items(&[
            &show_hide,
            &scan_qr,
            &clipboard,
            &recenter,
            &settings,
            &PredefinedMenuItem::separator(),
            &config_dir,
            &open_logs,
            &font_dir,
            &PredefinedMenuItem::separator(),
            &autostart_item,
            &language_menu,
            &theme_menu,
            &skin_menu,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|err| format!("menu build failed: {err}"))?;

        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(i18n.t("app.name"))
            // 左键直接呼出（不是弹菜单），符合"小宠物"的心智。
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(true);
        if let Some((rgba, w, h)) = icon_rgba {
            match tray_icon::Icon::from_rgba(rgba, w, h) {
                Ok(icon) => builder = builder.with_icon(icon),
                Err(err) => log::warn!("tray icon rejected: {err}"),
            }
        }
        let icon = builder
            .build()
            .map_err(|err| format!("tray build failed: {err}"))?;
        log::info!(
            "tray icon installed (language `{language_setting}`, autostart {} , window {})",
            if autostart { "on" } else { "off" },
            if visible { "visible" } else { "hidden" }
        );

        Ok(Self {
            icon,
            items: Items {
                show_hide,
                settings,
                config_dir,
                open_logs,
                scan_qr,
                clipboard,
                autostart: autostart_item,
                quit,
                language_menu,
                lang_auto,
                langs,
                theme_menu,
                themes: theme_items,
                skin_menu,
                skins: skin_items,
                skin_import,
                skin_export,
                font_dir,
            },
        })
    }

    /// 换主题/换皮肤后刷新托盘那一小格图标（用同一套画笔重画，见 `paint::tray_icon`）。
    pub fn set_icon(&self, icon_rgba: Option<(Vec<u8>, u32, u32)>) {
        let Some((rgba, w, h)) = icon_rgba else {
            return;
        };
        match tray_icon::Icon::from_rgba(rgba, w, h) {
            Ok(icon) => {
                if let Err(err) = self.icon.set_icon(Some(icon)) {
                    log::warn!("tray icon update failed: {err}");
                }
            }
            Err(err) => log::warn!("tray icon rejected: {err}"),
        }
    }

    /// 主题变了：只动勾选（文字不跟着变）。
    pub fn apply_theme(&self, theme_id: &str) {
        for (id, item) in &self.items.themes {
            item.set_checked(id == theme_id);
        }
    }

    pub fn apply_skin(&self, skin_id: &str) {
        for (id, item) in &self.items.skins {
            item.set_checked(id == skin_id);
        }
    }

    /// 按当前显示状态刷新"显示/隐藏"那一条。
    pub fn set_visible_state(&self, i18n: &I18n, visible: bool) {
        self.items.show_hide.set_text(show_hide_text(i18n, visible));
    }

    pub fn set_autostart_checked(&self, on: bool) {
        self.items.autostart.set_checked(on);
    }

    /// 语言变了：原地改文字 + 勾选状态，不重建托盘。
    pub fn apply_language(&self, i18n: &I18n, language_setting: &str, visible: bool) {
        self.items.show_hide.set_text(show_hide_text(i18n, visible));
        self.items.settings.set_text(i18n.t("tray.settings"));
        self.items.config_dir.set_text(i18n.t("tray.config"));
        self.items.open_logs.set_text(i18n.t("tray.open-logs"));
        self.items.scan_qr.set_text(i18n.t("tray.scan-qr"));
        self.items.clipboard.set_text(i18n.t("tray.clipboard"));
        self.items.font_dir.set_text(i18n.t("tray.font-dir"));
        self.items.autostart.set_text(i18n.t("tray.autostart"));
        self.items.quit.set_text(i18n.t("tray.quit"));
        self.items.language_menu.set_text(i18n.t("tray.language"));
        self.items.lang_auto.set_text(i18n.t("tray.language.auto"));
        self.items.lang_auto.set_checked(language_setting == AUTO);
        self.items.theme_menu.set_text(i18n.t("tray.theme"));
        self.items.skin_menu.set_text(i18n.t("tray.skin"));
        self.items.skin_import.set_text(i18n.t("tray.skin.import"));
        self.items.skin_export.set_text(i18n.t("tray.skin.export"));
        for (code, item) in &self.items.langs {
            item.set_text(language_label(code));
            item.set_checked(language_setting == *code);
        }
    }

    /// 排干菜单与图标事件，返回第一条有意义的命令。
    pub fn poll(&self) -> Option<TrayCommand> {
        // 图标点击：左键抬起 = 切换显隐；双击在 Windows 上也会先来两次 Click，
        // 所以这里只处理 Up，避免一次点击切两次。
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                return Some(TrayCommand::Toggle);
            }
        }

        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if let Some(cmd) = self.command_for(event.id()) {
                return Some(cmd);
            }
        }
        None
    }

    fn command_for(&self, id: &MenuId) -> Option<TrayCommand> {
        let id = id.as_ref();
        if id == ID_SHOW_HIDE {
            // 文字已经是"显示"/"隐藏"（互为反义），所以只发 Toggle，由 UI 层决定方向。
            return Some(TrayCommand::Toggle);
        }
        if id == ID_SETTINGS {
            return Some(TrayCommand::Settings);
        }
        if id == ID_CONFIG_DIR {
            return Some(TrayCommand::OpenConfigDir);
        }
        if id == ID_OPEN_LOGS {
            return Some(TrayCommand::OpenLogs);
        }
        if id == ID_RECENTER {
            return Some(TrayCommand::Recenter);
        }
        if id == ID_FONT_DIR {
            return Some(TrayCommand::OpenFontsDir);
        }
        if id == ID_SCAN_QR {
            return Some(TrayCommand::ScanQr);
        }
        if id == ID_CLIPBOARD {
            return Some(TrayCommand::Clipboard);
        }
        if id == ID_AUTOSTART {
            return Some(TrayCommand::SetAutostart(self.items.autostart.is_checked()));
        }
        if id == ID_QUIT {
            return Some(TrayCommand::Quit);
        }
        if id == ID_LANG_AUTO {
            return Some(TrayCommand::SetLanguage(AUTO));
        }
        for (code, item) in &self.items.langs {
            if item.id().as_ref() == id {
                return Some(TrayCommand::SetLanguage(code));
            }
        }
        if let Some(theme_id) = id.strip_prefix(ID_THEME_PREFIX) {
            return Some(TrayCommand::SetTheme(theme_id.to_string()));
        }
        if id == ID_SKIN_IMPORT {
            return Some(TrayCommand::ImportSkin);
        }
        if id == ID_SKIN_EXPORT {
            return Some(TrayCommand::ExportSkin);
        }
        if let Some(skin_id) = id.strip_prefix(ID_SKIN_PREFIX) {
            return Some(TrayCommand::SetSkin(skin_id.to_string()));
        }
        None
    }
}

fn show_hide_text(i18n: &I18n, visible: bool) -> String {
    if visible {
        i18n.t("tray.hide")
    } else {
        i18n.t("tray.show")
    }
}

/// 语言菜单里用**该语言自己的名字**（中文显示"简体中文"，英文显示"English"）。
/// 用界面语言去翻译语言名是常见的坏味道：英文用户看到"简体中文"不知道该不该点它。
/// 皮肤菜单项：`(id, 显示名)`。
///
/// `procedural` 没有清单，名字走 i18n；其余用清单里的 `name`（缺省用目录名）。
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

pub fn language_label(code: &str) -> String {
    match code {
        "zh-CN" => "简体中文".to_string(),
        "en-US" => "English".to_string(),
        other => other.to_string(),
    }
}
