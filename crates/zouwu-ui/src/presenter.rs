//! 呈现层抽象。
//!
//! 默认实现是 [`LayeredPresenter`]：CPU 光栅（[`crate::paint`]）出的一帧像素，
//! 通过 Windows 分层窗口（`UpdateLayeredWindow`）直接贴到屏幕上，逐像素 alpha。
//! 这条路径零 GPU 依赖、空闲零开销，特别适合这种小尺寸桌面挂件。
//!
//! `gpu` feature 下另有实验性的 wgpu 后端（见 [`crate::presenter_gpu`]）。

use tiny_skia::Pixmap;

/// 呈现一帧像素的抽象。`winit`、动画循环都只认这个 trait。
pub trait Presenter {
    /// 窗口物理尺寸变化时调用。
    fn resize(&mut self, width: u32, height: u32);

    /// 提交一帧（像素必须是预乘 alpha 的 RGBA8，尺寸等于最近一次 `resize`）。
    fn present(&mut self, pixmap: &Pixmap) -> Result<(), String>;
}

#[cfg(target_os = "windows")]
mod windows_layer {
    use super::Presenter;
    use tiny_skia::Pixmap;
    use zouwu_platform::layer::LayeredTarget;

    /// Windows 分层窗口后端。
    pub struct LayeredPresenter {
        target: LayeredTarget,
        frames: u32,
    }

    impl LayeredPresenter {
        /// `hwnd` 由 winit 的 raw window handle 提供。
        pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, String> {
            Ok(Self {
                target: LayeredTarget::new(hwnd, width, height)?,
                frames: 0,
            })
        }
    }

    impl Presenter for LayeredPresenter {
        fn resize(&mut self, width: u32, height: u32) {
            if let Err(err) = self.target.resize(width, height) {
                log::error!("layered resize failed: {err}");
            }
        }

        fn present(&mut self, pixmap: &Pixmap) -> Result<(), String> {
            if self.frames < 3 || self.frames % 120 == 0 {
                let data = pixmap.data();
                let mut max_a = 0u8;
                let mut opaque = 0usize;
                for px in data.chunks_exact(4) {
                    max_a = max_a.max(px[3]);
                    if px[3] > 8 {
                        opaque += 1;
                    }
                }
                log::debug!(
                    "present frame {}: {}x{} max_alpha={} visible_px={}",
                    self.frames,
                    pixmap.width(),
                    pixmap.height(),
                    max_a,
                    opaque
                );
            }
            self.frames += 1;
            self.target.present(pixmap.data())
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows_layer::LayeredPresenter;

/// 取出 winit 窗口的原生句柄（Windows 下是 HWND）。
#[cfg(target_os = "windows")]
fn win32_hwnd(window: &winit::window::Window) -> Result<isize, String> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window
        .window_handle()
        .map_err(|e| format!("window_handle: {e}"))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Ok(h.hwnd.get()),
        other => Err(format!("unsupported window handle: {other:?}")),
    }
}

/// 按平台挑选默认后端。
pub fn create(
    window: &winit::window::Window,
    width: u32,
    height: u32,
) -> Result<Box<dyn Presenter>, String> {
    #[cfg(target_os = "windows")]
    {
        let hwnd = win32_hwnd(window)?;
        Ok(Box::new(LayeredPresenter::new(hwnd, width, height)?))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, width, height);
        Err("M0 只实现了 Windows 呈现后端（见 docs/decisions.md ADR-001）".to_string())
    }
}
