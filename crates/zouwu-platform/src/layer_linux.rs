//! Linux/Wayland 版「分层窗口」：`zwlr_layer_shell_v1` 的 overlay 层 + `wl_shm` 逐像素 alpha。
//!
//! 和 Windows 那条 `UpdateLayeredWindow` 路径是同构的：
//!
//! * 逐像素 alpha（预乘 ARGB8888），真正的"镂空"窗口；
//! * alpha≈0 的像素通过 `set_input_region` 让鼠标穿透，桌面图标在气泡外照常可点；
//! * 一帧 = 一次 `attach + damage + commit`，小面积下 < 1ms，空闲 0 开销
//!   （只在像素真的变了的时候才提交，见 `present` 的 seq 判断）。
//!
//! 为什么非要用 layer-shell，而不是普通 `xdg_toplevel`：Wayland 的 toplevel 协议
//! **不给客户端**置顶、绝对定位、点击穿透这三件事。桌面挂件全靠这三条活着。
//! KWin 6.x 实现了 layer-shell（面板、壁纸都在用），所以走这条路。
//!
//! 线程模型：winit 的事件循环在 UI 线程上跑（面板窗口都在那儿），挂件自己
//! 独占一条 Wayland 连接和一个事件队列（`event_queue.blocking_dispatch`）。
//! 两者之间只有三条窄通道：
//!
//! 1. UI → 挂件：`Cmd` 队列 + 一次 `wl_display.sync` 把对端叫醒（不用 calloop，
//!    省一个依赖；sync 的回包保证 `blocking_dispatch` 一定会返回）；
//! 2. 挂件 → UI：`PetInput` 队列 + 一个 `waker` 回调（UI 侧塞的 `EventLoopProxy`）；
//! 3. 共享状态：几何 / 光标 / 按键（UI 侧轮询，语义和 Windows 的 `GetCursorPos` 一致）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_callback, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{Event as FractionalScaleEvent, WpFractionalScaleV1},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

/// 一个指针事件。坐标是**全局物理像素**（和 Windows 的 `GetCursorPos` 同域）。
#[derive(Clone, Copy, Debug)]
pub enum PetInput {
    Motion { x: i32, y: i32 },
    Leave,
    Press(PointerButton),
    Release(PointerButton),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
    Other,
}

fn map_button(button: u32) -> PointerButton {
    // linux/input-event-codes.h：BTN_LEFT=0x110, BTN_RIGHT=0x111, BTN_MIDDLE=0x112
    match button {
        0x110 => PointerButton::Left,
        0x111 => PointerButton::Right,
        0x112 => PointerButton::Middle,
        _ => PointerButton::Other,
    }
}

/// alpha 高于这个值的像素才接收鼠标。和 Windows 那边"alpha=0 天然穿透"对齐。
const INPUT_ALPHA: u8 = 8;
/// 输入区域最多发这么多矩形：多到没意义时宁可整块都可点（否则每帧几百个请求）。
///
/// 高 DPI 下同一只小兽会占 2 倍边长 = 4 倍像素，轮廓能切出的矩形数也跟着涨，
/// 96 条太容易越界（一越界就退化成整块，四角会开始吃掉桌面图标的点击）。
/// 抬高到 256 条，配合下面的"按缩放对齐"量化，稳定落在几十条。
const MAX_INPUT_RECTS: usize = 256;

enum Cmd {
    /// 尺寸变了（物理像素）。
    Resize(u32, u32),
    /// 挪到全局物理坐标。
    Move(i32, i32),
    /// 帧序号变了，去 `frame` 里取像素。
    Frame,
    Visible(bool),
    Quit,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
/// 拖拽/摆放时至少要留在屏上的像素数。窗口可以骑在两块屏中间，但不能整只
/// 掉出去——掉出去就再也点不到了。
const KEEP_ON_SCREEN: i32 = 56;

/// UI 线程和 Wayland 线程共享的那点状态。
struct Shared {
    id: isize,
    /// 物理像素尺寸（= 逻辑尺寸 × 当前 output 的缩放，由表面自己推导）。
    size: Mutex<(u32, u32)>,
    /// **逻辑**尺寸：UI 真正想要的那个大小。换到另一块缩放不同的屏上时，
    /// 物理缓冲跟着缩放变，用户看到的"大小"不变。
    logical: Mutex<(u32, u32)>,
    /// 全局物理坐标左上角。
    pos: Mutex<(i32, i32)>,
    /// 表面所在的缩放系数（buffer scale，分数缩放时是 1.15 这种值）。
    scale: Mutex<f64>,
    /// **坐标空间**的缩放（整数，= output 的 scale_factor）。
    monitor_scale: Mutex<i32>,
    /// 最近一帧预乘 RGBA（物理像素）。
    frame: Mutex<Vec<u8>>,
    frame_seq: AtomicU64,
    drawn_seq: AtomicU64,
    visible: AtomicBool,
    /// 光标在本体上的最后位置（全局物理），离开本体后是 None。
    cursor: Mutex<Option<(i32, i32)>>,
    /// (左键, 右键)
    buttons: Mutex<(bool, bool)>,
    input: Mutex<VecDeque<PetInput>>,
    cmds: Mutex<VecDeque<Cmd>>,
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// 直接戳 Wayland 线程的回调（`wl_display.sync` + flush）。
    ///
    /// `waker` 只叫醒 **UI** 线程。隐藏时 UI 没有帧要画、也就不会调 `present()`，
    /// 于是 `Cmd::Visible(false)` 会一直躺在 `cmds` 里，表面停在不透明的上一帧——
    /// 用户看到的就是「日志说 hidden，小兽还在屏幕上」。所以凡是命令队列自己
    /// 就能触发动作的调用，必须顺手把 Wayland 线程戳醒。
    poke: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// 显示器几何（全局物理）：(x, y, w, h, scale)
    monitors: Mutex<Vec<(i32, i32, i32, i32, f64)>>,
    /// 当前所在屏的刷新率（毫赫兹）。UI 用它定帧节拍——按 60fps 的节拍去画
    /// 一块 75Hz 的屏，会周期性地"多显示一帧"，看上去就是抖动。
    refresh_mhz: Mutex<u32>,
    /// 被合成器关掉（比如 output 移除）时置位，UI 侧据此走"重建"。
    closed: AtomicBool,
    /// 表面重建次数（换屏/换缩放）。用于盯"跨屏拖动别在缝上反复重建"。
    rebuilds: AtomicU64,
}

impl Shared {
    fn wake(&self) {
        let waker = lock(&self.waker).clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    fn push_cmd(&self, cmd: Cmd) {
        lock(&self.cmds).push_back(cmd);
    }

    /// 叫醒 Wayland 线程去处理命令队列（不等下一次 `present()`）。
    fn poke(&self) {
        let poke = lock(&self.poke).clone();
        if let Some(poke) = poke {
            poke();
        }
    }

    fn push_input(&self, ev: PetInput) {
        // 高频 Motion 只留最后一条：轮询兜底读的是 `cursor`，中间帧没有意义。
        {
            let mut q = lock(&self.input);
            if let PetInput::Motion { .. } = ev {
                q.retain(|e| !matches!(e, PetInput::Motion { .. }));
            }
            q.push_back(ev);
            if q.len() > 64 {
                q.pop_front();
            }
        }
        self.wake();
    }
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
static REGISTRY: OnceLock<Mutex<Vec<Weak<Shared>>>> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<Weak<Shared>>> {
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

fn lookup(id: isize) -> Option<Arc<Shared>> {
    let mut reg = lock(registry());
    reg.retain(|w| w.strong_count() > 0);
    reg.iter().find_map(|w| {
        let shared = w.upgrade()?;
        (shared.id == id).then_some(shared)
    })
}

/// 一个 layer-shell 表面：桌面挂件的"分层窗口"本体。
pub struct PetSurface {
    shared: Arc<Shared>,
    /// 叫醒 Wayland 线程用：把 sync 请求写进同一条连接。
    conn: Connection,
    display: wayland_client::protocol::wl_display::WlDisplay,
    qh: QueueHandle<Widget>,
}

impl PetSurface {
    /// 建表面。`pos` 是全局坐标（可以落在任何一块屏上）。
    ///
    /// **`width`/`height` 是逻辑像素**，不是物理像素：真实缓冲区尺寸由
    /// `create_surface()` 拿这块屏的缩放乘出来。这里先存一份逻辑值，
    /// `size` 只是个占位，拿到 output 之后立刻被重算。
    /// （两个调用方都曾经把物理尺寸传进来，于是每次重建都再乘一遍缩放——
    /// 680→1360→2720→5440 这样翻倍，最后整只飞出屏幕。）
    ///
    /// 同步等待第一次 configure：返回时尺寸/缩放已经确定，UI 侧可以立刻分配画布。
    pub fn create(width: u32, height: u32, pos: (i32, i32)) -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("wayland connect: {e}"))?;
        let (globals, mut event_queue) =
            registry_queue_init::<Widget>(&conn).map_err(|e| format!("wayland registry: {e}"))?;
        let qh = event_queue.handle();

        let shared = Arc::new(Shared {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed) as isize,
            size: Mutex::new((width.max(1), height.max(1))),
            logical: Mutex::new((width.max(1), height.max(1))),
            pos: Mutex::new(pos),
            scale: Mutex::new(1.0),
            monitor_scale: Mutex::new(1),
            frame: Mutex::new(Vec::new()),
            frame_seq: AtomicU64::new(0),
            drawn_seq: AtomicU64::new(0),
            visible: AtomicBool::new(true),
            cursor: Mutex::new(None),
            buttons: Mutex::new((false, false)),
            input: Mutex::new(VecDeque::new()),
            cmds: Mutex::new(VecDeque::new()),
            waker: Mutex::new(None),
            poke: Mutex::new(None),
            monitors: Mutex::new(Vec::new()),
            refresh_mhz: Mutex::new(0),
            closed: AtomicBool::new(false),
            rebuilds: AtomicU64::new(0),
        });

        let mut widget = Widget::new(&globals, &qh, shared.clone())?;
        // 先把 registry 走完（拿到 output/seat），再建表面。
        event_queue
            .roundtrip(&mut widget)
            .map_err(|e| format!("wayland roundtrip: {e}"))?;
        widget.create_surface(&qh, None)?;
        // 等第一个 configure：layer surface 必须先提交一次空 buffer 才会被配置。
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while !widget.configured {
            if std::time::Instant::now() > deadline {
                return Err("layer surface did not get a configure".into());
            }
            event_queue
                .blocking_dispatch(&mut widget)
                .map_err(|e| format!("wayland dispatch: {e}"))?;
        }
        widget.publish_monitors();

        let display = conn.display();
        {
            let display = display.clone();
            let conn = conn.clone();
            let qh = qh.clone();
            *lock(&shared.poke) = Some(Arc::new(move || {
                display.sync(&qh, ());
                let _ = conn.flush();
            }));
        }
        let handle = Self {
            shared: shared.clone(),
            conn,
            display,
            qh: qh.clone(),
        };
        lock(registry()).push(Arc::downgrade(&shared));

        std::thread::Builder::new()
            .name("zouwu-wayland".into())
            .spawn(move || {
                let mut widget = widget;
                let mut queue = event_queue;
                if let Err(err) = widget.run(&mut queue) {
                    log::error!("wayland thread: {err}");
                }
                shared.closed.store(true, Ordering::Relaxed);
                shared.wake();
            })
            .map_err(|e| format!("spawn wayland thread: {e}"))?;

        Ok(handle)
    }

    pub fn id(&self) -> isize {
        self.shared.id
    }

    /// 表面所在 output 的缩放系数（缓冲像素/逻辑像素，分数缩放时是 1.15 这种值）。
    pub fn scale(&self) -> f64 {
        *lock(&self.shared.scale)
    }

    /// **坐标空间**的缩放（整数）。`monitors()`/光标/位置的换算全用这一套，
    /// 和分数缩放无关——两套混用会让拖动和命中判定各自活在不同坐标系里。
    pub fn monitor_scale(&self) -> i32 {
        *lock(&self.shared.monitor_scale)
    }

    pub fn size(&self) -> (u32, u32) {
        *lock(&self.shared.size)
    }

    pub fn pos(&self) -> (i32, i32) {
        *lock(&self.shared.pos)
    }

    /// UI 线程插进来的"有事叫你"回调（通常是 `EventLoopProxy::send_event`）。
    pub fn set_waker(&self, waker: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.shared.waker) = Some(waker);
    }

    fn send(&self) {
        // 一次 sync 保证 Wayland 线程的 `blocking_dispatch` 一定会返回。
        self.display.sync(&self.qh, ());
        let _ = self.conn.flush();
    }

    pub fn resize(&self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        {
            let mut size = lock(&self.shared.size);
            if *size == (width, height) {
                return;
            }
            *size = (width, height);
            // 尺寸是物理像素，换算回逻辑单位存起来：以后换屏（缩放变了）
            // 就靠它重新推物理尺寸。
            let scale = lock(&self.shared.scale).max(1.0);
            *lock(&self.shared.logical) = (
                (width as f64 / scale).round().max(1.0) as u32,
                (height as f64 / scale).round().max(1.0) as u32,
            );
        }
        self.shared.push_cmd(Cmd::Resize(width, height));
        self.send();
    }

    pub fn set_position(&self, x: i32, y: i32) {
        {
            let mut pos = lock(&self.shared.pos);
            if *pos == (x, y) {
                return;
            }
            *pos = (x, y);
        }
        log::trace!("wayland set_position: {x},{y} logical");
        self.shared.push_cmd(Cmd::Move(x, y));
        self.send();
    }

    pub fn set_visible(&self, visible: bool) {
        if self.shared.visible.swap(visible, Ordering::Relaxed) == visible {
            return;
        }
        self.shared.push_cmd(Cmd::Visible(visible));
        self.send();
    }

    pub fn visible(&self) -> bool {
        self.shared.visible.load(Ordering::Relaxed)
    }

    /// 提交一帧（预乘 RGBA8，物理像素，尺寸等于 [`Self::size`]）。
    pub fn present(&self, rgba: &[u8]) -> Result<(), String> {
        let (w, h) = self.size();
        let need = (w as usize) * (h as usize) * 4;
        if rgba.len() < need {
            return Err(format!(
                "frame too small: {} < {need} ({w}x{h})",
                rgba.len()
            ));
        }
        {
            let mut frame = lock(&self.shared.frame);
            frame.clear();
            frame.extend_from_slice(&rgba[..need]);
        }
        self.shared.frame_seq.fetch_add(1, Ordering::Relaxed);
        self.shared.push_cmd(Cmd::Frame);
        self.send();
        Ok(())
    }

    pub fn take_input(&self) -> Vec<PetInput> {
        let mut q = lock(&self.shared.input);
        q.drain(..).collect()
    }

    /// 全局物理光标位置；光标不在本体上时是 None。
    pub fn cursor_position(&self) -> Option<(i32, i32)> {
        *lock(&self.shared.cursor)
    }

    pub fn buttons(&self) -> (bool, bool) {
        *lock(&self.shared.buttons)
    }

    /// 显示器几何（全局物理像素 + 缩放）。
    pub fn monitors(&self) -> Vec<(i32, i32, i32, i32, f64)> {
        lock(&self.shared.monitors).clone()
    }

    /// 当前所在屏的刷新率（Hz）。拿不到返回 None（UI 退回 60fps）。
    pub fn refresh_hz(&self) -> Option<f32> {
        let mhz = *lock(&self.shared.refresh_mhz);
        match mhz {
            0 => None,
            // 协议里是毫赫兹；有的合成器给的是整数 Hz，两种都认。
            mhz if mhz > 1000 => Some(mhz as f32 / 1000.0),
            hz => Some(hz as f32),
        }
    }

    pub fn closed(&self) -> bool {
        self.shared.closed.load(Ordering::Relaxed)
    }

    /// 表面重建了几次（换屏/换缩放各算一次）。
    pub fn rebuilds(&self) -> u64 {
        self.shared.rebuilds.load(Ordering::Relaxed)
    }
}

impl Drop for PetSurface {
    fn drop(&mut self) {
        self.shared.push_cmd(Cmd::Quit);
        self.send();
    }
}

// ---------------------------------------------------------------- Wayland 线程

struct Widget {
    registry_state: RegistryState,
    compositor: CompositorState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    layer_shell: LayerShell,
    surface: Option<wl_surface::WlSurface>,
    layer: Option<LayerSurface>,
    /// 当前挂在哪块屏上（layer surface 的 output 参数，创建后不能改）。
    attached_output: Option<wl_output::WlOutput>,
    pool: Option<SlotPool>,
    pointer: Option<wl_pointer::WlPointer>,
    qh: QueueHandle<Widget>,
    /// 表面所在 output 的**整数**缩放。这是「坐标空间」的尺度：`monitors()`、
    /// 光标全局坐标、`set_position` 全在这一套里，和分数缩放无关。
    output_scale: i32,
    /// **缓冲像素 / 逻辑像素**。有 `wp_fractional_scale_v1` 时它是 1.15 这种值，
    /// 没的话就等于 `output_scale`。只有拼 buffer、输入区域、命中网格用它。
    scale: f64,
    /// 分数缩放的两个全局对象（合成器不支持就是 None，退回整数 buffer scale）。
    fractional_mgr: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    /// 当前表面上的分数缩放对象和 viewport（随表面一起重建）。
    fractional: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    logical_size: (u32, u32),
    configured: bool,
    /// 已经提交过多少帧（只用于日志：出问题时要能一眼看出"到底画没画"）。
    frames: u32,
    /// 上一次发出去的输入区域（用来跳过多余的重发）。
    input_rects: Vec<(i32, i32, i32, i32)>,
    shared: Arc<Shared>,
}

impl Widget {
    fn new(
        globals: &wayland_client::globals::GlobalList,
        qh: &QueueHandle<Self>,
        shared: Arc<Shared>,
    ) -> Result<Self, String> {
        let compositor =
            CompositorState::bind(globals, qh).map_err(|e| format!("wl_compositor: {e}"))?;
        let layer_shell = LayerShell::bind(globals, qh)
            .map_err(|e| format!("zwlr_layer_shell_v1 (需要 wlroots/KWin 的 layer-shell): {e}"))?;
        let shm = Shm::bind(globals, qh).map_err(|e| format!("wl_shm: {e}"))?;
        // 两个都是可选的：拿不到就走传统的整数 buffer scale。
        let fractional_mgr = globals
            .bind::<WpFractionalScaleManagerV1, _, _>(qh, 1..=1, ())
            .ok();
        let viewporter = globals.bind::<WpViewporter, _, _>(qh, 1..=1, ()).ok();
        if fractional_mgr.is_none() || viewporter.is_none() {
            log::debug!(
                "wayland: no fractional scale (mgr={}, vp={}), using integer buffer scale",
                fractional_mgr.is_some(),
                viewporter.is_some()
            );
        }
        Ok(Self {
            registry_state: RegistryState::new(globals),
            seat_state: SeatState::new(globals, qh),
            output_state: OutputState::new(globals, qh),
            compositor,
            shm,
            layer_shell,
            surface: None,
            layer: None,
            attached_output: None,
            pool: None,
            pointer: None,
            qh: qh.clone(),
            output_scale: 1,
            scale: 1.0,
            fractional_mgr,
            viewporter,
            fractional: None,
            viewport: None,
            logical_size: (1, 1),
            configured: false,
            frames: 0,
            input_rects: Vec::new(),
            shared,
        })
    }

    fn publish_monitors(&self) {
        let mut rects = Vec::new();
        for output in self.output_state.outputs() {
            let Some(info) = self.output_state.info(&output) else {
                continue;
            };
            let scale = info.scale_factor.max(1) as f64;
            let location = info.logical_position.unwrap_or(info.location);
            let Some(logical) = info.logical_size else {
                continue;
            };
            // sctk 给的是逻辑尺寸；UI 侧一律用物理像素，这里乘回去。
            rects.push((
                (location.0 as f64 * scale) as i32,
                (location.1 as f64 * scale) as i32,
                (logical.0 as f64 * scale) as i32,
                (logical.1 as f64 * scale) as i32,
                scale,
            ));
        }
        rects.sort_by_key(|r| (r.1, r.0));
        *lock(&self.shared.monitors) = rects;
    }

    /// 某点落在哪块屏上，返回 `(output, 缩放, 屏幕原点)`。
    ///
    /// **坐标一律是逻辑坐标**（surface-local 的那套）。这个函数以前拿逻辑坐标
    /// 去比物理矩形，于是在 4K@2x 的副屏上既选错了屏、又把 margin 算成负数，
    /// 结果就是"表面建成功了、帧也提交了，可屏幕上什么都没有"。
    /// 物理像素只在跟 UI 层打交道时（`monitors`/`PetWindow`）才用。
    /// 目标位置落在哪块屏。`pos` 是**逻辑**坐标（和 margin 同域）。
    fn output_for(&self, pos: (i32, i32)) -> Option<(wl_output::WlOutput, i32, (i32, i32))> {
        let mut fallback = None;
        for output in self.output_state.outputs() {
            let Some(info) = self.output_state.info(&output) else {
                continue;
            };
            let scale = info.scale_factor.max(1);
            let origin = info.logical_position.unwrap_or(info.location);
            let Some(logical) = info.logical_size else {
                fallback.get_or_insert((output, scale, origin));
                continue;
            };
            let (x, y) = origin;
            let (w, h) = logical;
            if pos.0 >= x && pos.1 >= y && pos.0 < x + w && pos.1 < y + h {
                return Some((output, scale, origin));
            }
            fallback.get_or_insert((output, scale, origin));
        }
        fallback
    }

    /// 光标（全局物理坐标，见 `global_physical`）落在哪块屏。
    ///
    /// 拖动跨屏时**必须以光标所在屏为准**：layer 表面属于哪块屏是创建时定死的，
    /// 而位置只能夹进这块屏。若按“窗口左上角所在屏”选，只要左上角还在旧屏上，
    /// 位置就会被夹在旧屏右边、永远过不去（两块屏之间有 340px 的死区，拖到那里
    /// 就“卡住/弹回去”）。
    fn output_at_physical(
        &self,
        cursor: (i32, i32),
    ) -> Option<(wl_output::WlOutput, i32, (i32, i32))> {
        for output in self.output_state.outputs() {
            let Some(info) = self.output_state.info(&output) else {
                continue;
            };
            let scale = info.scale_factor.max(1);
            let origin = info.logical_position.unwrap_or(info.location);
            let (w, h) = info.logical_size.unwrap_or((0, 0));
            // 每块屏各自换算：两块屏缩放不同时用同一个 scale 会判错。
            let (px, py) = (origin.0 * scale, origin.1 * scale);
            let (pw, ph) = (w * scale, h * scale);
            if cursor.0 >= px && cursor.1 >= py && cursor.0 < px + pw && cursor.1 < py + ph {
                return Some((output, scale, origin));
            }
        }
        None
    }

    /// 建表面。`wanted` 是调用方按位置决定好的目标屏（`None` = 按左上角自己挑）。
    ///
    /// 必须能指定：跨屏那一步是按**重叠面积**挑屏的，此时左上角还在旧屏上。如果这里
    /// 又按左上角自己挑一次，就会和调用方的决定打架——缝上每一步都重建一次表面
    /// （实测冲刺 150 步里 24 次），拖过去就是一卡一卡。
    fn create_surface(
        &mut self,
        qh: &QueueHandle<Self>,
        wanted: Option<wl_output::WlOutput>,
    ) -> Result<(), String> {
        let pos = *lock(&self.shared.pos);
        let (output, scale, _origin) = wanted
            .and_then(|o| {
                let info = self.output_state.info(&o)?;
                Some((
                    Some(o),
                    info.scale_factor.max(1),
                    info.logical_position.unwrap_or(info.location),
                ))
            })
            .or_else(|| {
                self.output_for(pos)
                    .map(|(o, s, origin)| (Some(o), s, origin))
            })
            .unwrap_or((None, 1, (0, 0)));
        self.output_scale = scale;
        *lock(&self.shared.monitor_scale) = scale;
        // buffer 缩放：整数 `wl_output.scale` 在 1.15/1.25 的屏上只是向上取整（2），
        // 拿它建出来的 buffer 会大一截，等 `preferred_scale` 到了再缩回去——那一帧的
        // `present` 会因尺寸不符被丢掉，跨屏拖动时看着就是缝上卡一下。
        //
        // 真值其实当场就有：physical/logical 就是缩放比例（2560/2226 = 1.15），
        // 按协议的单位（1/120）取整到最近一档，和 `preferred_scale` 报的完全一致。
        let hinted = self
            .fractional_mgr
            .is_some()
            .then(|| output.as_ref().and_then(|o| self.output_state.info(o)))
            .flatten()
            .and_then(|info| {
                let (pw, _) = info.physical_size;
                let (lw, _) = info.logical_size?;
                snap_scale(pw, lw)
            });
        self.scale = hinted.unwrap_or(scale as f64);
        *lock(&self.shared.scale) = self.scale;
        // 记下这块屏的刷新率（UI 用定帧节拍，见 `Shared::refresh_mhz`）。
        if let Some(info) = output.as_ref().and_then(|o| self.output_state.info(o)) {
            // `modes` 是所有已知模式，当前生效的那个带 `current` 标记。
            let mode = info
                .modes
                .iter()
                .find(|m| m.current)
                .or_else(|| info.modes.first());
            if let Some(mode) = mode {
                *lock(&self.shared.refresh_mhz) = mode.refresh_rate.max(0) as u32;
                log::debug!(
                    "wayland: output {} refresh {} mHz ({}x{})",
                    info.name.as_deref().unwrap_or("?"),
                    mode.refresh_rate,
                    mode.dimensions.0,
                    mode.dimensions.1
                );
            }
        }
        // 物理缓冲区由**逻辑尺寸 × 缩放**推出（不是反过来）：挂件换到 2x 屏上时
        // 应该是"同样大、像素更细"，而不是"看起来小了一半"。
        let (lw, lh) = *lock(&self.shared.logical);
        let (w, h) = (
            ((lw as f64 * self.scale).round() as u32).max(1),
            ((lh as f64 * self.scale).round() as u32).max(1),
        );
        *lock(&self.shared.size) = (w, h);

        // 旧表面的协议对象先扔掉（重建时），它们随旧 `wl_surface` 一起失效。
        self.fractional = None;
        self.viewport = None;
        let surface = self.compositor.create_surface(qh);
        // 有 `wp_fractional_scale_v1` 时 buffer scale **必须** 是 1：真缩放由
        // preferred_scale 给，缓冲区到逻辑尺寸的映射交给 viewport。以前固定用
        // 整数缩放，KWin 在 1.15 的屏上只能报 2（向上取整），680 物理像素被当作
        // 680/1.15 逻辑显示——小兽平白大了 74%。
        if self.fractional_mgr.is_some() && self.viewporter.is_some() {
            surface.set_buffer_scale(1);
            if let Some(mgr) = self.fractional_mgr.clone() {
                self.fractional = Some(mgr.get_fractional_scale(&surface, qh, ()));
            }
            if let Some(vp) = self.viewporter.clone() {
                self.viewport = Some(vp.get_viewport(&surface, qh, ()));
            }
        } else {
            surface.set_buffer_scale(scale);
        }
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface.clone(),
            Layer::Overlay,
            Some("zouwu"),
            output.as_ref(),
        );
        layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        // -1 = 不独占桌面空间（overlay 挂在所有窗口之上，但不挤走别人）。
        layer.set_exclusive_zone(-1);
        self.logical_size = (lw.max(1), lh.max(1));
        layer.set_size(self.logical_size.0, self.logical_size.1);
        // **先记下这是哪块屏**，再算 margin：`apply_margins` 要用这块屏的原点做
        // 参照，否则会拿上一块屏（或 (0,0)）去减，把位置算到别处。
        self.attached_output = output;
        self.apply_margins(&layer);
        // viewport 的 destination 是**表面本地坐标**（= 逻辑尺寸）：
        // 告诉合成器"这块 buffer 请铺成这么大"，剩下的伸缩交给它。
        self.viewport_destination();
        // 第一次提交必须是空 buffer：等合成器发 configure。
        layer.commit();

        self.surface = Some(surface);
        self.layer = Some(layer);
        self.pool = Some(
            SlotPool::new((w.max(1) * h.max(1) * 4) as usize, &self.shm)
                .map_err(|e| format!("slot pool: {e}"))?,
        );
        Ok(())
    }

    /// 告诉合成器"这块 buffer 铺成逻辑尺寸那么大"（没 viewport 时什么都不做）。
    fn viewport_destination(&self) {
        if let Some(vp) = self.viewport.as_ref() {
            let (lw, lh) = self.logical_size;
            vp.set_destination(lw.max(1) as i32, lh.max(1) as i32);
        }
    }

    /// 按当前 `scale` 把逻辑尺寸换算成 buffer 像素，推给 UI，然后叫它重画。
    ///
    /// 分数缩放是**异步到达**的（`preferred_scale` 事件），所以尺寸可能在表面已经
    /// 显示出来之后才变：这里必须把新尺寸存进 `shared.size` 并叫醒 UI，否则它的
    /// 画布会一直停在旧尺寸（老帧和新 buffer 对不上，画出来是花的）。
    fn sync_size(&mut self) {
        let (lw, lh) = self.logical_size;
        let (w, h) = (
            ((lw as f64 * self.scale).round() as u32).max(1),
            ((lh as f64 * self.scale).round() as u32).max(1),
        );
        let changed = {
            let mut size = lock(&self.shared.size);
            let changed = *size != (w, h);
            *size = (w, h);
            changed
        };
        *lock(&self.shared.scale) = self.scale;
        self.viewport_destination();
        if changed {
            log::debug!(
                "wayland: buffer geometry -> {w}x{h} px (logical {lw}x{lh}, scale {})",
                self.scale
            );
            // 新尺寸 = 新画布：让 UI 重新分配像素图并重画。
            self.shared.drawn_seq.store(u64::MAX, Ordering::Relaxed);
            self.shared.wake();
        }
    }

    /// margin = 目标位置 − 屏幕原点（逻辑像素，锚点 TOP|LEFT）。
    /// margin 是**相对屏幕原点的逻辑距离**，而 `shared.pos` 也是逻辑坐标：
    /// 两边同一个单位才不用除缩放。除的话会把 2x 屏上的 margin 算成一半，
    /// 表面就被摆到屏幕中线附近（甚至负 margin 直接推出画面）。
    fn apply_margins(&mut self, layer: &LayerSurface) {
        let pos = *lock(&self.shared.pos);
        let (ox, oy) = self
            .attached_output
            .as_ref()
            .and_then(|o| self.output_state.info(o))
            .map(|info| info.logical_position.unwrap_or(info.location))
            .unwrap_or((0, 0));
        let (ow, oh) = self.attached_bounds();
        let (lw, lh) = (self.logical_size.0 as i32, self.logical_size.1 as i32);

        // 锚点跟着位置走：位置在屏内就用左/上，越到屏外就翻到右/下。
        //
        // layer-shell 的 margin 只是"离锚边的距离"，负值 KWin 会判协议错误
        // （整条连接被断——就是"拖一下小兽彻底不见"那条）。翻锚点之后 margin
        // 恒为非负，于是窗口可以像普通窗口那样骑在两块屏之间，跨屏拖动时位置
        // 能连续跟着光标走，不用"卡在屏幕边上等光标越过去再跳一下"。
        let anchor_right = pos.0 < ox;
        let anchor_bottom = pos.1 < oy;
        let left = if anchor_right { 0 } else { (pos.0 - ox).max(0) };
        let right = if anchor_right {
            (ox + ow - (pos.0 + lw)).max(0)
        } else {
            0
        };
        let top = if anchor_bottom {
            0
        } else {
            (pos.1 - oy).max(0)
        };
        let bottom = if anchor_bottom {
            (oy + oh - (pos.1 + lh)).max(0)
        } else {
            0
        };
        let anchor = if anchor_right {
            Anchor::RIGHT
        } else {
            Anchor::LEFT
        } | if anchor_bottom {
            Anchor::BOTTOM
        } else {
            Anchor::TOP
        };
        layer.set_anchor(anchor);
        layer.set_margin(top, right, bottom, left);
        log::trace!(
            "wayland margin: pos {pos:?} origin {ox},{oy} anchor {:?} -> t{top} r{right} b{bottom} l{left}",
            anchor
        );
    }

    /// 表面所在这块屏的逻辑尺寸（`logical_size` 缺失时退到 `location`/物理尺寸）。
    fn attached_bounds(&self) -> (i32, i32) {
        self.attached_output
            .as_ref()
            .and_then(|o| self.output_state.info(o))
            .map(|info| {
                let scale = info.scale_factor.max(1);
                info.logical_size
                    .unwrap_or((info.physical_size.0 / scale, info.physical_size.1 / scale))
            })
            .unwrap_or((0, 0))
    }

    /// 把目标位置夹成"至少 `KEEP_ON_SCREEN` 还在被选中的那块屏上"，并返回该屏。
    ///
    /// 选屏顺序：
    /// 1. **拖拽中**按光标所在屏——光标才是"我想让它去哪"的唯一信号；
    /// 2. 否则按重叠面积最大的一块，让停下来的窗口留在它看起来"主要在那块屏"上，
    ///    而不是按左上角一刀切（骑在缝上时会莫名其妙跳回旧屏）；
    /// 3. 兜底：按左上角所在屏。
    fn fit_on_output(
        &self,
        pos: (i32, i32),
    ) -> ((i32, i32), Option<(wl_output::WlOutput, i32, (i32, i32))>) {
        let dragging = lock(&self.shared.buttons).0;
        let attached = self.attached_output.as_ref().and_then(|o| {
            self.output_state.info(o).map(|info| {
                (
                    o.clone(),
                    info.scale_factor.max(1),
                    info.logical_position.unwrap_or(info.location),
                )
            })
        });
        let chosen = if dragging {
            let by_cursor = lock(&self.shared.cursor).and_then(|c| self.output_at_physical(c));
            match (by_cursor, attached) {
                // 光标已经跨过去了，就按**新屏占了多少**决定换不换：一半以上就换。
                // 以前要等新屏占到 60% 才换，于是小兽被缝裁掉大半才"哔"地跳过去。
                // 而更早的写法（单向判断）会在缝上每来一个 motion 事件就翻一次，
                // 每次翻都得重建表面 = 一帧空白，手上一抖就能看出来。
                (Some(other), Some(att))
                    if other.0 != att.0 && !self.overlap_at_least(&other.0, pos, 50) =>
                {
                    Some(att)
                }
                (Some(other), _) => Some(other),
                (None, att) => att,
            }
        } else {
            // 停下来的选择：按重叠面积，其次才按左上角在哪。
            self.output_most_overlapping(pos)
                .or(attached)
                .or_else(|| self.output_for(pos))
        };
        let Some((output, scale, origin)) = chosen else {
            return (pos, None);
        };
        let (bounds_w, bounds_h) = self
            .output_state
            .info(&output)
            .map(|info| {
                let s = info.scale_factor.max(1);
                info.logical_size
                    .unwrap_or((info.physical_size.0 / s, info.physical_size.1 / s))
            })
            .unwrap_or((0, 0));
        let x = keep_visible(pos.0, origin.0, bounds_w as i32, self.logical_size.0 as i32);
        let y = keep_visible(pos.1, origin.1, bounds_h as i32, self.logical_size.1 as i32);
        ((x, y), Some((output, scale, origin)))
    }

    /// 与窗口重叠面积最大的那块屏（完全不在任何屏上时返回 `None`）。
    fn output_most_overlapping(
        &self,
        pos: (i32, i32),
    ) -> Option<(wl_output::WlOutput, i32, (i32, i32))> {
        let (lw, lh) = (self.logical_size.0 as i32, self.logical_size.1 as i32);
        let mut best: Option<(i64, wl_output::WlOutput, i32, (i32, i32))> = None;
        for output in self.output_state.outputs() {
            let Some(info) = self.output_state.info(&output) else {
                continue;
            };
            let scale = info.scale_factor.max(1);
            let origin = info.logical_position.unwrap_or(info.location);
            let (w, h) = info
                .logical_size
                .unwrap_or((info.physical_size.0 / scale, info.physical_size.1 / scale));
            let x0 = origin.0.max(pos.0);
            let y0 = origin.1.max(pos.1);
            let x1 = (origin.0 + w as i32).min(pos.0 + lw);
            let y1 = (origin.1 + h as i32).min(pos.1 + lh);
            let area = i64::from((x1 - x0).max(0)) * i64::from((y1 - y0).max(0));
            if area > 0 && best.as_ref().is_none_or(|(a, ..)| area > *a) {
                best = Some((area, output, scale, origin));
            }
        }
        best.map(|(_, output, scale, origin)| (output, scale, origin))
    }

    /// 窗口与某块屏的重叠面积（逻辑像素²）。
    fn overlap_area(&self, output: &wl_output::WlOutput, pos: (i32, i32)) -> i64 {
        let Some(info) = self.output_state.info(output) else {
            return 0;
        };
        let origin = info.logical_position.unwrap_or(info.location);
        let scale = info.scale_factor.max(1);
        let (w, h) = info
            .logical_size
            .unwrap_or((info.physical_size.0 / scale, info.physical_size.1 / scale));
        let (lw, lh) = (self.logical_size.0 as i32, self.logical_size.1 as i32);
        let x0 = origin.0.max(pos.0);
        let y0 = origin.1.max(pos.1);
        let x1 = (origin.0 + w as i32).min(pos.0 + lw);
        let y1 = (origin.1 + h as i32).min(pos.1 + lh);
        i64::from((x1 - x0).max(0)) * i64::from((y1 - y0).max(0))
    }

    /// 窗口是否有至少 `pct`% 落在这块屏上。
    fn overlap_at_least(&self, output: &wl_output::WlOutput, pos: (i32, i32), pct: i64) -> bool {
        let area = i64::from(self.logical_size.0) * i64::from(self.logical_size.1);
        area > 0 && self.overlap_area(output, pos) * 100 >= area * pct
    }

    fn run(&mut self, queue: &mut EventQueue<Self>) -> Result<(), String> {
        loop {
            // 命令队列：UI 线程每塞一条就 `wl_display.sync` 一次，所以这里一定会醒。
            let cmds: Vec<Cmd> = {
                let mut q = lock(&self.shared.cmds);
                q.drain(..).collect()
            };
            for cmd in cmds {
                match cmd {
                    Cmd::Frame => {}
                    Cmd::Resize(w, h) => {
                        self.logical_size = (
                            (w as f64 / self.scale).round().max(1.0) as u32,
                            (h as f64 / self.scale).round().max(1.0) as u32,
                        );
                        if let Some(layer) = self.layer.clone() {
                            layer.set_size(self.logical_size.0.max(1), self.logical_size.1.max(1));
                            layer.commit();
                        }
                        // viewport 的 destination 跟着变；buffer 尺寸由逻辑反推（可能差 1 像素，
                        // 但边界一致，UI 收到新尺寸后会重建画布）。
                        self.sync_size();
                    }
                    Cmd::Move(..) => {
                        let raw = *lock(&self.shared.pos);
                        // 先把位置夹进一块真实存在的屏：越界的 margin 会让
                        // 合成器直接断连接（见 `apply_margins`）。
                        let (pos, target) = self.fit_on_output(raw);
                        if pos != raw {
                            *lock(&self.shared.pos) = pos;
                            log::debug!("wayland: clamped off-screen move {raw:?} -> {pos:?}");
                        }
                        let (want_output, want_scale) = match target {
                            Some((output, scale, _)) => (Some(output), scale),
                            None => (None, self.output_scale),
                        };
                        // 跨屏也得重建：layer surface 的 output 定了就不能改，
                        // 而 margin 只在这块屏内有效。以前只比缩放，两块屏同为 2x
                        // 时表面会留在旧屏上、带着一个越界的 margin。
                        if want_output != self.attached_output || want_scale != self.output_scale {
                            self.rebuild(queue, want_output.clone())?;
                        } else if let Some(layer) = self.layer.clone() {
                            self.apply_margins(&layer);
                            layer.commit();
                        }
                    }
                    Cmd::Visible(on) => self.set_mapped(on),
                    Cmd::Quit => {
                        // 丢下 Widget 就自动发 destroy；不主动 attach null，免得闪一帧。
                        self.layer = None;
                        self.surface = None;
                        return Ok(());
                    }
                }
            }
            if self.shared.closed.load(Ordering::Relaxed) {
                return Ok(());
            }
            if self.shared.visible.load(Ordering::Relaxed) {
                if let Err(err) = self.draw() {
                    log::warn!("wayland draw failed: {err}");
                }
            }
            queue
                .blocking_dispatch(self)
                .map_err(|e| format!("dispatch: {e}"))?;
        }
    }

    /// 换屏/换缩放：整个 role 重建（wl_surface 也换新的，旧的 drop 时自动 destroy）。
    ///
    /// 旧表面**留到新表面 configure 到、而且已经提交了一帧**之后才丢：跨屏拖动时
    /// 每换一块屏都要走一次，先销毁再重建的话缝上必然闪一帧空（拖过去看着就是卡）。
    fn rebuild(
        &mut self,
        queue: &mut EventQueue<Self>,
        wanted: Option<wl_output::WlOutput>,
    ) -> Result<(), String> {
        let old = (self.layer.take(), self.surface.take(), self.pool.take());
        self.configured = false;
        // 让下面的 draw 认为"还没说过这一帧"，configure 一到就重画。
        self.shared.drawn_seq.store(u64::MAX, Ordering::Relaxed);
        let qh = self.qh.clone();
        self.create_surface(&qh, wanted)?;
        self.shared.rebuilds.fetch_add(1, Ordering::Relaxed);
        // 等新表面的 configure（`blocking_dispatch` 会把它投进来），立刻先画一帧，
        // 然后才丢掉旧表面。最多等 8 次派发：合成器不回也不能把 UI 饿死。
        for _ in 0..8 {
            if self.configured || self.shared.closed.load(Ordering::Relaxed) {
                break;
            }
            queue
                .blocking_dispatch(self)
                .map_err(|e| format!("dispatch: {e}"))?;
        }
        if self.configured {
            if let Err(err) = self.draw() {
                log::warn!("wayland draw after rebuild failed: {err}");
            }
        }
        drop(old);
        Ok(())
    }

    fn set_mapped(&mut self, on: bool) {
        let Some(surface) = self.surface.clone() else {
            return;
        };
        if on {
            // 显示：逼一帧出来（attach 一个真 buffer 才会 map）。
            self.shared.drawn_seq.store(u64::MAX, Ordering::Relaxed);
            if !self.configured {
                // 卸下（`attach(None)`）之后合成器会把这块 layer surface 的
                // "configured" 清掉（wlroots/KWin 的 unmap 都这么干）。协议规定
                // **第一次 configure 之前不许 attach buffer**，违了就吃一个
                // role 错误："a buffer has been attached to a layer surface prior
                // to the first layer_surface.configure event"，整块表面被合成器
                // 关掉——表现就是「按了热键，日志说 shown，屏幕上什么都没有」。
                //
                // 所以显示分两步：先空 commit 去换一个 configure，
                // 真帧等 configure 到了由 `run` 的下一轮去画。
                match self.layer.as_ref() {
                    Some(layer) => layer.commit(),
                    None => surface.commit(),
                }
                return;
            }
            if let Err(err) = self.draw() {
                log::warn!("wayland redraw after show failed: {err}");
            }
        } else {
            // **不要把表面卸下来**（`attach(None)`）。
            //
            // KWin 对 layer surface 的 unmap 会立刻回一个 `closed` 事件，而且之后
            // 再想上图只能碰“configure 之前不许 attach buffer”那颗雷——两条路都不通，
            // 表面就彻底废了。实测：按热键 hide → show，日志说 `zouwu shown`，
            // 屏幕上永远空着，然后我们只好重建表面，重建又因为尺寸单位算错
            // 一次比一次大一倍，几轮之后整只飞到屏幕外。
            //
            // 换个办法：表面一直 map 着，隐藏 = 提交一帧**全透明**的 buffer。
            // 全透明 → 输入区域收集不到任何矩形 → 鼠标整个穿透；合成器那边只是
            // 混合一张空图，跟“没显示”对用户毫无区别，但没有任何协议上的坑。
            self.blank();
        }
    }

    /// 提交一帧全透明的 buffer（隐藏用）。
    fn blank(&mut self) {
        let (w, h) = *lock(&self.shared.size);
        let need = (w as usize) * (h as usize) * 4;
        {
            let mut frame = lock(&self.shared.frame);
            if frame.len() < need {
                frame.resize(need, 0);
            }
            frame[..need].fill(0);
        }
        self.shared.frame_seq.fetch_add(1, Ordering::Relaxed);
        if let Err(err) = self.draw() {
            log::warn!("wayland blank frame failed: {err}");
        }
    }

    fn draw(&mut self) -> Result<(), String> {
        if !self.configured {
            return Ok(());
        }
        let seq = self.shared.frame_seq.load(Ordering::Relaxed);
        if seq == self.shared.drawn_seq.load(Ordering::Relaxed) {
            return Ok(());
        }
        let (w, h) = *lock(&self.shared.size);
        let need = (w as usize) * (h as usize) * 4;
        let stride = (w * 4) as i32;
        let t_start = std::time::Instant::now();
        // 先把像素拷出来，别拿着 shared 的锁去碰 pool。
        let pixels = {
            let frame = lock(&self.shared.frame);
            if frame.len() < need {
                return Ok(());
            }
            frame[..need].to_vec()
        };
        let Some(surface) = self.surface.clone() else {
            return Ok(());
        };
        let Some(pool) = self.pool.as_mut() else {
            return Ok(());
        };
        if pool.len() < need {
            pool.resize(need).map_err(|e| format!("pool resize: {e}"))?;
        }
        let (buffer, canvas) =
            match pool.create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888) {
                Ok(pair) => pair,
                Err(err) => {
                    // 槽位都还在合成器手里：丢这一帧，下一帧再试。
                    log::debug!("wayland: buffer busy ({err})");
                    return Ok(());
                }
            };
        // 预乘 RGBA → 小端 ARGB8888（内存字节序 B,G,R,A）。
        for (src, dst) in pixels.chunks_exact(4).zip(canvas.chunks_exact_mut(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        let t_swap = t_start.elapsed();
        let mut rects = Vec::new();
        collect_input_rects(&pixels, w, h, self.scale, &mut rects);
        let t_rects = t_start.elapsed();
        // 只要 alpha 不透明的格子吃鼠标：气泡外面点得到桌面图标。
        //
        // 只在**轮廓真的变了**的时候才重发 region：呼吸动画每帧都在动，
        // 但按缩放量化之后大多数帧的掩码是同一个，省下几百个 Wayland 请求。
        if rects != self.input_rects {
            self.input_rects = rects.clone();
            apply_input_region(&self.compositor, &surface, self.scale, &rects);
        }
        surface.damage_buffer(0, 0, w as i32, h as i32);
        buffer
            .attach_to(&surface)
            .map_err(|e| format!("attach: {e}"))?;
        if let Some(layer) = self.layer.as_ref() {
            layer.commit();
        } else {
            surface.commit();
        }
        self.shared.drawn_seq.store(seq, Ordering::Relaxed);
        if std::env::var_os("ZOUWU_PERF").is_some_and(|v| !v.is_empty()) {
            let total = t_start.elapsed();
            log::info!(
                "perf: wayland frame {:.2}ms (copy+swap {:.2}, rects {:.2}, commit {:.2}), \
                 {} input rect(s)",
                total.as_secs_f64() * 1000.0,
                t_swap.as_secs_f64() * 1000.0,
                (t_rects - t_swap).as_secs_f64() * 1000.0,
                (total - t_rects).as_secs_f64() * 1000.0,
                rects.len()
            );
        }
        // 前几帧 + 之后每 120 帧打一条：这是"画面到底有没有上去"的唯一证据
        // （Wayland 下客户端看不到自己的窗口，只能靠日志确认提交成功）。
        if self.frames < 3 || self.frames % 120 == 0 {
            let (px, py) = *lock(&self.shared.pos);
            log::debug!(
                "wayland draw #{}, {w}x{h} px, pos {px},{py} logical, scale {:.2}, {} input rect(s)",
                self.frames,
                self.scale,
                rects.len()
            );
        }
        self.frames = self.frames.wrapping_add(1);
        Ok(())
    }
}

/// 只有 alpha 真的不透明的格子才吃鼠标。矩形是**缓冲像素**，除以缩放后填进 region。
/// 空 region = 整块不吃鼠标 = 彻底穿透。
///
/// `scale` 是**分数**的（1.15 也算）：输入区域的坐标是表面本地坐标，也就是
/// `logical_size` 那套，所以必须除真正的每逻辑像素比，而不是整数 buffer scale。
fn apply_input_region(
    compositor: &CompositorState,
    surface: &wl_surface::WlSurface,
    scale: f64,
    rects: &[(i32, i32, i32, i32)],
) {
    let Ok(region) = Region::new(compositor) else {
        return;
    };
    let s = if scale > 0.0 { scale } else { 1.0 };
    for (x, y, rw, rh) in rects {
        region.add(
            (*x as f64 / s).round() as i32,
            (*y as f64 / s).round() as i32,
            (*rw as f64 / s).round().max(1.0) as i32,
            (*rh as f64 / s).round().max(1.0) as i32,
        );
    }
    surface.set_input_region(Some(region.wl_region()));
}

/// 把 alpha 不透明的行合并成若干矩形（而不是逐像素，也不是整块）。
fn collect_input_rects(
    rgba: &[u8],
    w: u32,
    h: u32,
    scale: f64,
    out: &mut Vec<(i32, i32, i32, i32)>,
) {
    // 把矩形对齐到 "一个逻辑像素" 的网格上（不是整数 buffer scale：1.15 的屏上
    // 一个逻辑像素是 1.15 个缓冲像素），并且**向外**扩：
    // 结果永远是真轮廓的超集（不会有点不到的地方），但小幅度动画
    // 不会每帧改变掩码，`draw()` 里的比较就能把重发省掉。
    let scale = if scale > 0.0 { scale } else { 1.0 };
    out.clear();
    let w = w as usize;
    // 向下取整到网格边界（x0）、向上取整（x1）。
    let snap_lo = |x: usize| ((x as f64 / scale).floor() * scale).floor().max(0.0) as usize;
    let snap_hi = |x: usize| ((x as f64 / scale).ceil() * scale).ceil() as usize;
    let mut open: Option<(i32, i32, i32)> = None; // (x0, x1, y0)
    for y in 0..h as usize {
        let row = &rgba[y * w * 4..(y + 1) * w * 4];
        let mut x0 = None;
        let mut x1 = 0;
        for x in 0..w {
            if row[x * 4 + 3] > INPUT_ALPHA {
                if x0.is_none() {
                    x0 = Some(x);
                }
                x1 = x;
            }
        }
        let (x0, x1) = match x0 {
            Some(x0) => (Some(snap_lo(x0)), snap_hi(x1 + 1).min(w)),
            None => (None, 0),
        };
        let x1 = x1;
        let x0 = x0;
        match (open, x0) {
            (Some((ax0, ax1, ay0)), Some(bx0)) => {
                if ax0 == bx0 as i32 && ax1 == x1 as i32 {
                    continue; // 和上一行完全一样：接着长
                }
                out.push((ax0, ay0, ax1 - ax0, y as i32 - ay0));
                open = Some((bx0 as i32, x1 as i32, y as i32));
            }
            (Some((ax0, ax1, ay0)), None) => {
                out.push((ax0, ay0, ax1 - ax0, y as i32 - ay0));
                open = None;
            }
            (None, Some(bx0)) => open = Some((bx0 as i32, x1 as i32, y as i32)),
            (None, None) => {}
        }
        if out.len() >= MAX_INPUT_RECTS {
            // 形状太碎：退回整块，省掉几百个请求（代价是四角也能点）。
            out.clear();
            out.push((0, 0, w as i32, h as i32));
            return;
        }
    }
    if let Some((ax0, ax1, ay0)) = open {
        out.push((ax0, ay0, ax1 - ax0, h as i32 - ay0));
    }
}

// ---------------------------------------------------------------- sctk 接线

impl CompositorHandler for Widget {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        factor: i32,
    ) {
        // 这是**取整过**的 buffer scale（1.15 的屏上 KWin 会报 2）。有分数缩放时
        // 真值走 `preferred_scale`，别让这个取整值把 1.15 盖成 2——那正是之前
        // 小兽大 74% 的原因。没分数协议时它就是唯一的真相。
        self.output_scale = factor.max(1);
        *lock(&self.shared.monitor_scale) = self.output_scale;
        if self.fractional.is_none() {
            self.scale = factor.max(1) as f64;
            self.sync_size();
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wayland_client::protocol::wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    /// 合成器告诉我们表面进了哪块屏——**故意不听**。
    ///
    /// 表面属于哪块屏是 `create_surface` 定死的，而合成器的判断标准（比如按表面
    /// 中心算）和我们的"跨屏按重叠面积挑一块"不是一回事：骑在缝上时 KWin 会说旧屏，
    /// 于是 `attached_output` 每一步都被改回去、和刚建好的表面打架，`Cmd::Move`
    /// 就每来一个 motion 事件重建一次表面（实测 150 步 24 次，拖过去一卡一卡）。
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Widget {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.publish_monitors();
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.publish_monitors();
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.publish_monitors();
    }
}

impl LayerShellHandler for Widget {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        log::warn!("wayland: layer surface closed by compositor");
        self.shared.closed.store(true, Ordering::Relaxed);
        self.shared.wake();
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        // 我们给了非 0 的 set_size，合成器一般原样回；真被改了就跟随它。
        if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
            self.logical_size = configure.new_size;
        }
        // viewport 的 destination 与 buffer 尺寸都得跟着这个尺寸走。
        self.sync_size();
        self.configured = true;
    }
}

impl SeatHandler for Widget {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(err) => log::warn!("wayland: no pointer: {err}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

/// surface-local **逻辑**坐标 → 全局**物理**坐标。
///
/// `pos` 是表面左上角的全局逻辑位置，`local` 是 sctk 给的表面内逻辑坐标，
/// `scale` 是这张表面的整数缩放。三者必须走同一个公式，否则拖动、命中判定和
/// `Drag` 会各自活在不同的坐标系里——这正是「鼠标拖不动」的成因。
fn global_physical(pos: (i32, i32), local: (f64, f64), scale: i32) -> (i32, i32) {
    let s = scale.max(1) as f64;
    (
        ((pos.0 as f64 + local.0) * s).round() as i32,
        ((pos.1 as f64 + local.1) * s).round() as i32,
    )
}

impl PointerHandler for Widget {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let Some(surface) = self.surface.clone() else {
            return;
        };
        let scale = self.output_scale.max(1);
        let (px, py) = *lock(&self.shared.pos);
        for event in events {
            if event.surface != surface {
                continue;
            }
            // sctk 给的是 surface-local **逻辑**坐标，`pos` 也是逻辑坐标；
            // 两者先相加再乘缩放，才是**全局物理**坐标（和 Windows 的 GetCursorPos 同域）。
            //
            // 这里踩过一次坑：当初只把 local 乘 scale、pos 原样相加，等于把两个
            // 不同单位的数拼在一起——拖动时 `Drag` 拿到的位移是真实的 2 倍，
            // 窗口跟着鼠标跑得比手快（用户看到的就是「鼠标拖不动/乱飞」）。
            let (gx, gy) = global_physical((px, py), event.position, scale);
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    *lock(&self.shared.cursor) = Some((gx, gy));
                    self.shared.push_input(PetInput::Motion { x: gx, y: gy });
                }
                PointerEventKind::Leave { .. } => {
                    *lock(&self.shared.cursor) = None;
                    *lock(&self.shared.buttons) = (false, false);
                    self.shared.push_input(PetInput::Leave);
                }
                PointerEventKind::Press { button, .. } => {
                    let b = map_button(button);
                    {
                        let mut down = lock(&self.shared.buttons);
                        match b {
                            PointerButton::Left => down.0 = true,
                            PointerButton::Right => down.1 = true,
                            _ => {}
                        }
                    }
                    self.shared.push_input(PetInput::Press(b));
                }
                PointerEventKind::Release { button, .. } => {
                    let b = map_button(button);
                    {
                        let mut down = lock(&self.shared.buttons);
                        match b {
                            PointerButton::Left => down.0 = false,
                            PointerButton::Right => down.1 = false,
                            _ => {}
                        }
                    }
                    self.shared.push_input(PetInput::Release(b));
                    // 松手后重新"归位"：拖拽时允许窗口骑在缝上跟着光标走，松手
                    // 则按重叠面积挑一块屏落定（必要时重建表面），这样停下的位置
                    // 和用户看到的一致，不会留个半截挂在别人屏幕上。
                    let pos = *lock(&self.shared.pos);
                    self.shared.push_cmd(Cmd::Move(pos.0, pos.1));
                }
                _ => {}
            }
        }
    }
}

impl ShmHandler for Widget {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

// ------------------------------------------------ 分数缩放（wp_fractional_scale_v1）

impl Dispatch<WpFractionalScaleManagerV1, ()> for Widget {
    fn event(
        _: &mut Self,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // 管理器本身没有事件。
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for Widget {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: FractionalScaleEvent,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // `preferred_scale` 的单位是 120 分之一：138 -> 1.15，240 -> 2.0。
        // 它可能比 configure 还晚到，所以收到就得重算 buffer 尺寸（`sync_size`
        // 会把新尺寸推给 UI，让画布重建）。
        if let FractionalScaleEvent::PreferredScale { scale } = event {
            let frac = if scale > 0 {
                scale as f64 / 120.0
            } else {
                state.output_scale.max(1) as f64
            };
            if (frac - state.scale).abs() > 1e-6 {
                log::debug!("wayland: preferred fractional scale {} -> {frac}", scale);
                state.scale = frac;
                state.sync_size();
            }
        }
    }
}

impl Dispatch<WpViewporter, ()> for Widget {
    fn event(
        _: &mut Self,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewport, ()> for Widget {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl ProvidesRegistryState for Widget {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl Dispatch<wl_callback::WlCallback, ()> for Widget {
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        _: <wl_callback::WlCallback as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // 只是一个"叫醒信号"，内容无所谓。
    }
}

delegate_compositor!(Widget);
delegate_output!(Widget);
delegate_shm!(Widget);
delegate_seat!(Widget);
delegate_pointer!(Widget);
delegate_layer!(Widget);
delegate_registry!(Widget);

// ---------------------------------------------------------------- 给 UI 层用的门面
//
// 签名尽量和 Windows 的 `layer` 对齐（`hwnd` 在这里是一个 `isize` 表面 id），
// 这样 `app.rs` 里那一串 `layer::xxx(hwnd, ...)` 一行都不用改。

/// 存不存在一个活着的表面（给 UI 层判断"挂件是否还可控"用）。
pub fn is_alive(id: isize) -> bool {
    lookup(id).is_some_and(|s| !s.closed.load(Ordering::Relaxed))
}

/// 通过共享状态执行（找不到表面就安静地什么都不做）。
macro_rules! via_shared {
    ($id:expr, $shared:ident => $body:expr) => {{
        match lookup($id) {
            Some($shared) => $body,
            None => return,
        }
    }};
}

// 说明：真正常用的是 `PetSurface` 的方法本身；下面这些自由函数只是为了
// 让 `app.rs` 的 `layer::xxx(hwnd, ..)` 调用点保持原样。

pub fn set_visible(id: isize, visible: bool) {
    via_shared!(id, shared => {
        if shared.visible.swap(visible, Ordering::Relaxed) != visible {
            shared.push_cmd(Cmd::Visible(visible));
            // 隐藏不会再有 `present()`：必须直接戳 Wayland 线程，否则全透明的
            // 那一帧永远排在队列里，屏幕上留着上一张不透明的图。
            shared.poke();
            if let Some(waker) = lock(&shared.waker).clone() {
                waker();
            }
        }
    });
}

pub fn is_visible(id: isize) -> bool {
    lookup(id).is_some_and(|s| s.visible.load(Ordering::Relaxed))
}

pub fn set_bounds(id: isize, x: i32, y: i32, width: i32, height: i32) {
    via_shared!(id, shared => {
        {
            let mut pos = lock(&shared.pos);
            if *pos != (x, y) {
                *pos = (x, y);
                shared.push_cmd(Cmd::Move(x, y));
            }
        }
        {
            let mut size = lock(&shared.size);
            let new = (width.max(1) as u32, height.max(1) as u32);
            if *size != new {
                *size = new;
                let scale = lock(&shared.scale).max(1.0);
                *lock(&shared.logical) = (
                    (new.0 as f64 / scale).round().max(1.0) as u32,
                    (new.1 as f64 / scale).round().max(1.0) as u32,
                );
                shared.push_cmd(Cmd::Resize(new.0, new.1));
            }
        }
        if let Some(waker) = lock(&shared.waker).clone() {
            waker();
        }
    });
}

pub fn window_rect(id: isize) -> Option<(i32, i32, i32, i32)> {
    let shared = lookup(id)?;
    let (x, y) = *lock(&shared.pos);
    let (w, h) = *lock(&shared.size);
    Some((x, y, w as i32, h as i32))
}

pub fn bring_to_front(_id: isize) {}
pub fn activate(_id: isize) -> bool {
    false
}
pub fn set_always_on_top(_id: isize, _on_top: bool) {}
pub fn hide_from_taskbar(_id: isize) {}
pub fn set_dark_title_bar(_id: isize, _dark: bool) -> bool {
    false
}
pub fn set_title_colors(_id: isize, _caption: u32, _text: u32, _border: u32) -> bool {
    false
}

/// 某个屏幕点下面是不是我们的本体（挂件只有一个 layer surface）。
///
/// 签名和 Windows 版一致（那个平台可能有多个顶层窗口，所以返回 `Option<isize>`
/// 作为"命中谁"；这里恒等于"命中涂吾或没命中"）。
pub fn window_at(x: i32, y: i32) -> Option<isize> {
    let shared = single()?;
    let (wx, wy) = *lock(&shared.pos);
    let (ww, wh) = *lock(&shared.size);
    (x >= wx && y >= wy && x < wx + ww as i32 && y < wy + wh as i32).then_some(shared.id)
}

/// 全局物理光标位置。
///
/// **Wayland 客户端没有"查询全局光标"的能力**：只有指针落在我自己的表面上时
/// 我们才收到坐标。所以这个函数在光标离开本体的那一刻会返回 None——
/// 调用方（app.rs 的轮询）本来就是这么用的：None = 光标不在我身上。
pub fn cursor_position() -> Option<(i32, i32)> {
    single().and_then(|s| *lock(&s.cursor))
}

pub fn left_button_down() -> bool {
    single().is_some_and(|s| lock(&s.buttons).0)
}

pub fn right_button_down() -> bool {
    single().is_some_and(|s| lock(&s.buttons).1)
}

/// 唯一那个挂件表面（找不到说明还没建好或已经销毁）。
fn single() -> Option<Arc<Shared>> {
    let mut reg = lock(registry());
    reg.retain(|w| w.strong_count() > 0);
    reg.first().and_then(|w| w.upgrade())
}

/// 显示器几何（全局物理像素 + 缩放），给"把它拽回屏幕内"用。
pub fn monitors() -> Vec<(i32, i32, i32, i32, f64)> {
    single()
        .map(|s| lock(&s.monitors).clone())
        .unwrap_or_default()
}

/// 由 output 的物理/逻辑尺寸算出分数缩放（协议单位是 1/120）。
///
/// 2560/2226 = 1.15、2560/2048 = 1.25、2560/1280 = 2，和 `wp_fractional_scale_v1`
/// 的 `preferred_scale` 报的一模一样——所以不用等那次往返，建表面时就能直接算对。
fn snap_scale(physical: i32, logical: i32) -> Option<f64> {
    (physical > 0 && logical > 0)
        .then(|| (physical as f64 / logical as f64 * 120.0).round() / 120.0)
}

/// 把一维坐标夹成"至少 [`KEEP_ON_SCREEN`] 还在屏上"。
///
/// `origin`/`span` 是屏的逻辑起点与长度，`len` 是窗口长度。窗口比屏还长时
/// 没有能同时满足的区间，就居中放（否则 `clamp` 会 panic）。
fn keep_visible(value: i32, origin: i32, span: i32, len: i32) -> i32 {
    let lo = origin - len + KEEP_ON_SCREEN;
    let hi = origin + span - KEEP_ON_SCREEN;
    if lo <= hi {
        value.clamp(lo, hi)
    } else {
        origin - (len - span) / 2
    }
}

#[cfg(test)]
mod tests {
    use super::{collect_input_rects, global_physical, keep_visible, snap_scale};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    /// 隐藏时要直接戳 Wayland 线程。
    ///
    /// 回归点：`waker` 只叫醒 UI 线程，而隐藏后 UI 没有帧要画、不会再调
    /// `present()`；少了这一次 `poke`，`Cmd::Visible(false)` 会一直躺在队列里，
    /// 屏幕上留着上一张不透明的图（日志写着 hidden，小兽却还在）。
    #[test]
    fn hiding_wakes_the_wayland_thread() {
        let pokes = Arc::new(AtomicU64::new(0));
        let counter = pokes.clone();
        let shared = Arc::new(super::Shared {
            id: super::NEXT_ID.fetch_add(1, Ordering::Relaxed) as isize,
            size: Mutex::new((340, 340)),
            logical: Mutex::new((340, 340)),
            pos: Mutex::new((0, 0)),
            scale: Mutex::new(1.15),
            monitor_scale: Mutex::new(1),
            frame: Mutex::new(Vec::new()),
            frame_seq: AtomicU64::new(0),
            drawn_seq: AtomicU64::new(0),
            visible: AtomicBool::new(true),
            cursor: Mutex::new(None),
            buttons: Mutex::new((false, false)),
            input: Mutex::new(std::collections::VecDeque::new()),
            cmds: Mutex::new(std::collections::VecDeque::new()),
            waker: Mutex::new(None),
            poke: Mutex::new(Some(Arc::new(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            }))),
            monitors: Mutex::new(Vec::new()),
            refresh_mhz: Mutex::new(0),
            closed: AtomicBool::new(false),
            rebuilds: AtomicU64::new(0),
        });
        super::lock(&super::registry()).push(Arc::downgrade(&shared));

        super::set_visible(shared.id, false);
        assert_eq!(pokes.load(Ordering::Relaxed), 1, "hide must wake wayland");
        assert!(!shared.visible.load(Ordering::Relaxed));

        // 状态没变就什么都不该做（避免每次轮询都往队列里塞命令）。
        super::set_visible(shared.id, false);
        assert_eq!(pokes.load(Ordering::Relaxed), 1);

        super::set_visible(shared.id, true);
        assert_eq!(pokes.load(Ordering::Relaxed), 2, "show must wake wayland");
    }

    /// 造一块 10×4 的图：只在 x 2..6、y 1..3 涂上不透明像素（其余全透明）。
    fn opaque_patch() -> (Vec<u8>, u32, u32) {
        let (w, h) = (10u32, 4u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 1..3u32 {
            for x in 2..6u32 {
                rgba[((y * w + x) * 4 + 3) as usize] = 255;
            }
        }
        (rgba, w, h)
    }

    /// 输入区域必须**盖住**每一个不透明像素（否则那块就点不到）；
    /// 这是轮廓破碎/分数缩放取整时唯一不能破的不变量。
    fn covers(rgba: &[u8], w: u32, h: u32, rects: &[(i32, i32, i32, i32)]) -> bool {
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let alpha = rgba[((y as u32 * w + x as u32) * 4 + 3) as usize];
                if alpha <= 8 {
                    continue;
                }
                let hit = rects
                    .iter()
                    .any(|(rx, ry, rw, rh)| x >= *rx && x < rx + rw && y >= *ry && y < ry + rh);
                if !hit {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn input_rects_cover_the_patch_at_one_x() {
        let (rgba, w, h) = opaque_patch();
        let mut rects = Vec::new();
        collect_input_rects(&rgba, w, h, 1.0, &mut rects);
        assert!(covers(&rgba, w, h, &rects));
        // 1x 下应该是贴着像素的紧矩形（不是整块）。
        assert_eq!(rects, vec![(2, 1, 4, 2)]);
    }

    /// 1.15 的屏上，网格不再落在整数像素上：只要求“盖得住”，并且
    /// 边缘向外取整（不能向内，否则会有一列点不到）。
    #[test]
    fn input_rects_cover_the_patch_at_fractional_scale() {
        let (rgba, w, h) = opaque_patch();
        let mut rects = Vec::new();
        collect_input_rects(&rgba, w, h, 1.15, &mut rects);
        assert!(covers(&rgba, w, h, &rects));
        let (x0, _, x1_w, _) = rects[0];
        assert!(x0 <= 2, "left edge must round outwards, got {x0}");
        assert!(x0 + x1_w >= 6, "right edge must round outwards");
    }

    /// 全透明（隐藏帧）= 空 region = 鼠标整个穿透。
    #[test]
    fn a_blank_frame_has_no_input_area() {
        let (w, h) = (8u32, 8u32);
        let rgba = vec![0u8; (w * h * 4) as usize];
        let mut rects = vec![(0, 0, 1, 1)];
        collect_input_rects(&rgba, w, h, 1.15, &mut rects);
        assert!(rects.is_empty());
    }

    /// 建表面时就按 physical/logical 算对了分数缩放，别等 `preferred_scale`
    /// 那次往返（否则第一帧 buffer 会大一圈，`present` 直接被丢掉）。
    #[test]
    fn fractional_hint_matches_what_kwin_reports() {
        assert_eq!(snap_scale(2560, 2226), Some(1.15)); // 本机：KWin 报 preferred_scale 138
        assert_eq!(snap_scale(2560, 2048), Some(1.25));
        assert_eq!(snap_scale(2560, 1280), Some(2.0));
        assert_eq!(snap_scale(0, 1280), None);
    }

    #[test]
    fn keep_visible_leaves_inside_positions_alone() {
        // 屏 0..2226，窗口 340：屏内任意位置原样保留。
        assert_eq!(keep_visible(500, 0, 2226, 340), 500);
        assert_eq!(keep_visible(1886, 0, 2226, 340), 1886);
    }

    #[test]
    fn keep_visible_allows_straddling_but_keeps_a_slice() {
        // 骑在右边缘外：最多允许跑到"只剩 56px 在屏上"。
        assert_eq!(keep_visible(2100, 0, 2226, 340), 2100);
        assert_eq!(keep_visible(9999, 0, 2226, 340), 2226 - 56);
        // 越到左边同理。
        assert_eq!(keep_visible(-200, 0, 2226, 340), -200);
        assert_eq!(keep_visible(-9999, 0, 2226, 340), -340 + 56);
    }

    #[test]
    fn keep_visible_centers_when_the_window_is_wider_than_the_output() {
        // 屏比"两个 KEEP 还窄"：没有合法区间，居中放而不是 panic。
        assert_eq!(keep_visible(0, 0, 40, 60), -10);
    }

    /// 表面在逻辑 (916,226)、scale 2、指针在表面内逻辑 (10,10)
    /// → 全局物理 ((916+10)*2, (226+10)*2)。
    #[test]
    fn local_pointer_becomes_global_physical() {
        assert_eq!(global_physical((916, 226), (10.0, 10.0), 2), (1852, 472));
    }

    /// 指针在表面内移动 30 逻辑像素，`Drag` 看到的位移必须是 60 物理像素。
    /// 旧写法（pos 不乘 scale 就相加）会给出 90，于是窗口跑得比鼠标快 1.5 倍。
    #[test]
    fn drag_delta_matches_pointer_delta() {
        let a = global_physical((916, 226), (10.0, 10.0), 2);
        let b = global_physical((916, 226), (40.0, 10.0), 2);
        assert_eq!(b.0 - a.0, 60);
    }

    /// 1x 屏上换算必须是无损恒等，免得给没开缩放的机器引入取整抖动。
    #[test]
    fn identity_at_scale_one() {
        assert_eq!(global_physical((100, 200), (7.0, 9.0), 1), (107, 209));
    }
}
