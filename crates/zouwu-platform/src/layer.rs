//! Windows 分层窗口（WS_EX_LAYERED + UpdateLayeredWindow）直通绘制。
//!
//! 为什么不用 GPU：M0 实测发现 wgpu 在这个环境（windows-gnu 工具链）启动即崩，
//! 而我们要画的东西只有 340×340 的 CPU 光栅结果。分层窗口方案：
//!
//! * 逐像素 alpha，真正的"镂空"窗口（不是抠一个颜色当透明）；
//! * alpha = 0 的像素天然鼠标穿透，桌面图标在气泡外的区域照常可点；
//! * 一帧就是一次 `UpdateLayeredWindow`，小面积下 < 0.5ms，空闲 0 开销；
//! * 零 GPU 依赖，不需要 shader，跨工具链（gnu/msvc）都不挑链接器。
//!
//! 这里只声明用到的那几个 Win32 函数，避免引入整个 `windows` crate。

#![cfg(target_os = "windows")]

use std::ffi::c_void;

type Hwnd = *mut c_void;
type Hdc = *mut c_void;
type Hbitmap = *mut c_void;
type Hgdiobj = *mut c_void;
type Bool = i32;
type Dword = u32;

const WS_EX_LAYERED: isize = 0x0008_0000;
const GWL_EXSTYLE: i32 = -20;
const DIB_RGB_COLORS: u32 = 0;
const BI_RGB: u32 = 0;
const ULW_ALPHA: u32 = 0x02;
const AC_SRC_OVER: u8 = 0x00;
const AC_SRC_ALPHA: u8 = 0x01;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Size {
    cx: i32,
    cy: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BlendFunction {
    blend_op: u8,
    blend_flags: u8,
    source_constant_alpha: u8,
    alpha_format: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RgbQuad {
    blue: u8,
    green: u8,
    red: u8,
    reserved: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [RgbQuad; 1],
}

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

#[link(name = "user32")]
extern "system" {
    fn GetDC(hwnd: Hwnd) -> Hdc;
    fn ReleaseDC(hwnd: Hwnd, hdc: Hdc) -> i32;
    fn GetCursorPos(point: *mut Point) -> Bool;
    fn GetAsyncKeyState(vkey: i32) -> i16;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> Bool;
    fn IsWindowVisible(hwnd: Hwnd) -> Bool;
    fn SetForegroundWindow(hwnd: Hwnd) -> Bool;
    fn SetWindowPos(
        hwnd: Hwnd,
        insert_after: Hwnd,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> Bool;
    fn UpdateLayeredWindow(
        hwnd: Hwnd,
        hdc_dst: Hdc,
        dst_point: *const Point,
        size: *const Size,
        hdc_src: Hdc,
        src_point: *const Point,
        color_key: Dword,
        blend: *const BlendFunction,
        flags: Dword,
    ) -> Bool;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    fn DeleteDC(hdc: Hdc) -> Bool;
    fn CreateDIBSection(
        hdc: Hdc,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut c_void,
        section: *mut c_void,
        offset: u32,
    ) -> Hbitmap;
    fn SelectObject(hdc: Hdc, obj: Hgdiobj) -> Hgdiobj;
    fn DeleteObject(obj: Hgdiobj) -> Bool;
}

/// 一块可写的 32 位 DIB，承载一帧预乘 alpha 的 BGRA 像素。
struct Surface {
    bitmap: Hbitmap,
    screen_dc: Hdc,
    memory_dc: Hdc,
    width: u32,
    height: u32,
    pixels: *mut u8,
}

impl Surface {
    fn new(width: u32, height: u32) -> Result<Self, String> {
        let screen_dc = unsafe { GetDC(std::ptr::null_mut()) };
        if screen_dc.is_null() {
            return Err("GetDC failed".into());
        }
        let memory_dc = unsafe { CreateCompatibleDC(screen_dc) };
        if memory_dc.is_null() {
            unsafe { ReleaseDC(std::ptr::null_mut(), screen_dc) };
            return Err("CreateCompatibleDC failed".into());
        }

        let mut info = BitmapInfo::default();
        info.header.size = std::mem::size_of::<BitmapInfoHeader>() as u32;
        info.header.width = width as i32;
        // 负高度 = 自上而下的像素行，和我们的 pixmap 方向一致
        info.header.height = -(height as i32);
        info.header.planes = 1;
        info.header.bit_count = 32;
        info.header.compression = BI_RGB;

        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(
                screen_dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        if bitmap.is_null() || bits.is_null() {
            unsafe {
                DeleteDC(memory_dc);
                ReleaseDC(std::ptr::null_mut(), screen_dc);
            }
            return Err("CreateDIBSection failed".into());
        }
        unsafe { SelectObject(memory_dc, bitmap as Hgdiobj) };

        Ok(Self {
            bitmap,
            screen_dc,
            memory_dc,
            width,
            height,
            pixels: bits as *mut u8,
        })
    }

    /// 把预乘 RGBA 拷成 DIB 需要的预乘 BGRA。
    fn upload(&mut self, rgba: &[u8]) {
        let count = (self.width as usize) * (self.height as usize);
        if rgba.len() < count * 4 {
            return;
        }
        unsafe {
            let dst = std::slice::from_raw_parts_mut(self.pixels, count * 4);
            for i in 0..count {
                let s = i * 4;
                dst[s] = rgba[s + 2]; // B
                dst[s + 1] = rgba[s + 1]; // G
                dst[s + 2] = rgba[s]; // R
                dst[s + 3] = rgba[s + 3]; // A
            }
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.bitmap as Hgdiobj);
            DeleteDC(self.memory_dc);
            ReleaseDC(std::ptr::null_mut(), self.screen_dc);
        }
    }
}

/// 一个绑定到 winit 窗口的分层绘制目标。
// ---- 窗口显隐与置前 ----
// 托盘模式的两个关键 API：ShowWindow 负责"从托盘回来/躲进托盘"，
// SetWindowPos 负责"回到最前但不抢焦点"（SWP_NOACTIVATE）。
const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;
const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
const WS_EX_NOACTIVATE: isize = 0x0800_0000;
const HWND_TOPMOST: Hwnd = -1isize as Hwnd;
const HWND_NOTOPMOST: Hwnd = -2isize as Hwnd;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOACTIVATE: u32 = 0x0010;

/// 把窗口从任务栏/Alt-Tab 列表里摘掉，并声明"不抢焦点"。
///
/// 小工具的行为准则：它出现在桌面上，但**不应该打断你正在做的事**。
/// `WS_EX_TOOLWINDOW` 去掉任务栏与 Alt-Tab 项，`WS_EX_NOACTIVATE` 让点击它
/// 不抢走当前窗口的激活状态与焦点。
pub fn hide_from_taskbar(hwnd: isize) {
    let hwnd = hwnd as Hwnd;
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let wanted = ex | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if wanted != ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
        }
    }
}

/// 显示 / 隐藏（"缩进托盘"就是隐藏窗口，不是退出进程）。
pub fn set_visible(hwnd: isize, visible: bool) {
    let hwnd = hwnd as Hwnd;
    unsafe {
        ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
}

pub fn is_visible(hwnd: isize) -> bool {
    unsafe { IsWindowVisible(hwnd as Hwnd) != 0 }
}

/// 为深色面板设置暗色标题栏，并立即刷新非客户区。
///
/// `dwmapi.dll` 是**运行时加载**的：`x86_64-w64-mingw32` 工具链没有 `libdwmapi`
/// 导入库（链接期 `ld: cannot find -ldwmapi`），而这条调用纯属观感，
/// 用 `LoadLibrary` + `GetProcAddress` 拿一下最省事，也顺带兼容没有 DWM 的老系统
/// （Win7 上直接拿不到符号，跳过即可）。
///
/// `DWMWA_USE_IMMERSIVE_DARK_MODE` 在 Win10 1809~1903 上是 19、1909 之后是 20，
/// 两个都试一次，失败不报错。
pub fn set_dark_title_bar(hwnd: isize, dark: bool) -> bool {
    type SetAttr = unsafe extern "system" fn(Hwnd, Dword, *const c_void, Dword) -> i32;

    static PROC: std::sync::OnceLock<Option<SetAttr>> = std::sync::OnceLock::new();
    let proc = *PROC.get_or_init(|| unsafe {
        let lib = LoadLibraryA(c"dwmapi.dll".as_ptr() as *const u8);
        if lib.is_null() {
            return None;
        }
        let addr = GetProcAddress(lib, c"DwmSetWindowAttribute".as_ptr() as *const u8);
        if addr.is_null() {
            return None;
        }
        Some(std::mem::transmute::<*mut c_void, SetAttr>(addr))
    });

    let Some(set_attr) = proc else { return false };
    let value: i32 = i32::from(dark);
    unsafe {
        for attribute in [20u32, 19u32] {
            if set_attr(
                hwnd as Hwnd,
                attribute,
                &value as *const i32 as *const c_void,
                std::mem::size_of::<i32>() as u32,
            ) >= 0
            {
                // DWM 属性变更不会总是立即重绘标题栏。
                const SWP_NOZORDER: u32 = 0x0004;
                const SWP_FRAMECHANGED: u32 = 0x0020;
                SetWindowPos(
                    hwnd as Hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
                return true;
            }
        }
    }
    false
}

pub fn bring_to_front(hwnd: isize) {
    let hwnd = hwnd as Hwnd;
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
        );
        // 有些 shell 组合（比如全屏游戏退出后）需要显式再问一次前台。
        SetForegroundWindow(hwnd);
    }
}

/// 置顶 / 取消置顶。
///
/// 不能用 winit 的 `set_window_level`：它会把窗口重建一遍，重建后的窗口
/// **丢掉 `WS_EX_LAYERED`**，于是下一帧 `UpdateLayeredWindow` 就永久失败（实测
/// 1172 条 render failed，CPU 直接跑到 66%）。用 `SetWindowPos` 改 Z 序才是
/// 无创的，分层属性、位置、尺寸、激活状态全都不动。
pub fn set_always_on_top(hwnd: isize, on_top: bool) {
    let hwnd = hwnd as Hwnd;
    unsafe {
        SetWindowPos(
            hwnd,
            if on_top { HWND_TOPMOST } else { HWND_NOTOPMOST },
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
        );
    }
}

pub struct LayeredTarget {
    hwnd: Hwnd,
    surface: Surface,
}

impl LayeredTarget {
    /// `hwnd` 来自 winit 的 raw window handle。
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, String> {
        let hwnd = hwnd as Hwnd;
        // 让窗口走"分层"合成路径（逐像素 alpha 的前提），顺手摘掉任务栏项。
        hide_from_taskbar(hwnd as isize);
        Ok(Self {
            hwnd,
            surface: Surface::new(width.max(1), height.max(1))?,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.surface.width, self.surface.height)
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        let (w, h) = (width.max(1), height.max(1));
        if (w, h) == self.size() {
            return Ok(());
        }
        self.surface = Surface::new(w, h)?;
        Ok(())
    }

    /// 提交一帧预乘 RGBA 像素（尺寸需与 `resize` 后一致）。
    pub fn present(&mut self, rgba: &[u8]) -> Result<(), String> {
        self.surface.upload(rgba);
        let size = Size {
            cx: self.surface.width as i32,
            cy: self.surface.height as i32,
        };
        let src = Point { x: 0, y: 0 };
        let blend = BlendFunction {
            blend_op: AC_SRC_OVER,
            blend_flags: 0,
            source_constant_alpha: 255,
            alpha_format: AC_SRC_ALPHA,
        };
        let ok = unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                self.surface.screen_dc,
                std::ptr::null(), // NULL = 不改变窗口位置
                &size,
                self.surface.memory_dc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            )
        };
        if ok == 0 {
            return Err("UpdateLayeredWindow failed".into());
        }
        Ok(())
    }
}

/// 光标的**屏幕物理坐标**（多屏虚拟桌面坐标系，左上角为原点）。
///
/// 为什么需要它：分层窗口的鼠标命中测试由系统按 alpha 形状决定，在某些会话
/// （远程桌面断开、窗口未激活、命中被上层窗口截走）里我们收不到 `CursorMoved`。
/// 悬停展开是这小工具最核心的交互，不能依赖"窗口能收到鼠标消息"这个假设，
/// 所以 UI 层以固定频率（见 `zouwu-ui` 的 `CURSOR_POLL`）轮询本函数作为兜底。
pub fn cursor_position() -> Option<(i32, i32)> {
    let mut point = Point::default();
    let ok = unsafe { GetCursorPos(&mut point) };
    if ok == 0 {
        return None;
    }
    Some((point.x, point.y))
}

/// 主键（左键）当前是否按下。0x01 = VK_LBUTTON。
pub fn left_button_down() -> bool {
    unsafe { (GetAsyncKeyState(0x01) as u16 & 0x8000) != 0 }
}

/// 副键（右键）当前是否按下。0x02 = VK_RBUTTON。
pub fn right_button_down() -> bool {
    unsafe { (GetAsyncKeyState(0x02) as u16 & 0x8000) != 0 }
}
