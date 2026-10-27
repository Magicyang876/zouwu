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

pub const ID_SHOW_HIDE: &str = "zouwu.show-hide";
pub const ID_SETTINGS: &str = "zouwu.settings";
pub const ID_AUTOSTART: &str = "zouwu.autostart";
pub const ID_QUIT: &str = "zouwu.quit";
pub const ID_LANG_AUTO: &str = "zouwu.lang.auto";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    Hide,
    Toggle,
    Settings,
    SetAutostart(bool),
    SetLanguage(&'static str),
    Quit,
}

/// 托盘菜单里需要随语言/状态改文字的项。
struct Items {
    show_hide: MenuItem,
    settings: MenuItem,
    autostart: CheckMenuItem,
    quit: MenuItem,
    language_menu: Submenu,
    lang_auto: CheckMenuItem,
    /// `(语言代码, 菜单项)`，顺序与 `LANGUAGES` 一致。
    langs: Vec<(&'static str, CheckMenuItem)>,
}

pub struct Tray {
    // 必须一直活着：drop 掉托盘图标就没了。
    _icon: TrayIcon,
    items: Items,
}

impl Tray {
    /// `icon_rgba` 是 32×32 的非预乘 RGBA（由 UI 层用同一套画笔渲染，见
    /// `zouwu-ui::paint::tray_icon`），保证托盘图标和桌面上的小兽长得一样。
    pub fn new(
        i18n: &I18n,
        icon_rgba: Option<(Vec<u8>, u32, u32)>,
        language_setting: &str,
        autostart: bool,
        visible: bool,
    ) -> Result<Self, String> {
        let show_hide = MenuItem::with_id(ID_SHOW_HIDE, show_hide_text(i18n, visible), true, None);
        let settings = MenuItem::with_id(ID_SETTINGS, i18n.t("tray.config"), true, None);
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

        let menu = Menu::new();
        menu.append_items(&[
            &show_hide,
            &settings,
            &PredefinedMenuItem::separator(),
            &autostart_item,
            &language_menu,
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
            _icon: icon,
            items: Items {
                show_hide,
                settings,
                autostart: autostart_item,
                quit,
                language_menu,
                lang_auto,
                langs,
            },
        })
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
        self.items.settings.set_text(i18n.t("tray.config"));
        self.items.autostart.set_text(i18n.t("tray.autostart"));
        self.items.quit.set_text(i18n.t("tray.quit"));
        self.items.language_menu.set_text(i18n.t("tray.language"));
        self.items.lang_auto.set_text(i18n.t("tray.language.auto"));
        self.items.lang_auto.set_checked(language_setting == AUTO);
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
pub fn language_label(code: &str) -> String {
    match code {
        "zh-CN" => "简体中文".to_string(),
        "en-US" => "English".to_string(),
        other => other.to_string(),
    }
}
