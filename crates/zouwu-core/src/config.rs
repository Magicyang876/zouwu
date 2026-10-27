//! 配置模型（纯数据 + 序列化，不含 IO）。
//!
//! IO 由 `zouwu-platform` 负责，保证核心层可在任意环境（含测试）下使用。

use crate::ring::{RingConfig, DEG};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    /// 放在最前面：这是用户最常手改的一段（语言、热键、自启）。
    pub general: General,
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
            interaction: Interaction::default(),
            ring: RingCfg::default(),
            buddy: BuddyCfg::default(),
            window: WindowCfg::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Interaction {
    /// 悬停多久后展开（毫秒）。0 = 立即展开。
    pub hover_delay_ms: u64,
    /// 指针离开环带后多久收起（毫秒）。
    pub collapse_delay_ms: u64,
    /// 点击小兽是否立即展开（绕过延时）。
    pub expand_on_click: bool,
    /// 指针在"小兽半径 × 该系数"以内视为仍在身上，不收起。
    pub collapse_radius_factor: f32,
    /// 命中容差（气泡半径的倍数）。
    pub hit_tolerance: f32,
}

impl Default for Interaction {
    fn default() -> Self {
        Self {
            hover_delay_ms: 150,
            collapse_delay_ms: 250,
            expand_on_click: true,
            collapse_radius_factor: 1.1,
            hit_tolerance: 1.15,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// 界面语言：`"auto"` = 跟随系统，或 `"zh-CN"` / `"en-US"`。
    pub language: String,
    /// 全局热键，形如 `"Alt+Space"`；空串 = 不注册。
    pub hotkey: String,
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
            start_on_boot: false,
            show_on_start: true,
            bring_to_front: true,
        }
    }
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// 配置迁移入口（M0 只有 v1，占位保持结构稳定）。
    pub fn migrated(mut self) -> Self {
        if self.schema_version < SCHEMA_VERSION {
            self.schema_version = SCHEMA_VERSION;
        }
        self
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
    fn ring_cfg_converts_degrees() {
        let rc = RingCfg::default().to_ring_config();
        assert!((rc.orbit_start + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }
}
