//! 全局热键的**字符串规格**解析。
//!
//! 只做纯文本 ↔ 结构的转换，不碰系统 API（注册在 `zouwu-platform`）。
//! 放在核心层的原因：这是用户要写在 `config.toml` 里的东西，格式错误必须能
//! 被单测覆盖，且设置面板要能把它原样显示回去。

use std::fmt;

/// 一个按键（不含修饰键）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Space,
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    /// 字母键，永远以大写字形保存（实际注册与 Shift 无关）。
    Letter(char),
    /// 数字键 0-9。
    Digit(char),
    /// 功能键 F1-F24。
    Function(u8),
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Space => f.write_str("Space"),
            Key::Enter => f.write_str("Enter"),
            Key::Tab => f.write_str("Tab"),
            Key::Escape => f.write_str("Esc"),
            Key::Backspace => f.write_str("Backspace"),
            Key::Delete => f.write_str("Delete"),
            Key::Up => f.write_str("Up"),
            Key::Down => f.write_str("Down"),
            Key::Left => f.write_str("Left"),
            Key::Right => f.write_str("Right"),
            // 配置里统一存大写：`Ctrl+Q` 读起来像快捷键，`Ctrl+q` 像变量名。
            Key::Letter(c) => write!(f, "{}", c.to_ascii_uppercase()),
            Key::Digit(c) => write!(f, "{c}"),
            Key::Function(n) => write!(f, "F{n}"),
        }
    }
}

/// 一条热键规格：修饰键组合 + 主键。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Windows 键 / macOS Command 键。
    pub meta: bool,
    pub key: Key,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    Empty,
    MissingKey {
        text: String,
    },
    UnknownToken {
        token: String,
    },
    /// 没有修饰键的全局热键会把普通打字吃掉，直接禁止。
    NoModifier {
        key: Key,
    },
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HotkeyError::Empty => f.write_str("热键为空"),
            HotkeyError::MissingKey { text } => write!(f, "热键 `{text}` 缺少主键"),
            HotkeyError::UnknownToken { token } => write!(f, "无法识别的热键片段 `{token}`"),
            HotkeyError::NoModifier { key } => {
                write!(f, "热键 `{key}` 至少要带一个 Ctrl/Alt/Shift/Win")
            }
        }
    }
}

impl std::error::Error for HotkeyError {}

impl Hotkey {
    /// 解析 `"Alt+Space"`、`"ctrl+shift+s"`、`"Win+F1"` 之类的写法。
    ///
    /// 容错：大小写不敏感、允许 `+` 两侧有空格、`ctrl/control`、`alt/option`、
    /// `win/meta/super/cmd` 互为别名。
    pub fn parse(text: &str) -> Result<Self, HotkeyError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(HotkeyError::Empty);
        }

        let mut hk = Hotkey {
            ctrl: false,
            shift: false,
            alt: false,
            meta: false,
            key: Key::Space,
        };
        let mut have_key = false;
        let mut have_modifier = false;

        for token in trimmed.split('+').map(str::trim) {
            if token.is_empty() {
                continue;
            }
            match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => {
                    hk.ctrl = true;
                    have_modifier = true;
                }
                "shift" => {
                    hk.shift = true;
                    have_modifier = true;
                }
                "alt" | "option" => {
                    hk.alt = true;
                    have_modifier = true;
                }
                "win" | "meta" | "super" | "cmd" | "command" => {
                    hk.meta = true;
                    have_modifier = true;
                }
                _ => {
                    if have_key {
                        return Err(HotkeyError::UnknownToken {
                            token: token.to_string(),
                        });
                    }
                    hk.key = parse_key(token).ok_or_else(|| HotkeyError::UnknownToken {
                        token: token.to_string(),
                    })?;
                    have_key = true;
                }
            }
        }

        if !have_key {
            return Err(HotkeyError::MissingKey {
                text: trimmed.to_string(),
            });
        }
        if !have_modifier {
            return Err(HotkeyError::NoModifier { key: hk.key });
        }
        Ok(hk)
    }

    /// 解析失败时用于兜底的默认值。
    pub fn default_summon() -> Self {
        Hotkey {
            ctrl: false,
            shift: false,
            alt: true,
            meta: false,
            key: Key::Space,
        }
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::with_capacity(4);
        if self.ctrl {
            parts.push("Ctrl".into());
        }
        if self.alt {
            parts.push("Alt".into());
        }
        if self.shift {
            parts.push("Shift".into());
        }
        if self.meta {
            parts.push("Win".into());
        }
        parts.push(self.key.to_string());
        f.write_str(&parts.join("+"))
    }
}

fn parse_key(token: &str) -> Option<Key> {
    let lower = token.to_ascii_lowercase();
    let key = match lower.as_str() {
        "space" | "spacebar" => Key::Space,
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "esc" | "escape" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        _ => {
            let mut chars = token.chars();
            let (first, rest) = (chars.next()?, chars.next());
            if rest.is_some() {
                // 多字符：只可能是 F1..F24
                let n: u8 = lower.strip_prefix('f')?.parse().ok()?;
                return (1..=24).contains(&n).then_some(Key::Function(n));
            }
            if first.is_ascii_alphabetic() {
                Key::Letter(first.to_ascii_uppercase())
            } else if first.is_ascii_digit() {
                Key::Digit(first)
            } else {
                return None;
            }
        }
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_summon() {
        let hk = Hotkey::parse("Alt+Space").expect("parse");
        assert!(hk.alt && !hk.ctrl && !hk.shift && !hk.meta);
        assert_eq!(hk.key, Key::Space);
        assert_eq!(hk, Hotkey::default_summon());
    }

    #[test]
    fn is_case_and_space_insensitive() {
        let hk = Hotkey::parse(" ctrl + SHIFT + s ").expect("parse");
        assert!(hk.ctrl && hk.shift && !hk.alt);
        assert_eq!(hk.key, Key::Letter('S'));
        assert_eq!(hk.to_string(), "Ctrl+Shift+S");
    }

    #[test]
    fn modifiers_have_aliases() {
        let a = Hotkey::parse("Control+Option+Super+q").expect("parse");
        let b = Hotkey::parse("Ctrl+Alt+Win+Q").expect("parse");
        assert_eq!(a, b);
    }

    #[test]
    fn parses_function_and_named_keys() {
        assert_eq!(
            Hotkey::parse("Win+F12").expect("parse").key,
            Key::Function(12)
        );
        assert_eq!(Hotkey::parse("Ctrl+Esc").expect("parse").key, Key::Escape);
        assert!(Hotkey::parse("Alt+F25").is_err());
    }

    #[test]
    fn rejects_bare_keys() {
        assert_eq!(
            Hotkey::parse("A"),
            Err(HotkeyError::NoModifier {
                key: Key::Letter('A')
            })
        );
        assert!(Hotkey::parse("").is_err());
    }

    #[test]
    fn rejects_missing_or_extra_keys() {
        assert!(matches!(
            Hotkey::parse("Ctrl+Alt"),
            Err(HotkeyError::MissingKey { .. })
        ));
        assert!(matches!(
            Hotkey::parse("Ctrl+A+B"),
            Err(HotkeyError::UnknownToken { .. })
        ));
        assert!(matches!(
            Hotkey::parse("Ctrl+???"),
            Err(HotkeyError::UnknownToken { .. })
        ));
    }

    #[test]
    fn display_roundtrips() {
        for text in [
            "Ctrl+Alt+A",
            "Win+F1",
            "Shift+Space",
            "Ctrl+Alt+Shift+Win+Z",
        ] {
            let hk = Hotkey::parse(text).expect(text);
            assert_eq!(hk.to_string(), text);
            assert_eq!(Hotkey::parse(&hk.to_string()).expect("reparse"), hk);
        }
    }
}
