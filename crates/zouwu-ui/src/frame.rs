//! 暗色标题栏。
//!
//! 窗口创建后调一次**不够**：winit 的窗口一建出来就是可见的，DWM 在第一次绘制
//! 非客户区时就已经按浅色画好了边框，之后再改属性有时不重画——表现就是"要点一下
//! 最大化才变黑"。所以这里除了创建时调，还要在首次重绘时补一次（那时窗口一定
//! 已经上屏），以及重新获得焦点时补一次（Win10 切回前台会重刷一次边框）。

use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// 把窗口标题栏刷成暗色。幂等，失败只记一次日志。
pub fn apply_dark(window: &Window) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    if !zouwu_platform::layer::set_dark_title_bar(win32.hwnd.get(), true) {
        log::warn!("frame: DWM refused the dark title bar");
    }
}
