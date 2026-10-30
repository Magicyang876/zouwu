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
        Err("M0 只实现了 Windows 呈现后端".to_string())
    }
}

/// 普通（不透明、有边框）窗口的呈现后端：[`softbuffer`]。
///
/// 设置面板是一个**正经窗口**：系统给它客户区、给它键盘焦点、帮它画标题栏。
/// 这类窗口不需要逐像素 alpha，用分层窗口反而是错配；而 softbuffer 在 Windows
/// 上就是"一块 DIB + `BitBlt`"，零 GPU 依赖，和主界面的 CPU 光栅路线一致。
///
/// 为什么不用 `Presenter` 直接喂 `Arc<Window>`：`softbuffer` 要求
/// `HasWindowHandle`，而 winit 只为 `Window` 实现了它（不是 `Arc<Window>`）。
/// 这里用一个持有 `Arc` 的薄包装把两个 trait 转出去，生命周期就干净了。
mod soft {
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
    };
    use tiny_skia::Pixmap;
    use winit::window::Window;

    use super::Presenter;

    #[derive(Clone)]
    struct WinHandle(Arc<Window>);

    impl HasWindowHandle for WinHandle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            self.0.window_handle()
        }
    }

    impl HasDisplayHandle for WinHandle {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            self.0.display_handle()
        }
    }

    pub struct SoftPresenter {
        surface: softbuffer::Surface<WinHandle, WinHandle>,
        width: u32,
        height: u32,
        frames: u32,
    }

    impl SoftPresenter {
        pub fn new(window: Arc<Window>, width: u32, height: u32) -> Result<Self, String> {
            let handle = WinHandle(window);
            let context = softbuffer::Context::new(handle.clone())
                .map_err(|e| format!("softbuffer context: {e}"))?;
            let mut surface = softbuffer::Surface::new(&context, handle)
                .map_err(|e| format!("softbuffer surface: {e}"))?;
            let (w, h) = (width.max(1), height.max(1));
            surface
                .resize(
                    NonZeroU32::new(w).expect("non-zero"),
                    NonZeroU32::new(h).expect("non-zero"),
                )
                .map_err(|e| format!("softbuffer resize: {e}"))?;
            log::debug!("presenter: softbuffer {w}x{h}");
            Ok(Self {
                surface,
                width: w,
                height: h,
                frames: 0,
            })
        }
    }

    impl Presenter for SoftPresenter {
        fn resize(&mut self, width: u32, height: u32) {
            let (w, h) = (width.max(1), height.max(1));
            if (w, h) == (self.width, self.height) {
                return;
            }
            let (Some(w2), Some(h2)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else {
                return;
            };
            if let Err(err) = self.surface.resize(w2, h2) {
                log::error!("softbuffer resize failed: {err}");
                return;
            }
            self.width = w;
            self.height = h;
        }

        fn present(&mut self, pixmap: &Pixmap) -> Result<(), String> {
            let mut buffer = self
                .surface
                .buffer_mut()
                .map_err(|e| format!("softbuffer buffer: {e}"))?;
            // softbuffer 要的是 0x00RRGGBB。面板是不透明底，直接把颜色抬上去即可；
            // 若将来允许半透明窗口，这里得先按 alpha 与底色混合。
            let data = pixmap.data();
            if buffer.len() < data.len() / 4 {
                return Err(format!(
                    "buffer too small: {} px for {} px pixmap",
                    buffer.len(),
                    data.len() / 4
                ));
            }
            for (dst, px) in buffer.iter_mut().zip(data.chunks_exact(4)) {
                *dst = ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32;
            }
            buffer
                .present()
                .map_err(|e| format!("softbuffer present: {e}"))?;
            self.frames += 1;
            if self.frames <= 2 {
                log::debug!("presenter: soft present #{}", self.frames);
            }
            Ok(())
        }
    }
}

/// 给普通窗口创建呈现后端（设置面板用）。
pub fn create_soft(
    window: std::sync::Arc<winit::window::Window>,
    width: u32,
    height: u32,
) -> Result<Box<dyn Presenter>, String> {
    Ok(Box::new(soft::SoftPresenter::new(window, width, height)?))
}
