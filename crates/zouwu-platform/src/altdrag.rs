//! Alt 拖动窗口（AltSnap 的最小集）。
//!
//! 三件事，别的一概不做：
//!
//! * 按住 `Alt` + 左键拖动 = 移动光标下的窗口；
//! * 按住 `Alt` + 双击 = 最大化 / 还原；
//! * 按住 `Alt` + 滚轮上 = 最大化，滚轮下 = 最小化。
//!
//! ## 为什么自己挪窗口，而不是让系统进入它的移动循环
//!
//! 系统自带的移动是"给目标窗口 `WM_NCLBUTTONDOWN(HTCAPTION)`"：它会进入目标
//! 进程的**模态循环**，把鼠标吞掉，我们的钩子此后收不到事件（也拿不到"拖到
//! 哪儿了"）。所以这里改成自己算：记下按下时的光标和窗口原点，之后每个
//! `WM_MOUSEMOVE` 用 `SetWindowPos` 挪一次。
//!
//! ## 为什么钩子跑在独立线程
//!
//! `WH_MOUSE_LL` 的回调是**同步**的：回调不返回，鼠标就卡着。ADR-037 的教训
//! 是"主线程上任何阻塞都会让整个挂件像死了一样"，所以钩子从一开始就放在
//! 自己的线程里，主线程（winit 事件循环）不参与。
//!
//! 同理，`SetWindowPos` 一律带 `SWP_ASYNCWINDOWPOS`：跨进程挪窗口时系统本来
//! 要同步给目标线程发 `WM_WINDOWPOSCHANGING`，目标忙（比如它自己在算东西）
//! 就会把我们的钩子一起拖住，超过 `LowLevelHooksTimeout` 系统会**直接把钩子
//! 卸掉**——那才是"功能时有时无"的经典成因。异步投递牺牲一点点平滑，换来
//! 回调永远在微秒级返回。
//!
//! ## 饿死系统的坑：`SetWinEventHook` 没有，这里也没有
//!
//! 唯一躲不开的同步调用是 `WindowFromPoint`：它会给光标下的窗口
//! 发 `WM_NCHITTEST`。这是必要的（否则点到的是别人的子窗口，挪的却是整窗），
//! 代价是对方进程假死时我们这条钩子会被系统判定超时。真发生了只影响"下一次
//! 拖动"，重启挂件即恢复，不会污染配置。

#![cfg(target_os = "windows")]

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

type Hwnd = *mut c_void;
type Hinstance = *mut c_void;
type Hhook = *mut c_void;
type Bool = i32;
type Lresult = isize;
type Hookproc = unsafe extern "system" fn(code: i32, wparam: usize, lparam: isize) -> Lresult;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: Point,
    private: u32,
}

/// `MSLLHOOKSTRUCT`：低层鼠标钩子的 `lparam` 指向它。
#[repr(C)]
struct Msllhookstruct {
    pt: Point,
    mouse_data: u32,
    flags: u32,
    time: u32,
    extra: usize,
}

const WH_MOUSE_LL: i32 = 14;
const HC_ACTION: i32 = 0;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_QUIT: u32 = 0x0012;

const VK_MENU: i32 = 0x12;
const VK_LBUTTON: i32 = 0x01;

const GA_ROOT: u32 = 2;

const SM_CXDOUBLECLK: i32 = 36;
const SM_CYDOUBLECLK: i32 = 37;
const SM_CXDRAG: i32 = 68;
const SM_CYDRAG: i32 = 69;

const SW_MAXIMIZE: i32 = 3;
const SW_MINIMIZE: i32 = 6;
const SW_RESTORE: i32 = 9;

const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
/// 异步投递：绝不阻塞钩子线程（见模块文档）。
const SWP_ASYNCWINDOWPOS: u32 = 0x4000;

/// 注入的鼠标事件（`SendInput` 之类）默认直接放过：键盘鼠标重映射工具会大量
/// 注入合成事件，跟着它们挪窗口只会莫名其妙。
///
/// 环境变量 `ZOUWU_ALTDRAG=injected` 时不再过滤——只给自动化验证用（测试脚本
/// 只能用 `SendInput` 造手势，否则这个功能无法端到端验证）。
const LLMHF_INJECTED: u32 = 0x0000_0001;

fn accept_injected() -> bool {
    static ACCEPT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ACCEPT.get_or_init(|| {
        std::env::var("ZOUWU_ALTDRAG").is_ok_and(|v| v.eq_ignore_ascii_case("injected"))
    })
}

/// 滚轮防抖：精密触控板一次手势会连发十几个小 delta，
/// 不防抖就是"最大化-最小化"花式抖动。
const WHEEL_DEBOUNCE: Duration = Duration::from_millis(120);

/// 调试用事件跟踪：`ZOUWU_ALTDRAG_TRACE=<文件路径>` 时把每次钩子事件追加进去。
/// 默认关；开着的时候是同步写文件，只给排障用（正常路径一次也不碰）。
fn trace(line: &str) {
    static PATH: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    let Some(path) = PATH
        .get_or_init(|| std::env::var_os("ZOUWU_ALTDRAG_TRACE").map(Into::into))
        .as_ref()
    else {
        return;
    };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Hinstance;
    fn SetWindowsHookExW(id_hook: i32, proc: Hookproc, module: Hinstance, thread: u32) -> Hhook;
    fn UnhookWindowsHookEx(hook: Hhook) -> Bool;
    fn CallNextHookEx(hook: Hhook, code: i32, wparam: usize, lparam: isize) -> Lresult;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> Bool;
    fn PostThreadMessageW(thread: u32, msg: u32, wparam: usize, lparam: isize) -> Bool;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentProcessId() -> u32;
    fn GetAsyncKeyState(key: i32) -> i16;
    fn GetCursorPos(pt: *mut Point) -> Bool;
    fn WindowFromPoint(pt: Point) -> Hwnd;
    fn GetAncestor(hwnd: Hwnd, flags: u32) -> Hwnd;
    fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
    fn IsWindowVisible(hwnd: Hwnd) -> Bool;
    fn IsZoomed(hwnd: Hwnd) -> Bool;
    fn IsIconic(hwnd: Hwnd) -> Bool;
    fn GetClassNameW(hwnd: Hwnd, buf: *mut u16, len: i32) -> i32;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> Bool;
    fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> Bool;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> Bool;
    fn GetSystemMetrics(index: i32) -> i32;
    fn GetDoubleClickTime() -> u32;
}

// ── 手势判定（纯逻辑，可测） ─────────────────────────────

/// 系统认定"这两下算双击"的口径。
#[derive(Clone, Copy, Debug)]
pub struct DoubleClick {
    pub time: Duration,
    pub cx: i32,
    pub cy: i32,
}

impl DoubleClick {
    fn from_system() -> Self {
        unsafe {
            Self {
                time: Duration::from_millis(GetDoubleClickTime().max(200) as u64),
                cx: GetSystemMetrics(SM_CXDOUBLECLK).max(2),
                cy: GetSystemMetrics(SM_CYDOUBLECLK).max(2),
            }
        }
    }
}

/// 两次按下的归类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// 隔得够远 / 够久：一次新的按下。
    Click,
    /// 时间与位移都在系统双击口径内：双击的第二下。
    Double,
}

/// 左键的按下—移动—抬起状态机。
///
/// 单独抽出来是为了能测：钩子本身在测试里没法安全地跑（它真的会劫持鼠标）。
#[derive(Debug, Default)]
pub struct Tracker {
    start: Option<(i32, i32)>,
    last_click: Option<(Instant, (i32, i32))>,
    dragging: bool,
}

impl Tracker {
    /// 左键按下。返回这一下是单击还是双击的第二下。
    pub fn down(&mut self, now: Instant, pos: (i32, i32), dbl: DoubleClick) -> Press {
        let double = match self.last_click {
            Some((at, p)) => {
                // 双击口径是"距离不超过矩形的一半"，和系统一致。
                now.duration_since(at) <= dbl.time
                    && (pos.0 - p.0).abs() <= dbl.cx / 2
                    && (pos.1 - p.1).abs() <= dbl.cy / 2
            }
            None => false,
        };
        self.last_click = Some((now, pos));
        self.start = Some(pos);
        self.dragging = false;
        if double {
            Press::Double
        } else {
            Press::Click
        }
    }

    /// 移动。返回 `true` 表示"这一下该挪窗口"（包括刚开始拖的第一下）。
    pub fn moved(&mut self, pos: (i32, i32), threshold: (i32, i32)) -> bool {
        if self.dragging {
            return true;
        }
        let Some(start) = self.start else {
            return false;
        };
        if (pos.0 - start.0).abs() < threshold.0 && (pos.1 - start.1).abs() < threshold.1 {
            return false;
        }
        self.dragging = true;
        true
    }

    /// 左键抬起。返回 `true` 表示刚才在拖。
    pub fn up(&mut self) -> bool {
        let was = self.dragging;
        if was {
            // 拖过之后紧接着的一下不算双击的第二下：
            // "拖到这儿，然后点一下"是最常见的连贯操作。
            self.last_click = None;
        }
        self.dragging = false;
        self.start = None;
        was
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 丢掉所有状态（Alt 松开、切换目标时用）。
    pub fn clear(&mut self) {
        self.start = None;
        self.dragging = false;
    }
}

/// 滚轮方向 -> 动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wheel {
    Maximize,
    Minimize,
}

/// `WHEEL_DELTA` 是 120；正数向上（远离用户）。
pub fn wheel_action(delta: i16) -> Option<Wheel> {
    match delta {
        d if d > 0 => Some(Wheel::Maximize),
        d if d < 0 => Some(Wheel::Minimize),
        _ => None,
    }
}

// ── Win32 侧 ────────────────────────────────────────────

#[derive(Debug)]
struct Drag {
    hwnd: isize,
    /// 按下时光标的位置。
    from: (i32, i32),
    /// 按下时窗口的原点。
    origin: (i32, i32),
}

#[derive(Default)]
struct HookState {
    tracker: Tracker,
    /// 已经按下、还没开始拖的目标窗口。
    pending: Option<isize>,
    drag: Option<Drag>,
    /// 这一次抬起要吞掉（拖完的那一下不该再被目标程序当成点击）。
    swallow_up: bool,
    last_wheel: Option<Instant>,
}

thread_local! {
    /// 钩子只在钩子线程上被调用，所以线程本地就够，不需要锁。
    static STATE: RefCell<HookState> = RefCell::new(HookState::default());
}

static HOOK: AtomicIsize = AtomicIsize::new(0);
static THREAD: AtomicU32 = AtomicU32::new(0);

/// 钩子线程的句柄。`stop` 要等它真的退完才能返回——否则“关掉又马上开”会
/// 在旧线程还没卸钩子的时候建第二条线程，两个钩子同时拖窗口（双倍位移）。
fn thread_handle() -> std::sync::MutexGuard<'static, Option<std::thread::JoinHandle<()>>> {
    static HANDLE: std::sync::Mutex<Option<std::thread::JoinHandle<()>>> =
        std::sync::Mutex::new(None);
    HANDLE.lock().unwrap_or_else(|e| e.into_inner())
}

fn key_down(vk: i32) -> bool {
    unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
}

fn class_name(hwnd: Hwnd) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// 桌面的几个壳窗口不该被移动。
fn is_shell(hwnd: Hwnd) -> bool {
    matches!(
        class_name(hwnd).as_str(),
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "NotifyIconOverflowWindow"
    )
}

/// 光标底下"值得操作"的顶层窗口。
fn target() -> Option<Hwnd> {
    unsafe {
        let mut pt = Point::default();
        if GetCursorPos(&mut pt) == 0 {
            return None;
        }
        let hit = WindowFromPoint(pt);
        if hit.is_null() {
            return None;
        }
        let root = GetAncestor(hit, GA_ROOT);
        if root.is_null() || IsWindowVisible(root) == 0 || IsIconic(root) != 0 {
            return None;
        }
        // 自己的窗口（挂件、面板、护眼遮罩）一概不碰：
        // 挂件自己的拖动走的是 GetCursorPos 轮询，抢过来只会互相打架。
        let mut pid = 0u32;
        GetWindowThreadProcessId(root, &mut pid);
        if pid == GetCurrentProcessId() || is_shell(root) {
            return None;
        }
        Some(root)
    }
}

fn toggle_maximize(hwnd: Hwnd) {
    unsafe {
        if IsZoomed(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else {
            ShowWindow(hwnd, SW_MAXIMIZE);
        }
    }
}

fn resize_action(hwnd: Hwnd, what: Wheel) {
    unsafe {
        match what {
            Wheel::Maximize => {
                ShowWindow(hwnd, SW_MAXIMIZE);
            }
            Wheel::Minimize => {
                ShowWindow(hwnd, SW_MINIMIZE);
            }
        }
    }
}

/// 钩子用到的"现场状态"。抽成参数是为了让拖动 / 双击这些路径能被确定性验证：
/// 真机上 `alt` / `lbtn` 来自 `GetAsyncKeyState`、目标窗口来自 `WindowFromPoint`，
/// 测试里可以给死值 + 给一个固定的窗口 —— 之前只靠注入合成事件验证，结果这台
/// 机器上是"用户同时在动鼠标"，看到的事件流根本不可信。
struct Env<'a> {
    alt: bool,
    lbtn: bool,
    find: &'a dyn Fn() -> Option<Hwnd>,
}

/// 用状态机跑一次鼠标事件；返回 `true` 表示"吞掉这条消息"。
fn handle(msg: u32, data: &Msllhookstruct, now: Instant) -> bool {
    let pos = (data.pt.x, data.pt.y);
    let env = Env {
        alt: key_down(VK_MENU),
        lbtn: key_down(VK_LBUTTON),
        find: &target,
    };
    if std::env::var_os("ZOUWU_ALTDRAG_TRACE").is_some() {
        let hit = if env.alt {
            (env.find)().map(|h| h as isize)
        } else {
            None
        };
        trace(&format!(
            "msg={msg:#06x} pos={pos:?} alt={} lbtn={} data={:#x} target={hit:?} injected={}",
            env.alt,
            env.lbtn,
            data.mouse_data,
            data.flags & LLMHF_INJECTED != 0
        ));
    }
    handle_event(msg, pos, data.mouse_data, now, &env)
}

/// 事件状态机的本体（不碰任何全局输入状态，全部从 `env` / `pos` 来）。
fn handle_event(msg: u32, pos: (i32, i32), mouse_data: u32, now: Instant, env: &Env) -> bool {
    let alt = env.alt;

    match msg {
        WM_LBUTTONDOWN => {
            if !alt {
                return false;
            }
            let Some(hwnd) = (env.find)() else {
                return false;
            };
            let dbl = DoubleClick::from_system();
            let press = STATE.with(|cell| match cell.try_borrow_mut() {
                Ok(mut st) => {
                    st.pending = Some(hwnd as isize);
                    st.drag = None;
                    st.tracker.down(now, pos, dbl)
                }
                Err(_) => Press::Click,
            });
            match press {
                Press::Double => {
                    toggle_maximize(hwnd);
                    // 吞掉第二下：否则目标程序会同时认为"用户双击了"，
                    // 在编辑框里就是顺手选中一个词。
                    STATE.with(|cell| {
                        if let Ok(mut st) = cell.try_borrow_mut() {
                            st.pending = None;
                            st.tracker.clear();
                        }
                    });
                    true
                }
                Press::Click => {
                    // 一下不吞：普通 Alt+单击在不少程序里有自己的含义
                    // （取色、加选之类），我们只借用"按住时还拖了"这个动作。
                    false
                }
            }
        }
        WM_MOUSEMOVE => {
            let mut moved_to = None;
            let swallow = STATE.with(|cell| {
                let Ok(mut st) = cell.try_borrow_mut() else {
                    return false;
                };
                if st.drag.is_some() {
                    if !alt || !env.lbtn {
                        // Alt 中途松开 / 按键丢了：就地结束，不然窗口会跟着光标乱飞。
                        st.drag = None;
                        st.tracker.clear();
                        return false;
                    }
                    moved_to = Some(pos);
                    return true;
                }
                let Some(pending) = st.pending else {
                    return false;
                };
                let threshold = unsafe {
                    (
                        GetSystemMetrics(SM_CXDRAG).max(2),
                        GetSystemMetrics(SM_CYDRAG).max(2),
                    )
                };
                if !st.tracker.moved(pos, threshold) {
                    return false;
                }
                let hwnd = pending as Hwnd;
                let mut rect = Rect::default();
                if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
                    st.pending = None;
                    return false;
                }
                st.drag = Some(Drag {
                    hwnd: pending,
                    from: pos,
                    origin: (rect.left, rect.top),
                });
                st.swallow_up = true;
                moved_to = Some(pos);
                true
            });
            if let Some(pos) = moved_to {
                STATE.with(|cell| {
                    if let Ok(st) = cell.try_borrow() {
                        if let Some(drag) = st.drag.as_ref() {
                            move_window(drag, pos);
                        }
                    }
                });
            }
            swallow
        }
        WM_LBUTTONUP => {
            let (dragging, swallow) = STATE.with(|cell| {
                let Ok(mut st) = cell.try_borrow_mut() else {
                    return (false, false);
                };
                let dragging = st.tracker.up();
                st.drag = None;
                st.pending = None;
                let swallow = st.swallow_up || dragging;
                st.swallow_up = false;
                (dragging, swallow)
            });
            let _ = dragging;
            swallow
        }
        WM_MOUSEWHEEL => {
            if !alt {
                return false;
            }
            let delta = (mouse_data >> 16) as u16 as i16;
            let Some(what) = wheel_action(delta) else {
                return false;
            };
            // 滚轮要先判防抖再判目标窗口，否则每次防抖失败都会白付一次 WindowFromPoint。
            let fresh = STATE.with(|cell| match cell.try_borrow_mut() {
                Ok(mut st) => match st.last_wheel {
                    // 用事件时间戳而不是 `elapsed()`：钩子给的时间才是"这次手势什么时候发生"，
                    // 也才能被测试用合成时间戳驱动（`elapsed()` 是墙上时间，喂不进去）。
                    Some(at) if now.duration_since(at) < WHEEL_DEBOUNCE => false,
                    _ => {
                        st.last_wheel = Some(now);
                        true
                    }
                },
                Err(_) => false,
            });
            if !fresh {
                return true;
            }
            if let Some(hwnd) = (env.find)() {
                resize_action(hwnd, what);
                return true;
            }
            false
        }
        _ => false,
    }
}

fn move_window(drag: &Drag, pos: (i32, i32)) {
    unsafe {
        let x = drag.origin.0 + (pos.0 - drag.from.0);
        let y = drag.origin.1 + (pos.1 - drag.from.1);
        SetWindowPos(
            drag.hwnd as Hwnd,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        );
    }
}

unsafe extern "system" fn on_mouse(code: i32, wparam: usize, lparam: isize) -> Lresult {
    if code != HC_ACTION || lparam == 0 {
        return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
    }
    let data = &*(lparam as *const Msllhookstruct);
    if data.flags & LLMHF_INJECTED != 0 && !accept_injected() {
        return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
    }
    // 吞掉 = 返回 1；放过 = 交给下一个钩子。
    if handle(wparam as u32, data, Instant::now()) {
        1
    } else {
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }
}

/// 钩子线程主体：装钩子 -> 泵消息（低层钩子的回调是"投递到安装线程"的，
/// 没有消息循环就永远不会被调用）-> 收到 `WM_QUIT` 后卸钩子退出。
fn pump(hook: Hhook) {
    unsafe {
        let mut msg: Msg = std::mem::zeroed();
        loop {
            let ret = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if ret <= 0 {
                break;
            }
        }
        UnhookWindowsHookEx(hook);
    }
    HOOK.store(0, Ordering::SeqCst);
    THREAD.store(0, Ordering::SeqCst);
}

fn install_and_pump(report: mpsc::Sender<Result<(), String>>) {
    unsafe {
        THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        let hook = SetWindowsHookExW(WH_MOUSE_LL, on_mouse, GetModuleHandleW(std::ptr::null()), 0);
        if hook.is_null() {
            THREAD.store(0, Ordering::SeqCst);
            let err = std::io::Error::last_os_error();
            let _ = report.send(Err(format!("SetWindowsHookExW failed: {err}")));
            return;
        }
        HOOK.store(hook as isize, Ordering::SeqCst);
        let _ = report.send(Ok(()));
        pump(hook);
    }
}

/// 开启 / 关闭 Alt 拖动。幂等。
///
/// 返回 `Err` 只代表“这次没打开”（比如系统拒绝装钩子），调用方按功能不可用
/// 处理即可，不该让挂件启动失败。
pub fn apply(enabled: bool) -> Result<(), String> {
    if !enabled {
        stop();
        return Ok(());
    }
    {
        // 线程在跑（正在装或已装好）就不重复装。
        let handle = thread_handle();
        if handle.is_some() {
            return Ok(());
        }
    }
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("zouwu-altdrag".to_string())
        .spawn(move || install_and_pump(tx));
    match spawned {
        Ok(handle) => *thread_handle() = Some(handle),
        Err(err) => return Err(format!("alt drag: spawn failed: {err}")),
    }
    // 等钩子真的装上再返回：这样“开了但没反应”能在日志里立刻看见原因。
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => {
            stop();
            Err(err)
        }
        Err(_) => {
            stop();
            Err("alt drag: hook thread did not report back".to_string())
        }
    }
}

/// 关掉（幂等）：给钩子线程投 `WM_QUIT` 并**等它卸完钩子退出**。
pub fn stop() {
    let mut handle = thread_handle();
    let thread = THREAD.load(Ordering::SeqCst);
    if thread != 0 {
        unsafe {
            PostThreadMessageW(thread, WM_QUIT, 0, 0);
        }
    }
    if let Some(join) = handle.take() {
        // 线程此刻只可能在两件事上：阻塞在 `GetMessage`（立刻返回），
        // 或正在一次钩子回调里（微秒级）。所以这里等得住，不是无限期阻塞。
        let _ = join.join();
    }
}

/// 退出前调用（不调用也只是进程结束时一起消失）。
pub fn shutdown() {
    stop();
}

/// 当前是否装着钩子。
///
/// 直接看句柄，不另存一个 `bool`：两个变量描述同一件事，就一定会出现
/// “说没开其实开着”的时刻（实测就是这么抓到双钩子 bug 的）。
pub fn is_active() -> bool {
    HOOK.load(Ordering::SeqCst) != 0
}

/// 当前钩子句柄（调试 / 测试用，0 = 没装）。
pub fn hook_handle() -> isize {
    HOOK.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 真窗口：拖动 / 最大化这些动作最终要落到一个真的 HWND 上，
    //    光测 `Tracker` 测不到 `SetWindowPos` 与"窗口真的动了"这一层。
    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassW(class: *const Wndclassw) -> u16;
        fn CreateWindowExW(
            exstyle: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            parent: Hwnd,
            menu: Hwnd,
            instance: Hinstance,
            param: *mut c_void,
        ) -> Hwnd;
        fn DestroyWindow(hwnd: Hwnd) -> Bool;
        fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Lresult;
    }

    #[repr(C)]
    struct Wndclassw {
        style: u32,
        wnd_proc: Option<unsafe extern "system" fn(Hwnd, u32, usize, isize) -> Lresult>,
        cls_extra: i32,
        wnd_extra: i32,
        instance: Hinstance,
        icon: Hwnd,
        cursor: Hwnd,
        background: Hwnd,
        menu_name: *const u16,
        class_name: *const u16,
    }

    const WS_POPUP: u32 = 0x8000_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;

    /// 滚轮事件的 `mouse_data`：高 16 位是有符号 delta。
    fn wheel_data(delta: i16) -> u32 {
        (delta as u16 as u32) << 16
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 造一个真的顶层窗口（有类名、能被 `SetWindowPos` 挪、能被 `ShowWindow` 最大化）。
    fn test_window(x: i32, y: i32) -> Hwnd {
        let class = wide("ZouwuAltDragTest");
        let title = wide("zouwu altdrag test");
        unsafe {
            let wc = Wndclassw {
                style: 0,
                wnd_proc: Some(DefWindowProcW),
                cls_extra: 0,
                wnd_extra: 0,
                instance: GetModuleHandleW(std::ptr::null()),
                icon: std::ptr::null_mut(),
                cursor: std::ptr::null_mut(),
                background: std::ptr::null_mut(),
                menu_name: std::ptr::null(),
                class_name: class.as_ptr(),
            };
            RegisterClassW(&wc); // 重复注册失败也没关系（测试并行时同进程只会有一个类）
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                400,
                300,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null_mut(),
            );
            assert!(!hwnd.is_null(), "建不出测试窗口");
            hwnd
        }
    }

    fn origin(hwnd: Hwnd) -> (i32, i32) {
        let mut r = Rect::default();
        unsafe { GetWindowRect(hwnd, &mut r) };
        (r.left, r.top)
    }

    fn env_for(hwnd: Hwnd, alt: bool, lbtn: bool) -> Env<'static> {
        // `find` 里要捕获 hwnd：用 Box::leak 换一个 'static 闭包，测试里够用。
        let find: &'static dyn Fn() -> Option<Hwnd> = Box::leak(Box::new(move || Some(hwnd)));
        Env { alt, lbtn, find }
    }

    #[test]
    #[ignore = "需要交互式 Windows 桌面（要建真窗口）"]
    fn alt_drag_moves_the_window_by_the_cursor_delta() {
        let hwnd = test_window(200, 200);
        let start = origin(hwnd);
        let env = env_for(hwnd, true, true);
        let t0 = Instant::now();

        // 按下不吞（保留目标程序自己的 Alt+单击语义）
        assert!(!handle_event(WM_LBUTTONDOWN, (600, 600), 0, t0, &env));
        // 第一次移动只负责"超过拖动阈值、开始拖"，窗口还没动
        assert!(handle_event(WM_MOUSEMOVE, (640, 610), 0, t0, &env));
        assert_eq!(origin(hwnd), start, "刚开始拖的那一下不该跳位");
        // 之后按光标位移挪窗口
        handle_event(WM_MOUSEMOVE, (680, 620), 0, t0, &env);
        assert_eq!(
            origin(hwnd),
            (start.0 + 40, start.1 + 10),
            "窗口要跟着光标走同样的位移"
        );
        // 拖完的抬起要吞，否则目标程序会看到一次完整点击
        assert!(handle_event(WM_LBUTTONUP, (680, 620), 0, t0, &env));

        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    #[ignore = "需要交互式 Windows 桌面（要建真窗口）"]
    fn releasing_alt_midway_stops_moving_the_window() {
        let hwnd = test_window(300, 300);
        let env = env_for(hwnd, true, true);
        let t0 = Instant::now();
        handle_event(WM_LBUTTONDOWN, (700, 700), 0, t0, &env);
        handle_event(WM_MOUSEMOVE, (740, 700), 0, t0, &env);
        let dragged = origin(hwnd);

        // Alt 松开了：再动鼠标窗口不该跟着飞
        let no_alt = env_for(hwnd, false, true);
        assert!(!handle_event(WM_MOUSEMOVE, (900, 900), 0, t0, &no_alt));
        assert_eq!(origin(hwnd), dragged, "Alt 松开后窗口要停在原地");

        handle_event(WM_LBUTTONUP, (900, 900), 0, t0, &no_alt);
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    #[ignore = "需要交互式 Windows 桌面（要建真窗口）"]
    fn wheel_up_maximizes_and_wheel_down_minimizes_the_target() {
        let hwnd = test_window(400, 300);
        let env = env_for(hwnd, true, true);
        let now = Instant::now();
        // mouse_data 高 16 位是 delta：向上 = +120
        assert!(handle_event(
            WM_MOUSEWHEEL,
            (500, 400),
            wheel_data(120),
            now,
            &env
        ));
        assert_ne!(unsafe { IsZoomed(hwnd) }, 0, "Alt+滚轮上要最大化");
        // 防抖：紧接着的滚轮不该再生效（触控板一次手势会连发十几个）
        let now = now + WHEEL_DEBOUNCE + Duration::from_millis(20);
        assert!(handle_event(
            WM_MOUSEWHEEL,
            (500, 400),
            wheel_data(-120),
            now,
            &env
        ));
        assert_ne!(unsafe { IsIconic(hwnd) }, 0, "Alt+滚轮下要最小化");
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    #[ignore = "需要交互式 Windows 桌面（要建真窗口）"]
    fn wheel_within_the_debounce_window_is_ignored() {
        let hwnd = test_window(500, 300);
        let env = env_for(hwnd, true, true);
        let now = Instant::now();
        assert!(handle_event(
            WM_MOUSEWHEEL,
            (500, 400),
            wheel_data(120),
            now,
            &env
        ));
        let zoomed = unsafe { IsZoomed(hwnd) };
        // 同一次手势里的第二格：吞掉但不动窗口
        assert!(handle_event(
            WM_MOUSEWHEEL,
            (500, 400),
            wheel_data(-120),
            now + Duration::from_millis(30),
            &env
        ));
        assert_eq!(
            unsafe { IsZoomed(hwnd) },
            zoomed,
            "防抖窗口内的滚轮不该翻转状态"
        );
        unsafe { DestroyWindow(hwnd) };
    }

    #[test]
    #[ignore = "需要交互式 Windows 桌面（要建真窗口）"]
    fn double_click_with_alt_toggles_maximize() {
        let hwnd = test_window(600, 300);
        let env = env_for(hwnd, true, true);
        let t0 = Instant::now();
        assert!(
            !handle_event(WM_LBUTTONDOWN, (650, 350), 0, t0, &env),
            "第一下是单击"
        );
        assert!(!handle_event(WM_LBUTTONUP, (650, 350), 0, t0, &env));
        // 第二下落在系统双击时间与矩形之内 -> 双击
        assert!(
            handle_event(
                WM_LBUTTONDOWN,
                (651, 351),
                0,
                t0 + Duration::from_millis(120),
                &env
            ),
            "双击的第二下要吞掉"
        );
        assert_ne!(unsafe { IsZoomed(hwnd) }, 0, "Alt+双击要最大化");
        unsafe { DestroyWindow(hwnd) };
    }

    fn dbl() -> DoubleClick {
        DoubleClick {
            time: Duration::from_millis(500),
            cx: 4,
            cy: 4,
        }
    }

    #[test]
    fn second_press_inside_the_double_click_box_is_a_double() {
        let mut t = Tracker::default();
        let now = Instant::now();
        assert_eq!(t.down(now, (100, 100), dbl()), Press::Click);
        assert_eq!(
            t.down(now + Duration::from_millis(120), (101, 101), dbl()),
            Press::Double
        );
    }

    #[test]
    fn slow_or_far_second_press_is_a_new_click() {
        let mut t = Tracker::default();
        let now = Instant::now();
        assert_eq!(t.down(now, (100, 100), dbl()), Press::Click);
        assert_eq!(
            t.down(now + Duration::from_millis(900), (100, 100), dbl()),
            Press::Click,
            "太慢"
        );
        assert_eq!(
            t.down(now + Duration::from_millis(100), (160, 100), dbl()),
            Press::Click,
            "太远"
        );
    }

    #[test]
    fn drag_starts_only_after_the_drag_threshold() {
        let mut t = Tracker::default();
        let now = Instant::now();
        t.down(now, (0, 0), dbl());
        // 手抖 1px 不该让窗口跳一下。
        assert!(!t.moved((1, 1), (4, 4)));
        assert!(!t.is_dragging());
        assert!(t.moved((10, 0), (4, 4)), "超过阈值就该开始拖");
        assert!(t.is_dragging());
        // 之后每一帧都要挪，不再判断位移。
        assert!(t.moved((10, 0), (4, 4)));
    }

    #[test]
    fn moving_without_a_press_never_drags() {
        let mut t = Tracker::default();
        assert!(!t.moved((500, 500), (4, 4)));
        assert!(!t.is_dragging());
    }

    #[test]
    fn a_drag_does_not_make_the_next_click_a_double() {
        let mut t = Tracker::default();
        let now = Instant::now();
        t.down(now, (0, 0), dbl());
        assert!(t.moved((40, 40), (4, 4)));
        assert!(t.up());
        // 拖完立刻在原地再点一下：这是"拖到这儿然后点一下"，不是双击。
        assert_eq!(
            t.down(now + Duration::from_millis(80), (40, 40), dbl()),
            Press::Click
        );
    }

    #[test]
    fn up_reports_whether_we_were_dragging() {
        let mut t = Tracker::default();
        let now = Instant::now();
        t.down(now, (0, 0), dbl());
        assert!(!t.up(), "单击抬起不该被当成拖动");
        t.down(now, (0, 0), dbl());
        t.moved((30, 0), (4, 4));
        assert!(t.up());
        assert!(!t.is_dragging());
    }

    #[test]
    fn wheel_direction_maps_to_maximize_and_minimize() {
        assert_eq!(wheel_action(120), Some(Wheel::Maximize));
        assert_eq!(wheel_action(360), Some(Wheel::Maximize));
        assert_eq!(wheel_action(-120), Some(Wheel::Minimize));
        assert_eq!(wheel_action(0), None);
    }

    #[test]
    fn stop_is_idempotent_and_start_reports_failure_without_poisoning_state() {
        assert!(!is_active());
        stop();
        stop();
        assert!(!is_active(), "没开着的时候停一下不该变成“开着”");
        assert_eq!(hook_handle(), 0);
    }

    /// 真的装一次低层鼠标钩子再卸掉。需要交互式 Windows 会话。
    ///
    /// 测试期间钩子是活的：只要没人按 Alt，它不会动任何窗口；
    /// 但别在自动化点击的过程中跑它。
    #[test]
    #[ignore = "installs a real low-level mouse hook; needs an interactive Windows session"]
    fn hook_can_be_installed_and_removed() {
        apply(true).expect("install hook");
        assert!(is_active());
        assert_ne!(hook_handle(), 0, "装上之后应该有句柄");
        apply(true).expect("second apply is a no-op");
        apply(false).expect("remove hook");
        assert!(!is_active());
        assert_eq!(hook_handle(), 0, "卸掉之后句柄要清零");
        // `stop` 必须等线程退完：不然“关掉又马上开”会在旧钩子还在的时候
        // 再装一个，两个钩子同时拖窗口。
        assert_eq!(
            THREAD.load(Ordering::SeqCst),
            0,
            "stop 返回时线程应该已经没了"
        );
        assert!(thread_handle().is_none(), "句柄也要收掉");
        // 关了还能再开（设置里反复拨开关就是这条路径）。
        apply(true).expect("re-install");
        assert!(is_active());
        shutdown();
        assert!(!is_active());
    }
}
