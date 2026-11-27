//! 护眼遮罩（Linux/Wayland）：整屏暖色、完全穿透、不抢焦点。
//!
//! ## 为什么不是一个"全屏置顶窗口"
//!
//! Wayland 客户端没法把自己摆到任意位置、也没法保证压住别人的窗口——只有
//! `wlr-layer-shell` 的 `overlay` 层能保证"在所有窗口之上"。所以每块屏幕起一个
//! overlay 层的层表面：
//!
//! - `set_anchor(四边) + set_size(0, 0)`：铺满整块屏，输出分辨率变了也跟着变；
//! - `set_exclusive_zone(-1)`：不挤走桌面可用区域（否则窗口会被"缩"进去）；
//! - `set_keyboard_interactivity(None)`：永远不拿键盘焦点；
//! - `set_input_region(空)`：整块**吞不掉**任何点击，鼠标照常落在下面的窗口上。
//!
//! 和 Windows 版（`WS_EX_LAYERED|WS_EX_TRANSPARENT|WS_EX_NOACTIVATE` + `LWA_ALPHA`）
//! 同一个思路，但那边还得靠 `SetWinEventHook` 反复重贴来压住新的置顶窗口；
//! 这里由合成器保证层序，**不需要任何 hook**。
//!
//! 明确不用 `SetDeviceGammaRamp` 那一套的 Wayland 等价物（KWin 的 NightLight）：
//! 那是"篡改用户的夜灯设置"，关掉时还原不干净。这里只是一个半透明矩形。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
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
    protocol::{wl_callback, wl_display::WlDisplay, wl_output, wl_shm, wl_surface},
    Connection, Dispatch, Proxy, QueueHandle,
};

/// 暖色。和 Windows 版同一个值：`COLORREF 0x006EC6FF` = `RGB(255, 198, 110)`。
///
/// 偏琥珀而不是纯橙：纯橙（B≈0）在白底上会把正文染成粉色，琥珀在同样的
/// 不透明度下只是"白得暖一点"。
const TINT: (u8, u8, u8) = (255, 198, 110);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 预乘 RGBA 的遮罩色。
fn tint_rgba(alpha: u8) -> [u8; 4] {
    let a = alpha as u32;
    let scale = |c: u8| ((c as u32 * a) / 255) as u8;
    [scale(TINT.0), scale(TINT.1), scale(TINT.2), alpha]
}

// ------------------------------------------------------------------ 对外状态

fn handle_slot() -> &'static Mutex<Option<Handle>> {
    static HANDLE: OnceLock<Mutex<Option<Handle>>> = OnceLock::new();
    HANDLE.get_or_init(|| Mutex::new(None))
}

fn alpha_slot() -> &'static Mutex<u8> {
    static ALPHA: OnceLock<Mutex<u8>> = OnceLock::new();
    ALPHA.get_or_init(|| Mutex::new(0))
}

/// 应用护眼状态。`alpha` 是遮罩不透明度（`0..=255`，0 = 无效果）。
pub fn apply(enabled: bool, alpha: u8) -> Result<(), String> {
    *lock(alpha_slot()) = alpha;
    if !(enabled && alpha > 0) {
        stop();
        return Ok(());
    }
    let want = tint_rgba(alpha);
    // 已经在跑：只换颜色，不重建表面（避免闪一下）。
    if let Some(handle) = lock(handle_slot()).as_ref() {
        *lock(&handle.desired) = Some(want);
        handle.nudge();
        return Ok(());
    }
    let handle = Handle::spawn(want)?;
    *lock(handle_slot()) = Some(handle);
    Ok(())
}

/// 撤掉遮罩（关掉开关、退出程序时调用）。可重复调用。
pub fn stop() {
    // 先摘掉句柄再通知线程退出：并发的第二次 `apply(true)` 会重新造一个，
    // 不会拿到一个正在退出的遮罩。
    let handle = lock(handle_slot()).take();
    if let Some(handle) = handle {
        handle.shutdown();
    }
}

pub fn shutdown() {
    stop();
}

pub fn is_active() -> bool {
    lock(handle_slot()).is_some()
}

pub fn alpha() -> u8 {
    *lock(alpha_slot())
}

// ------------------------------------------------------------------ 线程句柄

struct Handle {
    /// 留着它只为了能从 UI 线程 `wl_display.sync` 叫醒 Wayland 线程。
    conn: Connection,
    display: WlDisplay,
    qh: QueueHandle<Overlay>,
    desired: Arc<Mutex<Option<[u8; 4]>>>,
    quit: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Handle {
    fn spawn(tint: [u8; 4]) -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("wayland connect: {e}"))?;
        let (globals, mut queue) = registry_queue_init::<Overlay>(&conn)
            .map_err(|e| format!("wayland registry: {e}"))?;
        let qh = queue.handle();
        let desired = Arc::new(Mutex::new(None::<[u8; 4]>));
        let quit = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let mut overlay = Overlay::new(&globals, &qh, desired.clone(), quit.clone(), done.clone())?;
        overlay.tint = tint;
        // 先走完一轮 registry 拿到 output，再建表面。
        queue
            .roundtrip(&mut overlay)
            .map_err(|e| format!("wayland roundtrip: {e}"))?;
        overlay.create_panes(&qh);
        overlay.paint_all(&qh);
        queue
            .roundtrip(&mut overlay)
            .map_err(|e| format!("wayland roundtrip: {e}"))?;
        log::info!(
            "eye care: overlay on {} output(s), alpha {}",
            overlay.panes.len(),
            tint[3]
        );

        let thread_qh = qh.clone();
        std::thread::Builder::new()
            .name("zouwu-eyecare".into())
            .spawn(move || {
                overlay.run(&mut queue, &thread_qh);
            })
            .map_err(|e| format!("spawn eye care thread: {e}"))?;

        let display = conn.display();
        Ok(Self {
            conn,
            display,
            qh,
            desired,
            quit,
            done,
        })
    }

    /// 叫醒 Wayland 线程（同一个 fd 上发一次 sync，dispatch 会立刻返回）。
    fn nudge(&self) {
        self.display.sync(&self.qh, ());
        let _ = self.conn.flush();
    }

    fn shutdown(self) {
        self.quit.store(true, Ordering::Relaxed);
        self.nudge();
        // 给线程 ~300ms 收摊（撤掉 surface 再退）；超时就直接丢连接。
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while std::time::Instant::now() < deadline {
            if self.done.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

// ------------------------------------------------------------------ Wayland 侧

struct Pane {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    surface: wl_surface::WlSurface,
    /// 逻辑尺寸（configure 给的）。
    size: (u32, u32),
    scale: i32,
    pool: Option<SlotPool>,
}

struct Overlay {
    registry_state: RegistryState,
    compositor: CompositorState,
    output_state: OutputState,
    shm: Shm,
    layer_shell: LayerShell,
    panes: Vec<Pane>,
    /// 当前遮罩色（预乘 RGBA）。
    tint: [u8; 4],
    desired: Arc<Mutex<Option<[u8; 4]>>>,
    quit: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Overlay {
    fn new(
        globals: &wayland_client::globals::GlobalList,
        qh: &QueueHandle<Self>,
        desired: Arc<Mutex<Option<[u8; 4]>>>,
        quit: Arc<AtomicBool>,
        done: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let compositor =
            CompositorState::bind(globals, qh).map_err(|e| format!("wl_compositor: {e}"))?;
        let layer_shell = LayerShell::bind(globals, qh)
            .map_err(|e| format!("zwlr_layer_shell_v1 (护眼遮罩需要 layer-shell): {e}"))?;
        let shm = Shm::bind(globals, qh).map_err(|e| format!("wl_shm: {e}"))?;
        Ok(Self {
            registry_state: RegistryState::new(globals),
            compositor,
            output_state: OutputState::new(globals, qh),
            shm,
            layer_shell,
            panes: Vec::new(),
            tint: [0, 0, 0, 0],
            desired,
            quit,
            done,
        })
    }

    /// 每块屏一个铺满的层表面。已经在的屏不重复建。
    fn create_panes(&mut self, qh: &QueueHandle<Self>) {
        for output in self.output_state.outputs() {
            if self.panes.iter().any(|pane| pane.output == output) {
                continue;
            }
            let scale = self
                .output_state
                .info(&output)
                .map(|info| info.scale_factor.max(1))
                .unwrap_or(1);
            let surface = self.compositor.create_surface(qh);
            surface.set_buffer_scale(scale);
            let layer = self.layer_shell.create_layer_surface(
                qh,
                surface.clone(),
                Layer::Overlay,
                Some("zouwu-eye-care"),
                Some(&output),
            );
            layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
            // 0x0 + 四边锚定 = 由合成器决定尺寸（跟着输出分辨率走）。
            layer.set_size(0, 0);
            // 不独占桌面空间：面板/最大化窗口的可用区域一点都不能变。
            layer.set_exclusive_zone(-1);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            // 空白输入区域 = 点击完全穿透（不做这件事的话整屏都会变成"死区"）。
            if let Ok(region) = Region::new(&self.compositor) {
                surface.set_input_region(Some(region.wl_region()));
            }
            layer.commit();
            self.panes.push(Pane {
                output,
                layer,
                surface,
                size: (0, 0),
                scale,
                pool: None,
            });
        }
    }

    fn paint_all(&mut self, qh: &QueueHandle<Self>) {
        for index in 0..self.panes.len() {
            self.paint(qh, index);
        }
    }

    /// 把整块表面填成遮罩色。
    fn paint(&mut self, _qh: &QueueHandle<Self>, index: usize) {
        let tint = self.tint;
        let shm = &self.shm;
        let compositor = &self.compositor;
        let pane = &mut self.panes[index];
        let (lw, lh) = pane.size;
        if lw == 0 || lh == 0 {
            return;
        }
        let scale = pane.scale.max(1) as u32;
        let (w, h) = (lw * scale, lh * scale);
        let stride = (w * 4) as i32;
        let need = (w as usize) * (h as usize) * 4;
        if pane.pool.as_ref().is_none_or(|pool| pool.len() < need) {
            match SlotPool::new(need, shm) {
                Ok(pool) => pane.pool = Some(pool),
                Err(err) => {
                    log::warn!("eye care: slot pool: {err}");
                    return;
                }
            }
        }
        let Some(pool) = pane.pool.as_mut() else {
            return;
        };
        let (buffer, canvas) = match pool.create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888)
        {
            Ok(pair) => pair,
            Err(err) => {
                // 槽位还在合成器手里：这一帧先丢，下一帧再试。
                log::debug!("eye care: buffer busy ({err})");
                return;
            }
        };
        // 预乘 RGBA → 小端 ARGB8888（内存字节序 B,G,R,A）。
        for px in canvas.chunks_exact_mut(4) {
            px[0] = tint[2];
            px[1] = tint[1];
            px[2] = tint[0];
            px[3] = tint[3];
        }
        // 保住输入区域：有些合成器在 commit 新 buffer 后仍按上次的 region 走，
        // 但重新设一次是幂等的、也不贵。
        if let Ok(region) = Region::new(compositor) {
            pane.surface.set_input_region(Some(region.wl_region()));
        }
        if let Err(err) = buffer.attach_to(&pane.surface) {
            log::warn!("eye care: attach: {err}");
            return;
        }
        pane.surface.damage_buffer(0, 0, w as i32, h as i32);
        pane.layer.commit();
        log::debug!("eye care: painted pane {index} {w}x{h}px (alpha {})", tint[3]);
    }

    fn run(&mut self, queue: &mut wayland_client::EventQueue<Self>, qh: &QueueHandle<Self>) {
        loop {
            if self.quit.load(Ordering::Relaxed) {
                break;
            }
            // 换色请求：UI 线程每次 apply 都会塞一个新值并 sync 一次。
            let want = lock(&self.desired).take();
            if let Some(tint) = want {
                if tint != self.tint {
                    self.tint = tint;
                    self.paint_all(qh);
                }
            }
            if let Err(err) = queue.blocking_dispatch(self) {
                log::debug!("eye care: dispatch ended: {err}");
                break;
            }
        }
        // 收摊：把每一块表面都卸下来（attach(None) + commit 即"撤下"）。
        for pane in &self.panes {
            pane.surface.attach(None, 0, 0);
            pane.layer.commit();
        }
        let _ = queue.roundtrip(self);
        self.done.store(true, Ordering::Relaxed);
    }
}

impl OutputHandler for Overlay {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: wl_output::WlOutput) {
        // 热插拔：新屏也要盖住。
        self.create_panes(qh);
        self.paint_all(qh);
    }

    fn update_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if let Some(index) = self.panes.iter().position(|pane| pane.output == output) {
            let scale = self
                .output_state
                .info(&output)
                .map(|info| info.scale_factor.max(1))
                .unwrap_or(1);
            if scale != self.panes[index].scale {
                self.panes[index].scale = scale;
                self.panes[index].surface.set_buffer_scale(scale);
            }
            self.paint(qh, index);
        }
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.panes.retain(|pane| pane.output != output);
    }
}

impl LayerShellHandler for Overlay {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        log::debug!("eye care: overlay closed by compositor");
        let closed = layer.wl_surface().id();
        self.panes.retain(|pane| pane.surface.id() != closed);
        // 一块屏都不剩就没必要留着这个线程了：UI 侧下次 apply 会重建。
        if self.panes.is_empty() {
            self.quit.store(true, Ordering::Relaxed);
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let id = layer.wl_surface().id();
        let Some(index) = self.panes.iter().position(|pane| pane.surface.id() == id) else {
            return;
        };
        self.panes[index].size = configure.new_size;
        log::debug!("eye care: configure pane {index} -> {:?}", configure.new_size);
        self.paint(qh, index);
    }
}

impl ShmHandler for Overlay {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Overlay {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

impl Dispatch<wl_callback::WlCallback, ()> for Overlay {
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        _: <wl_callback::WlCallback as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // 只是个叫醒信号。
    }
}

impl CompositorHandler for Overlay {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if let Some(index) = self.panes.iter().position(|pane| &pane.surface == surface) {
            self.panes[index].scale = new_factor.max(1);
            self.paint(qh, index);
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

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }

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

delegate_compositor!(Overlay);
delegate_output!(Overlay);
delegate_shm!(Overlay);
delegate_layer!(Overlay);
delegate_registry!(Overlay);

#[cfg(test)]
mod tests {
    use super::tint_rgba;

    /// 预乘：alpha=0 必须是全 0（否则合成器按预乘解析会在暗色背景上留下灰雾）。
    #[test]
    fn tint_is_premultiplied() {
        assert_eq!(tint_rgba(0), [0, 0, 0, 0]);
        assert_eq!(tint_rgba(255), [255, 198, 110, 255]);
        let half = tint_rgba(128);
        assert_eq!(half[3], 128);
        assert_eq!(half[0], 128, "255 * 128 / 255");
        assert_eq!(half[1], 99, "198 * 128 / 255");
    }
}