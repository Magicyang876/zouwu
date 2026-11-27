//! Linux/Wayland 的"小兽窗口"宿主。
//!
//! 为什么不是 winit 窗口：Wayland 的 `xdg_toplevel` 不给客户端**置顶**、
//! **绝对坐标定位**、**点击穿透**这三件事，而桌面挂件全靠这三条。能做这三条
//! 的只有 `zwlr_layer_shell_v1`（KWin 支持），而 winit 不暴露 layer-shell
//! （一个 `wl_surface` 也只能有一个 role，硬挂会被合成器判协议错误）。
//!
//! 于是：**小兽本体**是自己那条 Wayland 连接上的 layer 表面
//! （[`zouwu_platform::layer::PetSurface`]），**面板窗口**（设置/历史/报告/
//! 系统字体/皮肤菜单）仍然是 winit 窗口——它们是正经窗口，winit 完全够用。
//!
//! 本模块做的事就是把 PetSurface 包装成 app.rs 认识的那个形状（[`PetWindow`]），
//! 并把"有输入了"这件事通过 winit 的 user event 叫醒事件循环。方法名和语义
//! 一律对齐 `winit::window::Window`，所以 app.rs 的主状态机可以原样复用：
//! 它只用到 `inner_size / scale_factor / outer_position / set_outer_position /
//! request_inner_size / request_redraw` 这几个。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event_loop::EventLoopProxy;
use winit::window::WindowId;
use zouwu_platform::layer::PetSurface;

/// 事件循环的 user event：Wayland 线程唯一能"戳"到 UI 线程的手段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserEvent {
    /// 小兽表面上有新输入（悬停/按下/离开），该排空 `PetInput` 了。
    Input,
    /// 有人调了 `request_redraw`（winit 的 `RedrawRequested` 在这里的手工版）。
    Redraw,
}

/// 事件循环的唤醒器。Wayland 线程回调它，UI 线程被叫醒后去 `take_input`。
///
/// 不用 `Arc<EventLoopProxy>` 直接当回调，是因为 `EventLoopProxy` 的 `Sync`
/// 在各后端不一致；走一个 `static` + `Mutex` 就只用得到 `Send`。
static PROXY: Mutex<Option<EventLoopProxy<UserEvent>>> = Mutex::new(None);
static PROXY_READY: OnceLock<()> = OnceLock::new();

/// 装好唤醒器。**必须在 `run_app` 之前**调用，否则早到的输入会丢。
pub fn install_waker(proxy: EventLoopProxy<UserEvent>) {
    // 全局热键是**从 D-Bus 线程**来的（kglobalaccel 的信号）：后端也需要一条
    // “醒一醒”的通路，否则事件循环在 `ControlFlow::Wait` 里睡着，按下热键
    // 永远没人去取队列。
    let hotkey_proxy = proxy.clone();
    zouwu_platform::hotkey::set_waker(std::sync::Arc::new(move || {
        let _ = hotkey_proxy.send_event(UserEvent::Input);
    }));
    // 注销/关机/`pkill` 都是 SIGTERM。不接管的话 `App::exiting()` 一行都不跑，
    // 被改过的 kwinrc（Alt 拖动）就永远留在用户机器上。
    let signal_proxy = proxy.clone();
    zouwu_platform::signal::install(std::sync::Arc::new(move || {
        let _ = signal_proxy.send_event(UserEvent::Input);
    }));
    // Wayland 没有“把自己的窗口抬到最前”的接口，面板靠 KWin 脚本代劳；
    // 装一次，之后每次开面板调一次。不是 KDE 就静默跳过。
    zouwu_platform::activate::install();
    *PROXY.lock().unwrap_or_else(|e| e.into_inner()) = Some(proxy);
    let _ = PROXY_READY.set(());
}

/// 叫醒事件循环。可以随便从哪个线程调，重复叫醒是安全的。
pub fn wake(event: UserEvent) {
    let proxy = PROXY.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(proxy) = proxy {
        let _ = proxy.send_event(event);
    }
}

/// 小兽本体的窗口外观（和 `winit::window::Window` 同名同语义）。
pub struct PetWindow {
    surface: Arc<PetSurface>,
    /// 上一帧是不是让事件循环重绘过（`request_redraw` 的合并去重）。
    redraw_pending: AtomicBool,
}

impl PetWindow {
    /// 建窗口并把它的"有输入了"接到事件循环上。
    pub fn new(surface: Arc<PetSurface>) -> Self {
        let waker: Arc<dyn Fn() + Send + Sync> = Arc::new(|| wake(UserEvent::Input));
        surface.set_waker(waker);
        Self {
            surface,
            redraw_pending: AtomicBool::new(false),
        }
    }

    /// 底层表面（拖动、置顶、几何都要它）。
    pub fn surface(&self) -> &Arc<PetSurface> {
        &self.surface
    }

    /// 平台的原始窗口句柄：Linux 下是"表面编号"。
    /// 托盘/显隐/几何这些原生概念在 app.rs 里统一走 `layer::*`，靠它认表面。
    pub fn id_raw(&self) -> isize {
        self.surface.id()
    }

    pub fn id(&self) -> WindowId {
        // 和 Windows 侧 `WindowId::from(hwnd as u64)` 同一个思路：
        // 让"原生句柄"和"窗口 id"一一对应，面板遮挡判定才写得出。
        WindowId::from(self.surface.id() as u64)
    }

    /// 所在屏的刷新率（Hz）。UI 拿它定帧节拍（见 `App::frame_interval`）。
    pub fn refresh_hz(&self) -> Option<f32> {
        self.surface.refresh_hz()
    }

    pub fn scale_factor(&self) -> f64 {
        self.surface.scale()
    }

    /// **坐标空间**的缩放（整数 `wl_output.scale`）。
    ///
    /// 位置、光标、`outer_position`/`set_outer_position` 全在这套坐标里换算；
    /// `scale_factor()` 是 buffer 的分数缩放（1.15 这种），只用于 buffer 尺寸、
    /// 画布和文字。拿它去把「屏幕物理坐标」换成「本地逻辑坐标」，在本体大小、
    /// 命中判定和拖动起点上都会偏掉 `fractional / integer` 倍（1.15 的屏上
    /// 1.74 倍），表现就是鼠标按在小兽身上却判定成没按到、拖不动。
    pub fn monitor_scale(&self) -> f64 {
        self.surface.monitor_scale().max(1) as f64
    }
    pub fn inner_size(&self) -> PhysicalSize<u32> {
        let (w, h) = self.surface.size();
        PhysicalSize::new(w, h)
    }

    pub fn outer_position(&self) -> Result<PhysicalPosition<i32>, String> {
        let (x, y) = self.surface.pos();
        // 只用**这块表面所在屏**的缩放。曾经按坐标去查"落在哪块屏"再取缩放，
        // 结果和 `layer::cursor_position()` 用了两套换算，拖动时窗口和鼠标
        // 对不上（位移被放大一倍）。坐标只有一套真相才不会打架。
        let scale = self.close_scale();
        Ok(PhysicalPosition::new(
            (x as f64 * scale).round() as i32,
            (y as f64 * scale).round() as i32,
        ))
    }

    pub fn set_outer_position(&self, position: PhysicalPosition<i32>) {
        let scale = self.close_scale();
        self.surface.set_position(
            (position.x as f64 / scale).round() as i32,
            (position.y as f64 / scale).round() as i32,
        );
    }

    /// 表面所在 output 的缩放（跨屏重建期间可能短暂是旧值，可接受）。
    ///
    /// 用**整数**的 monitor 缩放：位置/光标/monitors() 全在这套坐标空间里，
    /// 混进 1.15 这种分数会让拖动位移和命中判定对不上（老毛病了）。
    fn close_scale(&self) -> f64 {
        self.surface.monitor_scale().max(1) as f64
    }

    /// 逻辑尺寸请求 → 表面物理尺寸。返回 `Some` 和 winit 一样表示"接受了"。
    pub fn request_inner_size(&self, size: LogicalSize<f32>) -> Option<PhysicalSize<u32>> {
        let scale = self.surface.scale();
        let physical = PhysicalSize::new(
            ((size.width as f64 * scale).round() as u32).max(1),
            ((size.height as f64 * scale).round() as u32).max(1),
        );
        self.surface.resize(physical.width, physical.height);
        Some(physical)
    }

    /// 请求一帧。等价于 winit 的 `request_redraw`：下一轮事件循环画。
    pub fn request_redraw(&self) {
        // 合并：连点期间 `repaint_now` 会连叫好几次，只值一趟。
        if !self.redraw_pending.swap(true, Ordering::AcqRel) {
            wake(UserEvent::Redraw);
        }
    }

    /// 取出"重绘请求"并把标志清掉（事件循环用）。
    pub fn take_redraw(&self) -> bool {
        self.redraw_pending.swap(false, Ordering::AcqRel)
    }

    /// 表面被合成器关掉了（output 拔了/缩放变了要重建）。
    pub fn closed(&self) -> bool {
        self.surface.closed()
    }
}
