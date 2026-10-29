//! 护眼遮罩（蓝光过滤）。
//!
//! 做法和 f.lux / 护眼宝那类工具一样：在整个虚拟桌面上贴一层**暖色半透明
//! 窗口**，靠混合把蓝色压下去，而不是改显示器的伽马表。
//!
//! ## 为什么不用 `SetDeviceGammaRamp`
//!
//! 改伽马是全局硬件状态：
//!
//! * 进程崩了 / 被任务管理器杀掉，颜色不会自己恢复，用户只能重启。
//! * 多显示器各自的伽马曲线不同，要逐个设、逐个存原值。
//! * 有些驱动（远程桌面、部分集显）直接忽略，功能看起来"开了没用"。
//!
//! 遮罩最坏情况只是"关掉窗口"就消失，退出、崩溃、断电都不会留下痕迹。
//!
//! ## 为什么只有一个窗口盖住整个虚拟桌面
//!
//! 逐屏建窗口要先枚举显示器，还得处理各自不同的 DPI 缩放；而遮罩只涂一种
//! 纯色，跨越所有屏幕的一块大矩形效果完全一致，少一类"漏了副屏"的 bug。
//! 代价是显示器布局变化后要重新贴一次尺寸——`apply` 每次都重设几何，所以
//! 调用方在布局变化时再调一次即可。
//!
//! ## 为什么不挡操作
//!
//! 三件套：`WS_EX_TRANSPARENT`（命中测试穿透）、`WS_EX_NOACTIVATE`（不抢焦点）、
//! `WS_EX_TOOLWINDOW`（不进 Alt+Tab / 任务栏）。挂件自己的拖拽用的是
//! `GetCursorPos` 轮询，也不受影响。

#![cfg(target_os = "windows")]

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

type Hwnd = *mut c_void;
type Hinstance = *mut c_void;
type Hbrush = *mut c_void;
type Bool = i32;
type Lresult = isize;
type Wineventhook = *mut c_void;
type Wineventproc = unsafe extern "system" fn(Wineventhook, u32, Hwnd, i32, i32, u32, u32);
type Wndproc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> Lresult;

const WS_POPUP: u32 = 0x8000_0000;
const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_LAYERED: u32 = 0x0008_0000;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
/// 只按整体不透明度混合（不涉及逐像素 alpha）。
const LWA_ALPHA: u32 = 0x0000_0002;

const SM_XVIRTUALSCREEN: i32 = 76;
const SM_YVIRTUALSCREEN: i32 = 77;
const SM_CXVIRTUALSCREEN: i32 = 78;
const SM_CYVIRTUALSCREEN: i32 = 79;

const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;

const HWND_TOPMOST: Hwnd = -1isize as Hwnd;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;

/// 有窗口被显示 / 前台被切换。新冒出来的**置顶**窗口会插到遮罩上方，
/// 必须立刻把遮罩抬回去，不能等主循环的 3s 体检。
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_OBJECT_SHOW: u32 = 0x8002;
const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const OBJID_WINDOW: i32 = 0;

/// 暖色。`COLORREF` 是 `0x00BBGGRR`：R=255 / G=198 / B=110。
///
/// 偏琥珀而不是纯橙：纯橙（B≈0）在白底上会把正文染成粉色，琥珀在同样的
/// 不透明度下只是"白得暖一点"。
const TINT: u32 = 0x006E_C6FF;

const CLASS_NAME: &str = "zouwu-eye-care-overlay";

#[repr(C)]
struct WndClassW {
    style: u32,
    wnd_proc: Option<Wndproc>,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Hinstance,
    icon: *mut c_void,
    cursor: *mut c_void,
    background: Hbrush,
    menu_name: *const u16,
    class_name: *const u16,
}

extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Hinstance;
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: Hwnd,
        instance: Hinstance,
        param: *mut c_void,
    ) -> Hwnd;
    fn DestroyWindow(hwnd: Hwnd) -> Bool;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Lresult;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> Bool;
    fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> Bool;
    fn SetLayeredWindowAttributes(hwnd: Hwnd, key: u32, alpha: u8, flags: u32) -> Bool;
    fn GetSystemMetrics(index: i32) -> i32;
    fn CreateSolidBrush(color: u32) -> Hbrush;
    fn DeleteObject(obj: *mut c_void) -> Bool;
    fn SetWinEventHook(
        event_min: u32,
        event_max: u32,
        module: Hinstance,
        proc: Wineventproc,
        process: u32,
        thread: u32,
        flags: u32,
    ) -> Wineventhook;
    fn UnhookWinEvent(hook: Wineventhook) -> Bool;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 窗口过程：遮罩不需要处理任何消息。
///
/// 背景由注册时给类设置的画刷涂（`DefWindowProcW` 处理 `WM_ERASEBKGND`），
/// 所以这里只剩"别的都交回系统"。**不能**在这里 `PostQuitMessage`：
/// 这个窗口和挂件主窗共用一个消息循环，一旦投递退出消息整个程序就没了。
unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> Lresult {
    DefWindowProcW(hwnd, msg, w, l)
}

/// 注册窗口类（只做一次）。
///
/// 类画刷不走"退出时释放"这条路：类本身还留在系统里，
/// 提前释放会让残留的类指向野指针，而进程结束时它自然回收。
fn register_class() -> Result<(), String> {
    static CLASS: OnceLock<Result<(), String>> = OnceLock::new();
    CLASS
        .get_or_init(|| unsafe {
            let brush = CreateSolidBrush(TINT);
            if brush.is_null() {
                return Err("eye care: CreateSolidBrush failed".to_string());
            }
            let name = wide(CLASS_NAME);
            let class = WndClassW {
                style: 0,
                wnd_proc: Some(wnd_proc),
                cls_extra: 0,
                wnd_extra: 0,
                instance: GetModuleHandleW(std::ptr::null()),
                icon: std::ptr::null_mut(),
                cursor: std::ptr::null_mut(),
                background: brush,
                menu_name: std::ptr::null(),
                class_name: name.as_ptr(),
            };
            if RegisterClassW(&class) == 0 {
                DeleteObject(brush);
                return Err("eye care: RegisterClassW failed".to_string());
            }
            Ok(())
        })
        .clone()
}

/// 把遮罩摆到整个虚拟桌面上，并抬到最高层。
unsafe fn place(hwnd: Hwnd) {
    let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
    let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
    let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
    let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        x,
        y,
        w,
        h,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
}

struct State {
    /// 用 `isize` 存句柄：`*mut c_void` 不是 `Send`，进不了 `static`。
    hwnd: isize,
    alpha: u8,
    /// 两个精确的事件钩子（遮罩活跃时才挂）。同样用 `isize` 存。
    /// `SetWinEventHook` 的 min/max 是区间，一个区间只能盯一个事件，
    /// 盯宽了会把所有无障碍事件都灌到主线程来，白耗 CPU。
    hook_foreground: isize,
    hook_show: isize,
    /// 上次因事件抬升遮罩的时刻：tooltip/IME 会让 SHOW 事件成串来，
    /// 不能每个都来一次 `SetWindowPos`。防抖窗口比人眼可感知的
    /// “新窗口没变色”阈值（约 100ms）小得多，不会造成可感知延迟。
    last_raise: Option<std::time::Instant>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            hwnd: 0,
            alpha: 0,
            hook_foreground: 0,
            hook_show: 0,
            last_raise: None,
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    // 中毒也要继续用：这里只有一个句柄和一个数字，没有"被破坏的不变量"。
    state().lock().unwrap_or_else(|e| e.into_inner())
}

/// 窗口事件回调：有窗口显示或前台切换，就把遮罩重新抬到最上。
///
/// 钩子是 `WINEVENT_OUTOFCONTEXT`，系统把事件投到安装线程（主线程），
/// 由 winit 的事件循环泵出来，所以这里可以安全地拿锁、调 USER32。
unsafe extern "system" fn on_win_event(
    _hook: Wineventhook,
    _event: u32,
    hwnd: Hwnd,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // 只关心窗口本体（ tooltip、控件、滚动条也会发 SHOW 事件）。
    if id_object != OBJID_WINDOW || hwnd.is_null() {
        return;
    }
    let mut st = lock();
    if st.hwnd == 0 || st.hook_foreground == 0 || st.hwnd == hwnd as isize {
        return;
    }
    if let Some(at) = st.last_raise {
        if at.elapsed() < std::time::Duration::from_millis(30) {
            return;
        }
    }
    st.last_raise = Some(std::time::Instant::now());
    let overlay = st.hwnd as Hwnd;
    drop(st);
    place(overlay);
}

/// 挂上事件钩子（遮罩创建后调用；重复调用无副作用）。
fn install_hook(st: &mut State) {
    unsafe {
        let install = |event: u32| -> isize {
            let hook = SetWinEventHook(
                event,
                event,
                std::ptr::null_mut(),
                on_win_event,
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if hook.is_null() {
                0
            } else {
                hook as isize
            }
        };
        if st.hook_foreground == 0 {
            st.hook_foreground = install(EVENT_SYSTEM_FOREGROUND);
        }
        if st.hook_show == 0 {
            st.hook_show = install(EVENT_OBJECT_SHOW);
        }
        // 挂不上就退回“只靠 3s 体检重贴”：旧窗口仍被遮住，
        // 只是新置顶窗口的覆盖会慢一拍，不是功能失效。
    }
}

/// 摘掉事件钩子（遮罩收起/退出时调用）。
fn remove_hook(st: &mut State) {
    unsafe {
        if st.hook_foreground != 0 {
            UnhookWinEvent(st.hook_foreground as Wineventhook);
            st.hook_foreground = 0;
        }
        if st.hook_show != 0 {
            UnhookWinEvent(st.hook_show as Wineventhook);
            st.hook_show = 0;
        }
    }
    st.last_raise = None;
}

/// 应用护眼状态。`alpha` 是遮罩不透明度（`0..=255`，0 = 无效果）。
///
/// 幂等：重复调用只是重新摆位置、改不透明度。显示器布局变了再调一次即可。
/// 只在创建/更新窗口这条路上会失败（注册类、创建窗口），失败时遮罩不出现，
/// 调用方按"功能不可用"处理即可，不该让挂件启动失败。
pub fn apply(enabled: bool, alpha: u8) -> Result<(), String> {
    let want = enabled && alpha > 0;
    let mut st = lock();

    if !want {
        if st.hwnd != 0 {
            remove_hook(&mut st);
            unsafe {
                ShowWindow(st.hwnd as Hwnd, SW_HIDE);
            }
            st.hwnd = 0;
        }
        st.alpha = alpha;
        return Ok(());
    }

    unsafe {
        if st.hwnd == 0 {
            register_class()?;
            let name = wide(CLASS_NAME);
            let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            let hwnd = CreateWindowExW(
                ex,
                name.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                return Err("eye care: CreateWindowExW failed".to_string());
            }
            st.hwnd = hwnd as isize;
            install_hook(&mut st);
        }
        let hwnd = st.hwnd as Hwnd;
        if SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA) == 0 {
            return Err("eye care: SetLayeredWindowAttributes failed".to_string());
        }
        // 顺序有讲究：先定不透明度再显示，避免"闪一下全色"。
        place(hwnd);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    st.alpha = alpha;
    Ok(())
}

/// 撤销遮罩（退出时调用；不调用也只有进程退出才消失）。
pub fn shutdown() {
    let mut st = lock();
    remove_hook(&mut st);
    if st.hwnd != 0 {
        unsafe {
            let hwnd = st.hwnd as Hwnd;
            ShowWindow(hwnd, SW_HIDE);
            DestroyWindow(hwnd);
        }
        st.hwnd = 0;
    }
    // 类画刷随进程一起回收：窗口类本身也还在注册表里，
    // 这时候释放画刷会让残留的类指向野指针。
}

/// 当前是否贴着遮罩（托盘菜单 / 调试用）。
pub fn is_active() -> bool {
    lock().hwnd != 0
}

/// 当前不透明度（0 = 没贴）。
pub fn alpha() -> u8 {
    let st = lock();
    if st.hwnd == 0 {
        0
    } else {
        st.alpha
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真的注册类、建窗、贴上、收起。需要交互式 Windows 会话。
    #[test]
    #[ignore = "creates a real overlay window; needs an interactive Windows session"]
    fn overlay_can_be_applied_updated_and_removed() {
        assert!(!is_active());
        apply(true, 120).expect("create overlay");
        assert!(is_active());
        assert_eq!(alpha(), 120);
        {
            let st = lock();
            assert_ne!(st.hook_foreground, 0, "遮罩活跃时应挂前台钩子");
            assert_ne!(st.hook_show, 0, "遮罩活跃时应挂显示钩子");
        }

        // 幂等：重复 apply 只是改不透明度 / 重贴几何，不重建窗口。
        let before = lock().hwnd;
        apply(true, 60).expect("update overlay");
        assert_eq!(alpha(), 60);
        assert_eq!(lock().hwnd, before, "重复 apply 不该重建窗口");

        apply(false, 60).expect("hide overlay");
        assert!(!is_active());
        assert_eq!(alpha(), 0, "收起后对外报告“无遮罩”");
        {
            let st = lock();
            assert_eq!(st.hook_foreground, 0, "收起时该摘掉前台钩子");
            assert_eq!(st.hook_show, 0, "收起时该摘掉显示钩子");
        }

        // 关掉再开要能重新建起来（设置里反复切换就是这条路径）。
        apply(true, 30).expect("re-create overlay");
        assert_eq!(alpha(), 30);
        shutdown();
        assert!(!is_active());
    }
}
