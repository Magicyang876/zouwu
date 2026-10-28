//! zouwu 的绘制与窗口层。
//!
//! * [`paint`]：纯 CPU 光栅化（tiny-skia），把场景画到 RGBA 像素缓冲；
//! * [`presenter`]：wgpu 把像素缓冲 blit 到窗口（预乘 alpha，支持逐像素透明）；
//! * [`app`]：winit 事件循环 + 状态机（托盘/悬停展开/动画驱动）。

pub mod app;
pub mod frame;
pub mod history;
pub mod paint;
pub mod presenter;
#[cfg(feature = "gpu")]
pub mod presenter_gpu;
pub mod report;
pub mod settings;
pub mod settings_window;
pub mod skin;
pub mod text;
pub mod thumb;
pub mod watchdog;
pub mod widget;

pub use app::run;
