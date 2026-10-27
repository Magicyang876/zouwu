//! 主题令牌。渲染层只消费这里的颜色/参数，不写死色值。
//!
//! 主题分两层：
//! - **内置**（`zouwu-warm` / `zouwu-dark`）：编译进二进制的常量，永远可用，
//!   是"换肤失败也不会变丑"的兜底。
//! - **外部**（`%APPDATA%/zouwu/themes/*.toml`）：只会**增量覆盖**内置主题的若干令牌，
//!   所以用户想"把眼睛改成金色"不用抄 14 个字段，写 1 个就够。
//!
//! 为什么色值用 `#RRGGBB[AA]` 而不是 `0xRRGGBB` 或数组：设计师/剪辑软件里复制出来的
//! 就是这一种，贴着现实比贴着解析器重要。

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// 与非预乘 alpha 相乘（保持 RGB 不变）。
    pub fn with_alpha(self, a: f32) -> Self {
        Self {
            a: (self.a as f32 * a.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }

    pub fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let f = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Self {
            r: f(self.r, other.r),
            g: f(self.g, other.g),
            b: f(self.b, other.b),
            a: f(self.a, other.a),
        }
    }

    /// 解析 `#RRGGBB` / `#RRGGBBAA`（`#` 可省，大小写不限）。
    pub fn parse(text: &str) -> Result<Self, String> {
        let hex = text.trim().trim_start_matches('#');
        if hex.len() != 6 && hex.len() != 8 {
            return Err(format!(
                "color `{text}` must be #RRGGBB or #RRGGBBAA (got {} hex digits)",
                hex.len()
            ));
        }
        let byte = |i: usize| -> Result<u8, String> {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|_| format!("color `{text}` has a non-hex digit at {i}"))
        };
        Ok(Self {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: if hex.len() == 8 { byte(6)? } else { 255 },
        })
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
        }
    }
}

/// 一套调色板。`id`/`name` 用 `String` 是因为外部主题是运行时读进来的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub id: String,
    pub name: String,
    // 小兽
    pub body: Rgba,
    pub body_shade: Rgba,
    pub mane: Rgba,
    pub eye: Rgba,
    pub muzzle: Rgba,
    // 气泡
    pub bubble_bg: Rgba,
    pub bubble_edge: Rgba,
    pub bubble_bg_active: Rgba,
    pub bubble_edge_active: Rgba,
    pub glyph: Rgba,
    pub glyph_active: Rgba,
    // 通用
    pub shadow: Rgba,
    pub accent: Rgba,
    pub glow: Rgba,
}

/// 内部构造用：`(id, name, 14 个色值)` —— 与 [`Theme`] 字段一一对应，
/// 加新令牌时编译器会在这里报错，不会忘记补主题。
struct Builtin {
    id: &'static str,
    name: &'static str,
    body: Rgba,
    body_shade: Rgba,
    mane: Rgba,
    eye: Rgba,
    muzzle: Rgba,
    bubble_bg: Rgba,
    bubble_edge: Rgba,
    bubble_bg_active: Rgba,
    bubble_edge_active: Rgba,
    glyph: Rgba,
    glyph_active: Rgba,
    shadow: Rgba,
    accent: Rgba,
    glow: Rgba,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "zouwu-warm",
        name: "驺吾 · 暖橙",
        body: Rgba::rgb(0xE8, 0x7A, 0x4A),
        body_shade: Rgba::rgb(0xC9, 0x5F, 0x33),
        mane: Rgba::rgb(0xF6, 0xC0, 0x8C),
        eye: Rgba::rgb(0x2B, 0x1C, 0x16),
        muzzle: Rgba::rgb(0xFD, 0xE9, 0xD6),
        bubble_bg: Rgba::rgb(0x2A, 0x22, 0x22),
        bubble_edge: Rgba::new(0xF6, 0xC0, 0x8C, 130),
        bubble_bg_active: Rgba::rgb(0x3A, 0x2A, 0x24),
        bubble_edge_active: Rgba::rgb(0xF6, 0xC0, 0x8C),
        glyph: Rgba::rgb(0xF0, 0xD4, 0xBC),
        glyph_active: Rgba::rgb(0xFF, 0xE2, 0xB8),
        shadow: Rgba::new(0x28, 0x14, 0x0A, 90),
        accent: Rgba::rgb(0xE8, 0x7A, 0x4A),
        glow: Rgba::new(0xFF, 0xB2, 0x7A, 150),
    },
    Builtin {
        id: "zouwu-dark",
        name: "驺吾 · 夜行",
        body: Rgba::rgb(0x5B, 0x6B, 0x8C),
        body_shade: Rgba::rgb(0x41, 0x4E, 0x69),
        mane: Rgba::rgb(0x9A, 0xAE, 0xD6),
        eye: Rgba::rgb(0x0E, 0x11, 0x18),
        muzzle: Rgba::rgb(0xD9, 0xE2, 0xF2),
        bubble_bg: Rgba::rgb(0x1B, 0x1F, 0x28),
        bubble_edge: Rgba::new(0x9A, 0xAE, 0xD6, 120),
        bubble_bg_active: Rgba::rgb(0x26, 0x2C, 0x39),
        bubble_edge_active: Rgba::rgb(0x9A, 0xAE, 0xD6),
        glyph: Rgba::rgb(0xC8, 0xD4, 0xE8),
        glyph_active: Rgba::rgb(0xEA, 0xF1, 0xFF),
        shadow: Rgba::new(0x05, 0x07, 0x0C, 100),
        accent: Rgba::rgb(0x7C, 0x9C, 0xE0),
        glow: Rgba::new(0x8F, 0xB0, 0xF0, 130),
    },
];

/// 外部 `theme.toml` 的形状：除 `id` 外全部可选（缺什么就用 `base` 的）。
#[derive(Debug, Deserialize)]
struct ThemeFile {
    #[serde(default)]
    schema_version: u32,
    id: String,
    #[serde(default)]
    name: Option<String>,
    /// 覆盖哪一套内置主题，默认 `zouwu-warm`。
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    palette: PaletteFile,
}

#[derive(Debug, Default, Deserialize)]
struct PaletteFile {
    body: Option<String>,
    body_shade: Option<String>,
    mane: Option<String>,
    eye: Option<String>,
    muzzle: Option<String>,
    bubble_bg: Option<String>,
    bubble_edge: Option<String>,
    bubble_bg_active: Option<String>,
    bubble_edge_active: Option<String>,
    glyph: Option<String>,
    glyph_active: Option<String>,
    shadow: Option<String>,
    accent: Option<String>,
    glow: Option<String>,
}

/// 主题文件的 schema 版本（`theme.toml` 里可以省略，省略按 1 处理）。
pub const THEME_SCHEMA_VERSION: u32 = 1;

impl Theme {
    /// 兜底主题：任何"主题加载失败"都退到它，界面绝不会变成漏色。
    pub fn fallback() -> Self {
        Self::builtin("zouwu-warm").expect("zouwu-warm is always built in")
    }

    pub fn builtin(id: &str) -> Option<Self> {
        BUILTINS.iter().find(|b| b.id == id).map(Builtin::to_theme)
    }

    /// 内置主题列表 `(id, 显示名)`，给设置界面/托盘菜单用。
    pub fn builtin_ids() -> Vec<(&'static str, &'static str)> {
        BUILTINS.iter().map(|b| (b.id, b.name)).collect()
    }

    pub fn is_builtin(id: &str) -> bool {
        BUILTINS.iter().any(|b| b.id == id)
    }

    /// 解析一个外部主题（增量覆盖 `base`）。
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: ThemeFile = toml::from_str(text).map_err(|err| format!("theme.toml: {err}"))?;
        // 版本比本程序新 → 明确拒绝。静默忽略未知字段会让用户以为"我写的没生效"。
        if file.schema_version > THEME_SCHEMA_VERSION {
            return Err(format!(
                "theme needs schema_version {}, this build supports {} — please update zouwu",
                file.schema_version, THEME_SCHEMA_VERSION
            ));
        }
        let base_id = file.base.as_deref().unwrap_or("zouwu-warm");
        let mut theme = Self::builtin(base_id)
            .ok_or_else(|| format!("theme base `{base_id}` is not built in"))?;
        if file.id.trim().is_empty() {
            return Err("theme id must not be empty".into());
        }
        theme.id = file.id;
        theme.name = file.name.unwrap_or_else(|| theme.id.clone());
        theme.apply(&file.palette)?;
        Ok(theme)
    }

    fn apply(&mut self, p: &PaletteFile) -> Result<(), String> {
        // 逐个 try：任何一处色值写错都带上字段名报错，而不是"整个主题静默失效"。
        macro_rules! apply {
            ($($field:ident),* $(,)?) => {$(
                if let Some(text) = &p.$field {
                    self.$field = Rgba::parse(text).map_err(|e| format!("palette.{}: {e}", stringify!($field)))?;
                }
            )*};
        }
        apply!(
            body,
            body_shade,
            mane,
            eye,
            muzzle,
            bubble_bg,
            bubble_edge,
            bubble_bg_active,
            bubble_edge_active,
            glyph,
            glyph_active,
            shadow,
            accent,
            glow,
        );
        Ok(())
    }

    /// 兼容旧调用点：未知名字退到兜底主题。
    pub fn by_name(name: &str) -> Self {
        Self::builtin(name).unwrap_or_else(Self::fallback)
    }

    /// 导出为 `theme.toml`（给"复制一份改改"用，字段全量展开）。
    pub fn to_toml(&self) -> String {
        format!(
            r#"# zouwu theme
schema_version = 1
id = "{id}"
name = "{name}"
base = "{base}"

[palette]
body = "{body}"
body_shade = "{body_shade}"
mane = "{mane}"
eye = "{eye}"
muzzle = "{muzzle}"
bubble_bg = "{bubble_bg}"
bubble_edge = "{bubble_edge}"
bubble_bg_active = "{bubble_bg_active}"
bubble_edge_active = "{bubble_edge_active}"
glyph = "{glyph}"
glyph_active = "{glyph_active}"
shadow = "{shadow}"
accent = "{accent}"
glow = "{glow}"
"#,
            id = self.id,
            name = self.name,
            base = if Self::is_builtin(&self.id) {
                self.id.as_str()
            } else {
                "zouwu-warm"
            },
            body = self.body.to_hex(),
            body_shade = self.body_shade.to_hex(),
            mane = self.mane.to_hex(),
            eye = self.eye.to_hex(),
            muzzle = self.muzzle.to_hex(),
            bubble_bg = self.bubble_bg.to_hex(),
            bubble_edge = self.bubble_edge.to_hex(),
            bubble_bg_active = self.bubble_bg_active.to_hex(),
            bubble_edge_active = self.bubble_edge_active.to_hex(),
            glyph = self.glyph.to_hex(),
            glyph_active = self.glyph_active.to_hex(),
            shadow = self.shadow.to_hex(),
            accent = self.accent.to_hex(),
            glow = self.glow.to_hex(),
        )
    }
}

impl Builtin {
    fn to_theme(&self) -> Theme {
        Theme {
            id: self.id.to_string(),
            name: self.name.to_string(),
            body: self.body,
            body_shade: self.body_shade,
            mane: self.mane,
            eye: self.eye,
            muzzle: self.muzzle,
            bubble_bg: self.bubble_bg,
            bubble_edge: self.bubble_edge,
            bubble_bg_active: self.bubble_bg_active,
            bubble_edge_active: self.bubble_edge_active,
            glyph: self.glyph,
            glyph_active: self.glyph_active,
            shadow: self.shadow,
            accent: self.accent,
            glow: self.glow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_scaling() {
        let c = Rgba::rgb(10, 20, 30).with_alpha(0.5);
        assert_eq!(c.a, 128);
    }

    #[test]
    fn mix_endpoints() {
        let a = Rgba::rgb(0, 0, 0);
        let b = Rgba::rgb(255, 255, 255);
        assert_eq!(a.mix(b, 0.0), a);
        assert_eq!(a.mix(b, 1.0), b);
    }

    #[test]
    fn parses_hex_with_and_without_alpha() {
        assert_eq!(Rgba::parse("#E87A4A"), Ok(Rgba::rgb(0xE8, 0x7A, 0x4A)));
        assert_eq!(
            Rgba::parse("e87a4a80"),
            Ok(Rgba::new(0xE8, 0x7A, 0x4A, 0x80))
        );
        assert!(Rgba::parse("#E87A").is_err());
        assert!(Rgba::parse("#GG0000").is_err());
    }

    #[test]
    fn hex_roundtrip() {
        for c in [Rgba::rgb(1, 2, 3), Rgba::new(9, 8, 7, 6)] {
            assert_eq!(Rgba::parse(&c.to_hex()), Ok(c));
        }
    }

    #[test]
    fn builtin_lookup() {
        assert_eq!(Theme::fallback().id, "zouwu-warm");
        assert_eq!(Theme::by_name("zouwu-dark").id, "zouwu-dark");
        // 未知名字不炸，退到兜底
        assert_eq!(Theme::by_name("nope").id, "zouwu-warm");
        assert_eq!(Theme::builtin_ids().len(), 2);
    }

    #[test]
    fn toml_overrides_only_what_it_mentions() {
        let warm = Theme::fallback();
        let theme = Theme::from_toml(
            r##"
id = "my-theme"
name = "我的主题"
[palette]
body = "#00FF00"
eye = "#000000"
"##,
        )
        .expect("valid theme");
        assert_eq!(theme.id, "my-theme");
        assert_eq!(theme.name, "我的主题");
        assert_eq!(theme.body, Rgba::rgb(0, 255, 0));
        assert_eq!(theme.eye, Rgba::rgb(0, 0, 0));
        // 没提到的令牌沿用 base
        assert_eq!(theme.mane, warm.mane);
        assert_eq!(theme.bubble_bg, warm.bubble_bg);
    }

    #[test]
    fn toml_base_can_be_dark() {
        let dark = Theme::by_name("zouwu-dark");
        let theme = Theme::from_toml(
            r##"
id = "dark-green"
base = "zouwu-dark"
[palette]
body = "#224422"
"##,
        )
        .expect("valid theme");
        assert_eq!(theme.mane, dark.mane);
        assert_eq!(theme.body, Rgba::rgb(0x22, 0x44, 0x22));
        // 名字没给就用 id
        assert_eq!(theme.name, "dark-green");
    }

    #[test]
    fn bad_color_names_the_field() {
        let err = Theme::from_toml(
            r##"
id = "broken"
[palette]
mane = "not-a-color"
"##,
        )
        .expect_err("must fail");
        assert!(err.contains("palette.mane"), "err was: {err}");
    }

    #[test]
    fn unknown_base_is_an_error() {
        assert!(Theme::from_toml("id = \"x\"\nbase = \"ghost\"\n").is_err());
    }

    #[test]
    fn export_then_reimport_keeps_colors() {
        let theme = Theme::by_name("zouwu-dark");
        let text = theme.to_toml();
        let back = Theme::from_toml(&text).expect("export must re-import");
        assert_eq!(back.body, theme.body);
        assert_eq!(back.glow, theme.glow);
        assert_eq!(back.id, theme.id);
    }
}
