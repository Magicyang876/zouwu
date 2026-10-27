//! 主题令牌。渲染层只消费这里的颜色/参数，不写死色值。

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
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: &'static str,
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

pub const ZOUWU_WARM: Theme = Theme {
    name: "zouwu-warm",
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
};

pub const ZOUWU_DARK: Theme = Theme {
    name: "zouwu-dark",
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
};

impl Theme {
    pub fn by_name(name: &str) -> Self {
        match name {
            "zouwu-dark" => ZOUWU_DARK,
            _ => ZOUWU_WARM,
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
}
