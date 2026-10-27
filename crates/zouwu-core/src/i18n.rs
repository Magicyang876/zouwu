//! 多语言（Fluent）。
//!
//! 设计要点：
//! - **资源内嵌**进二进制（`include_str!`），不做运行时文件加载 —— 小工具不该
//!   因为丢了一个 `.ftl` 就变成半中半英，也省掉启动期的 IO。
//! - **纯逻辑**：不碰系统 API。系统语言由 `zouwu-platform` 之外的 `sys-locale`
//!   读取（它本身是纯读取库，不创建窗口，放在核心层是安全的）。
//! - Fluent 的 id 不允许出现 `.`，而我们的动作 id 形如 `action.screenshot`，
//!   所以查找前统一做 `key_to_id` 归一化（`.` → `-`）。这样领域层可以继续用
//!   点分命名（配置/日志里更清楚），不必为了翻译改命名风格。
//! - 缺失键**返回键名本身**而不是 panic 或空串：漏翻时界面上看得见是哪个键，
//!   同时不会炸掉一个常驻程序。

use std::str::FromStr;

use fluent::{FluentArgs, FluentBundle, FluentResource};
use unic_langid::LanguageIdentifier;

const ZH_CN: &str = include_str!("../i18n/zh-CN/zouwu.ftl");
const EN_US: &str = include_str!("../i18n/en-US/zouwu.ftl");

/// 当前支持的界面语言（也是设置面板里的选项顺序）。
pub const LANGUAGES: [&str; 2] = ["zh-CN", "en-US"];

/// 配置里表示"跟随系统"的魔法值。
pub const AUTO: &str = "auto";

/// 兜底语言：系统语言不认识时用它。
pub const FALLBACK: &str = "en-US";

pub struct I18n {
    lang: &'static str,
    bundle: FluentBundle<FluentResource>,
}

impl std::fmt::Debug for I18n {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("I18n").field("lang", &self.lang).finish()
    }
}

impl Default for I18n {
    fn default() -> Self {
        Self::new(AUTO)
    }
}

impl I18n {
    /// `requested` 可以是 `"auto"`、`"zh-CN"`、`"en-US"`，或任何系统语言串
    /// （会按主语言标签匹配，例如 `zh-Hans-CN`/`en-GB`）。
    pub fn new(requested: &str) -> Self {
        let lang = resolve(requested);
        Self {
            lang,
            bundle: build_bundle(lang),
        }
    }

    /// 当前生效的语言（一定在 `LANGUAGES` 里）。
    pub fn language(&self) -> &'static str {
        self.lang
    }

    /// 配置里该存的值：跟随系统时存 `"auto"`，否则存具体语言。
    pub fn configured(&self, requested: &str) -> String {
        if requested == AUTO {
            AUTO.to_string()
        } else {
            self.lang.to_string()
        }
    }

    /// 切换语言；真的发生变化时返回 `true`（用于触发重绘/重建菜单）。
    pub fn set_language(&mut self, requested: &str) -> bool {
        let next = resolve(requested);
        if next == self.lang {
            return false;
        }
        self.lang = next;
        self.bundle = build_bundle(next);
        true
    }

    /// 取一条文案。缺失时返回键名本身。
    pub fn t(&self, key: &str) -> String {
        self.t_args(key, &[])
    }

    /// 带变量的文案，例如 `hint-hotkey` 里的 `{$hotkey}`。
    pub fn t_args(&self, key: &str, args: &[(&str, &str)]) -> String {
        let id = key_to_id(key);
        let Some(message) = self.bundle.get_message(id.as_ref()) else {
            log_missing(&id);
            return key.to_string();
        };
        let Some(pattern) = message.value() else {
            log_missing(&id);
            return key.to_string();
        };
        let mut errors = Vec::new();
        let text = if args.is_empty() {
            self.bundle.format_pattern(pattern, None, &mut errors)
        } else {
            let mut fargs = FluentArgs::new();
            for (name, value) in args {
                fargs.set(*name, *value);
            }
            self.bundle
                .format_pattern(pattern, Some(&fargs), &mut errors)
        };
        if let Some(err) = errors.first() {
            log::warn!("i18n format error in `{id}`: {err}");
        }
        // 格式化失败时 Fluent 会把占位符原样留在串里，仍然比空串可用。
        text.to_string()
    }

    /// 动作 id → 菜单/提示上的可读标签（走 `ActionItem::label_key`）。
    pub fn label(&self, label_key: &str) -> String {
        self.t(label_key)
    }
}

/// 把领域层的点分键名归一化成合法的 Fluent id。
pub fn key_to_id(key: &str) -> std::borrow::Cow<'_, str> {
    if key.contains('.') {
        std::borrow::Cow::Owned(key.replace('.', "-"))
    } else {
        std::borrow::Cow::Borrowed(key)
    }
}

/// 把一个"请求"解析成受支持的语言。
///
/// 匹配规则：`auto`/空 → 系统语言；否则按主语言标签（`zh`/`en`）匹配，
/// 都不沾边就退回 `FALLBACK`。这样 `zh-Hans-CN`、`en-GB` 这类系统串也能用。
pub fn resolve(requested: &str) -> &'static str {
    let requested = requested.trim();
    if requested.is_empty() || requested == AUTO {
        let system = sys_locale::get_locale().unwrap_or_default();
        return match_supported(&system);
    }
    match_supported(requested)
}

fn match_supported(tag: &str) -> &'static str {
    if let Ok(id) = LanguageIdentifier::from_str(tag) {
        let primary = id.language.as_str();
        for lang in LANGUAGES {
            if let Ok(known) = LanguageIdentifier::from_str(lang) {
                if known.language.as_str() == primary {
                    return lang;
                }
            }
        }
    }
    FALLBACK
}

fn build_bundle(lang: &'static str) -> FluentBundle<FluentResource> {
    let text = match lang {
        "zh-CN" => ZH_CN,
        _ => EN_US,
    };
    let res = FluentResource::try_new(text.to_string()).unwrap_or_else(|(res, errors)| {
        for err in &errors {
            log::error!("i18n resource error ({lang}): {err}");
        }
        res
    });
    let mut bundle = FluentBundle::new(vec![LanguageIdentifier::from_str(lang)
        .unwrap_or_else(|_| LanguageIdentifier::from_str(FALLBACK).expect("fallback langid"))]);
    // 允许覆盖：同一个键在资源里重复时保留最后一条，避免整包加载失败。
    bundle.set_use_isolating(false);
    if let Err(errors) = bundle.add_resource(res) {
        for err in &errors {
            log::error!("i18n add_resource error ({lang}): {err}");
        }
    }
    bundle
}

fn log_missing(id: &str) {
    log::warn!("i18n key missing: {id}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::default_ring;

    #[test]
    fn zh_and_en_differ() {
        let zh = I18n::new("zh-CN");
        let en = I18n::new("en-US");
        assert_eq!(zh.t("action.screenshot"), "截图");
        assert_eq!(en.t("action.screenshot"), "Screenshot");
        assert_eq!(zh.language(), "zh-CN");
    }

    #[test]
    fn every_default_action_has_a_label() {
        for lang in LANGUAGES {
            let i18n = I18n::new(lang);
            for item in default_ring() {
                let label = i18n.label(item.label_key);
                assert_ne!(
                    label, item.label_key,
                    "{lang} 缺少 {} 的文案",
                    item.label_key
                );
                assert!(!label.is_empty());
            }
        }
    }

    #[test]
    fn missing_key_falls_back_to_key_name() {
        let i18n = I18n::new("en-US");
        assert_eq!(i18n.t("nope.not.there"), "nope.not.there");
    }

    #[test]
    fn system_tag_matches_by_primary_language() {
        assert_eq!(match_supported("zh-Hans-CN"), "zh-CN");
        assert_eq!(match_supported("zh"), "zh-CN");
        assert_eq!(match_supported("en-GB"), "en-US");
        assert_eq!(match_supported("ja-JP"), FALLBACK);
        assert_eq!(match_supported(""), FALLBACK);
    }

    #[test]
    fn auto_resolves_to_supported_language() {
        let i18n = I18n::new(AUTO);
        assert!(LANGUAGES.contains(&i18n.language()));
    }

    #[test]
    fn set_language_reports_change() {
        let mut i18n = I18n::new("zh-CN");
        assert!(!i18n.set_language("zh-CN"));
        assert!(i18n.set_language("en-US"));
        assert_eq!(i18n.t("action.more"), "More");
        assert!(!i18n.set_language("ja-JP")); // 已回退到 en-US，无需重载
    }

    #[test]
    fn args_are_interpolated() {
        let i18n = I18n::new("zh-CN");
        let text = i18n.t_args("hint.hotkey", &[("hotkey", "Alt+Space")]);
        assert!(text.contains("Alt+Space"), "got {text}");
        assert!(!text.contains('{'), "占位符没被替换: {text}");
    }
}
