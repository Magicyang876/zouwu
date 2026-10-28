//! 让标题栏和面板一个颜色。
//!
//! 只设 `DWMWA_USE_IMMERSIVE_DARK_MODE` 时，标题栏怎么画由**系统**主题决定：
//! 系统是浅色模式，DWM 画的仍是浅色边框——面板是暖棕色，边框却是蓝灰，
//! 看起来就是"主题没对上"。所以这里额外把 Win11 的标题栏/文字/边框三色
//! 直接设成当前主题的颜色（Win10 没有这三条属性，调用失败，无副作用）。
//!
//! 时机也是坑：winit 的窗口一建出来就是可见的，DWM 第一次绘制非客户区时
//! 就按默认配色画好了边框，之后再改属性不一定重画——表现成"要点一下最大化
//! 才变黑"。所以除了创建时调，还要在首次重绘（那时窗口一定上屏了）、
//! 尺寸变化、重新获得焦点时各补一次。

use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use zouwu_core::theme::{Rgba, Theme};

/// 标题栏底色 / 标题文字 / 窗口边框。DWM 要 `0x00BBGGRR`，不是 `0x00RRGGBB`。
fn colorref(c: Rgba) -> u32 {
    u32::from(c.r) | (u32::from(c.g) << 8) | (u32::from(c.b) << 16)
}

/// 窗口生命周期日志。
///
/// 卡死这类问题看不到屏幕就没法猜，所以把"建/隐/毁"三个时刻各打一行，
/// 并且都带 hwnd——同一类窗口在日志里能按句柄区分代次。
pub fn life(kind: &str, phase: &str, window: &Window) {
    let hwnd = window
        .window_handle()
        .ok()
        .and_then(|handle| match handle.as_raw() {
            RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
            _ => None,
        });
    log::info!("window {kind}: {phase} hwnd={hwnd:?}");
}

/// 把窗口标题栏刷成当前主题的颜色。幂等，失败只记一次日志。
pub fn apply(window: &Window, theme: &Theme) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = win32.hwnd.get();
    if !zouwu_platform::layer::set_dark_title_bar(hwnd, true) {
        log::warn!("frame: DWM refused the dark title bar");
    }
    zouwu_platform::layer::set_title_colors(
        hwnd,
        colorref(theme.bubble_bg),
        colorref(theme.glyph),
        colorref(theme.bubble_edge),
    );
}
