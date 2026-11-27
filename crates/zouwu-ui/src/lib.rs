//! zouwu 的绘制与窗口层。
//!
//! * [`paint`]：纯 CPU 光栅化（tiny-skia），把场景画到 RGBA 像素缓冲；
//! * [`presenter`]：wgpu 把像素缓冲 blit 到窗口（预乘 alpha，支持逐像素透明）；
//! * [`app`]：winit 事件循环 + 状态机（托盘/悬停展开/动画驱动）。

pub mod app;
pub mod frame;
pub mod history;
/// Linux/Wayland：小兽本体的 layer 表面宿主（其他平台没有这个模块，
/// 小兽本体就是普通的 winit 窗口）。
#[cfg(not(target_os = "windows"))]
pub mod host;
pub mod icon;
pub mod launcher;
pub mod linefield;
pub mod paint;
pub mod presenter;
#[cfg(feature = "gpu")]
pub mod presenter_gpu;
pub mod report;
pub mod settings;
pub mod settings_window;
pub mod skin;
pub mod skin_menu;
/// 「系统字体」小窗：改 Windows 自己的界面字体（见 ADR-066）。
pub mod sysfont_window;
pub mod text;
pub mod thumb;
pub mod watchdog;
pub mod widget;

pub use app::run;
