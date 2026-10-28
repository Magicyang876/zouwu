//! 整块虚拟桌面的截图（GDI，不依赖第三方 crate）。
//!
//! 只做一件事：把"所有显示器拼成的那个大矩形"抓成一块 RGBA。
//!
//! ## 为什么截虚拟桌面而不是逐屏截
//!
//! 逐屏要先枚举显示器、为每台建 DC、再按各自的原点拼回去；而扫码只关心
//! 像素，多抓的那点显示器之间的空隙（黑）完全无害。少一层拼接，就少一类
//! "偏移了一个屏幕"的 bug。
//!
//! ## 为什么用 `SM_XVIRTUALSCREEN` 而不是 `GetSystemMetrics(0)`
//!
//! 多屏可以是负坐标（主屏左边还有一块）。取 `(x, y, w, h)` 四个值才能把
//! 整个桌面框住；只取宽高会漏掉左下/左上的那部分屏幕。
//!
//! ## DPI
//!
//! 进程是 per-monitor DPI aware（winit 设的），所以这里的 `GetDC(NULL)`
//! 拿到的是**物理像素**坐标系，和 `outer_position()` 同一口径。
//! 不需要再乘缩放——乘了反而会错。

#![cfg(target_os = "windows")]

use std::ffi::c_void;

type Hdc = *mut c_void;
type Hbitmap = *mut c_void;
type Hgdiobj = *mut c_void;
type Bool = i32;

const SM_XVIRTUALSCREEN: i32 = 76;
const SM_YVIRTUALSCREEN: i32 = 77;
const SM_CXVIRTUALSCREEN: i32 = 78;
const SM_CYVIRTUALSCREEN: i32 = 79;
const SRCCOPY: u32 = 0x00CC_0020;
const DIB_RGB_COLORS: u32 = 0;
const BI_RGB: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    /// 负数 = 自上而下存行。DIB 默认是自下而上，直接用会把整张图上下颠倒，
    /// 而颠倒后的二维码检测不到（定位图案不在三个角上），却不会报错——
    /// 所以这里一开始就把它翻正，而不是事后补救。
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels: i32,
    y_pels: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 3],
}

#[link(name = "user32")]
extern "system" {
    fn GetDC(hwnd: *mut c_void) -> Hdc;
    fn ReleaseDC(hwnd: *mut c_void, hdc: Hdc) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    fn CreateCompatibleBitmap(hdc: Hdc, width: i32, height: i32) -> Hbitmap;
    fn SelectObject(hdc: Hdc, obj: Hgdiobj) -> Hgdiobj;
    fn BitBlt(
        dst: Hdc,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: Hdc,
        sx: i32,
        sy: i32,
        rop: u32,
    ) -> Bool;
    fn GetDIBits(
        hdc: Hdc,
        bitmap: Hbitmap,
        start: u32,
        lines: u32,
        bits: *mut c_void,
        info: *mut BitmapInfo,
        usage: u32,
    ) -> i32;
    fn DeleteObject(obj: Hgdiobj) -> Bool;
    fn DeleteDC(hdc: Hdc) -> Bool;
}

/// 一张整桌面截图。
pub struct Capture {
    /// 截图左上角在桌面坐标系里的位置（多屏时可能是负数）。
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    /// RGBA8，行优先。
    pub rgba: Vec<u8>,
}

impl Capture {
    /// 把截图坐标换算成桌面坐标。
    pub fn to_desktop(&self, px: u32, py: u32) -> (i32, i32) {
        (self.x + px as i32, self.y + py as i32)
    }
}

/// 抓整个虚拟桌面。
///
/// 失败时返回人话（"截不到屏幕"比 `Err(())` 有用得多，这条错误会直接显示给用户）。
pub fn capture_virtual_screen() -> Result<Capture, String> {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if width <= 0 || height <= 0 {
            return Err("没有可截取的桌面（分辨率读出来是 0）".to_string());
        }

        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("拿不到屏幕 DC".to_string());
        }
        let mem = CreateCompatibleDC(screen);
        if mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("建不了内存 DC".to_string());
        }
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        if bitmap.is_null() {
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("建不了位图".to_string());
        }

        let old = SelectObject(mem, bitmap);
        // 源 DC 就是整个屏幕，源坐标写 (x, y) 才能从负坐标的屏幕上取到内容。
        let copied = BitBlt(mem, 0, 0, width, height, screen, x, y, SRCCOPY);

        let mut rgba = Vec::new();
        if copied != 0 {
            let mut info: BitmapInfo = std::mem::zeroed();
            info.header.size = std::mem::size_of::<BitmapInfoHeader>() as u32;
            info.header.width = width;
            info.header.height = -height; // 见 BitmapInfoHeader::height 的注释
            info.header.planes = 1;
            info.header.bit_count = 32;
            info.header.compression = BI_RGB;

            let bytes = (width as usize) * (height as usize) * 4;
            let mut bgra = vec![0u8; bytes];
            let lines = GetDIBits(
                mem,
                bitmap,
                0,
                height as u32,
                bgra.as_mut_ptr() as *mut c_void,
                &mut info,
                DIB_RGB_COLORS,
            );
            if lines as i32 == height {
                // 屏幕像素没有 alpha（XP 之后就没人给它填 1.0 了），
                // 这里统一补成不透明，否则后续做灰度时会被当成全透明。
                rgba = bgra;
                for px in rgba.chunks_exact_mut(4) {
                    px.swap(0, 2);
                    px[3] = 255;
                }
            }
        }

        SelectObject(mem, old);
        DeleteObject(bitmap);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);

        if rgba.is_empty() {
            return Err("BitBlt 没能把屏幕内容拷进位图".to_string());
        }

        Ok(Capture {
            x,
            y,
            width: width as usize,
            height: height as usize,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真截一张：CI 上没有桌面，所以只在失败时要求"给出人话"而不是 panic。
    #[test]
    fn capture_reports_a_usable_size_or_a_readable_error() {
        match capture_virtual_screen() {
            Ok(shot) => {
                assert!(shot.width > 0 && shot.height > 0);
                assert_eq!(shot.rgba.len(), shot.width * shot.height * 4);
                // 全黑是允许的（锁屏/远程会话），但"全透明"说明拿错了通道。
                assert!(
                    shot.rgba.chunks_exact(4).all(|px| px[3] == 255),
                    "every pixel must be opaque"
                );
            }
            Err(err) => assert!(!err.is_empty(), "error text must not be empty"),
        }
    }
}
