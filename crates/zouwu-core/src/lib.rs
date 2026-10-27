//! zouwu-core —— 领域层。
//!
//! 规则：**不依赖任何 UI / 平台代码**，纯逻辑可单测。
//! 平台相关能力一律通过 `zouwu-platform` 的 trait 注入。

pub mod action;
pub mod anim;
pub mod config;
pub mod geom;
pub mod hotkey;
pub mod i18n;
pub mod ring;
pub mod theme;

pub use action::{default_ring, ActionItem, Glyph};
pub use anim::Spring;
pub use config::Config;
pub use geom::{Point, Rect};
pub use hotkey::Hotkey;
pub use i18n::I18n;
pub use ring::{Bubble, RingConfig};
pub use theme::Theme;

/// 一个便于复用的逻辑像素常量：60fps 帧间隔。
pub const FRAME_60FPS: f32 = 1.0 / 60.0;
