//! winit 事件循环与"变形"状态机。
//!
//! M0 只做一件事：验证"透明窗口 + 一圈气泡 + 悬停 150ms 变形 + 空闲 0 重绘"。
//! 托盘、全局热键、配置面板在 M1 接入，状态与渲染已经按它们的存在设计。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

use zouwu_core::action::{default_ring, ActionItem};
use zouwu_core::anim::Spring;
use zouwu_core::config::{Config, WindowState};
use zouwu_core::geom::{Point, Rect};
use zouwu_core::ring::{solve_rotation, Bubble, RingConfig};
use zouwu_core::theme::{Theme, ZOUWU_WARM};

use crate::paint::{render, BubblePaint, Scene};
use crate::presenter::Presenter;

/// 启动 M0 演示窗口。
pub fn run() -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);

    // 配置在启动时读一次（缺文件会写出一份默认配置，方便用户手改）
    let mut app = App::new(zouwu_platform::load_config(), ZOUWU_WARM, default_ring());
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("run_app: {e}"))?;
    Ok(())
}

/// 一次呼吸起伏的时长（毫秒）
const BREATH_MS: Duration = Duration::from_millis(1200);
/// 纯环境动画（眨眼 / 呼吸）的帧率。交互态另用 60fps。
/// 空闲（无交互）时的帧率上限。
/// 眨眼 180ms、呼吸 ~1.2s 都是“秒级慢变化”，30fps 纯属浪费：
/// 实测常驻 CPU 几乎与“空闲帧率 × 单帧成本”成正比，降到 20fps 肉眼无差。
const AMBIENT_FPS: f32 = 20.0;
/// 光标在窗口外时的轮询间隔：只用来尽早发现“光标进来了”。
///
/// 分层窗口的鼠标命中测试由系统决定，实测在部分环境里根本收不到 `CursorMoved`，
/// 所以除了被动接收窗口事件，我们还得主动问一次光标位置（见 ADR-009）。
/// 一次 `GetCursorPos` 是几微秒，10Hz 的空跑远低于 0.1% CPU。
const CURSOR_POLL_OUTSIDE: Duration = Duration::from_millis(100);
/// 光标在窗口内时的轮询间隔。
/// 必须短于一次真实的按下-松开（~40ms 起），否则 100ms 的轮询会**漏掉点击**（实测）。
/// 代价可忽略：单次 `GetCursorPos` 约 0.6µs，60Hz 轮询 ≈ 0.004% of one core。
const CURSOR_POLL_INSIDE: Duration = Duration::from_millis(16);
/// 同一个物理点击被事件与轮询重复识别时的去重窗口。
const CLICK_DEBOUNCE: Duration = Duration::from_millis(250);

struct App {
    config: Config,
    theme: Theme,
    items: Vec<ActionItem>,

    window: Option<Arc<Window>>,
    presenter: Option<Box<dyn Presenter>>,
    pixmap: Option<Pixmap>,

    /// 逻辑坐标下的光标位置
    cursor: Option<Point>,
    /// 光标进入小工具的时刻（悬停计时起点）
    hover_since: Option<Instant>,
    /// 用户意图：展开 / 收起
    expanded: bool,
    /// 展开动画起点（用于气泡逐个延迟出场）
    expand_started: Option<Instant>,

    ring: RingConfig,
    bubbles: Vec<Bubble>,
    anims: Vec<Spring>,
    buddy_scale: Spring,
    hovered_bubble: Option<usize>,

    blink_start: Option<Instant>,
    /// 下一次眨眼的计划时刻（空闲时的唯一"活着"的证据）
    next_blink: Instant,
    /// 下一次呼吸起伏的计划时刻（`buddy.breathe = true` 时）
    next_breath: Instant,
    /// 呼吸动画的起点
    breath_start: Option<Instant>,

    last_frame: Instant,
    boot: Instant,
    /// 事件导致的"脏"标记：即使弹簧已收敛也要再画一帧
    dirty: bool,
    /// 眨眼/呼吸的抖动种子
    rng: u32,

    /// 下一次轮询光标的时刻
    next_poll: Instant,
    /// 上一次轮询到的按键状态（用于识别按下沿）
    buttons_down: (bool, bool),
    /// 最近一次已处理的点击（事件与轮询去重）
    last_click: Option<Instant>,

    /// 窗口位置：`Moved` 事件只记在这里，节流后再落盘（拖动会刷出大量事件）
    state_pos: Option<(i32, i32)>,
    /// 位置待落盘
    state_pending: bool,
    /// 上一次位置落盘的时刻（节流）
    state_saved_at: Instant,

    /// 诊断开关（环境变量 `ZOUWU_PERF=1`）：周期性上报实际帧率与单帧耗时
    perf_trace: bool,
    perf_frames: u32,
    perf_mark: Instant,
    perf_started: Instant,
}

impl App {
    fn new(config: Config, theme: Theme, items: Vec<ActionItem>) -> Self {
        let ring = config.ring.to_ring_config();
        let n = items.len();
        Self {
            config,
            theme,
            items,
            window: None,
            presenter: None,
            pixmap: None,
            cursor: None,
            hover_since: None,
            expanded: false,
            expand_started: None,
            ring,
            bubbles: Vec::with_capacity(n),
            anims: (0..n).map(|_| Spring::soft(0.0)).collect(),
            buddy_scale: Spring::soft(1.0),
            hovered_bubble: None,
            blink_start: None,
            next_blink: Instant::now() + Duration::from_millis(2600),
            next_breath: Instant::now() + Duration::from_millis(4200),
            breath_start: None,
            last_frame: Instant::now(),
            boot: Instant::now(),
            dirty: true,
            rng: 0x5EED_1234,
            next_poll: Instant::now(),
            buttons_down: (false, false),
            last_click: None,
            state_pos: None,
            state_pending: false,
            state_saved_at: Instant::now(),
            perf_trace: std::env::var("ZOUWU_PERF").is_ok(),
            perf_frames: 0,
            perf_mark: Instant::now(),
            perf_started: Instant::now(),
        }
    }

    fn window_arc(&self) -> Option<Arc<Window>> {
        self.window.clone()
    }

    fn logical_size(&self) -> (f32, f32) {
        let Some(win) = &self.window else {
            let s = self.config.window.size;
            return (s, s);
        };
        let size = win.inner_size();
        let scale = win.scale_factor() as f32;
        (size.width as f32 / scale, size.height as f32 / scale)
    }

    fn viewport(&self) -> Rect {
        let (w, h) = self.logical_size();
        Rect::new(0.0, 0.0, w, h)
    }

    fn center(&self) -> Point {
        let (w, h) = self.logical_size();
        Point::new(w * 0.5, h * 0.5)
    }

    /// 重新计算气泡位置（含避让屏幕边缘的整体旋转）
    fn relayout(&mut self) {
        let center = self.center();
        let view = self.viewport();
        let rotation = solve_rotation(&self.ring, center, view, self.items.len());
        self.bubbles = self.ring.layout(self.items.len(), center, rotation);
    }

    fn set_expanded(&mut self, expanded: bool, now: Instant) {
        if self.expanded == expanded {
            return;
        }
        self.expanded = expanded;
        self.expand_started = Some(now);
        self.dirty = true;
    }

    fn hover_delay(&self) -> Duration {
        Duration::from_millis(self.config.interaction.hover_delay_ms)
    }

    fn update(&mut self, now: Instant) {
        let dt = now
            .duration_since(self.last_frame)
            .as_secs_f32()
            .clamp(0.0, 0.05);
        self.last_frame = now;

        // 0) 输入兜底在 `about_to_wait` 里做（那里每轮都会走一遍，包括定时器唤醒）

        self.relayout();

        // 1) 悬停 → 展开
        if let (Some(since), Some(_)) = (self.hover_since, self.cursor) {
            if !self.expanded && since.elapsed() >= self.hover_delay() {
                self.set_expanded(true, now);
            }
        }

        // 2) 气泡弹簧（逐个延迟出场：stagger）
        let stagger = self.config.ring.expand_stagger_ms;
        let elapsed_ms = self
            .expand_started
            .map(|t| now.duration_since(t).as_millis() as u64)
            .unwrap_or(u64::MAX);
        for (i, spring) in self.anims.iter_mut().enumerate() {
            let gate_open = elapsed_ms >= i as u64 * stagger;
            spring.set_target(if self.expanded && gate_open { 1.0 } else { 0.0 });
            spring.step(dt);
        }

        // 3) 本体悬停微放大
        let hovering_buddy = self.cursor.is_some() && self.hovered_bubble.is_none();
        self.buddy_scale
            .set_target(if hovering_buddy && !self.expanded {
                1.05
            } else {
                1.0
            });
        self.buddy_scale.step(dt);

        // 3.5) 空闲时的"活着"：眨眼与呼吸
        // 只有这些时刻才让常驻窗口付出一点点 CPU，其余时间一帧都不画。
        if now >= self.next_blink {
            self.blink_start = Some(now);
            let wait = self.jitter_ms(3200, 7000);
            self.next_blink = now + Duration::from_millis(wait as u64);
        }
        if self.config.buddy.breathe {
            if now >= self.next_breath && self.breath_start.is_none() {
                self.breath_start = Some(now);
            }
            if let Some(t0) = self.breath_start {
                if now.duration_since(t0) >= BREATH_MS {
                    self.breath_start = None;
                    // 间歇远长于动作时长：呼吸是“偶尔活一下”，不是持续动。
                    // 占空比直接压常驻 CPU（1.2s/12s ≈ 10%）。
                    let wait = self.jitter_ms(9000, 16000);
                    self.next_breath = now + Duration::from_millis(wait as u64);
                }
            }
        }

        // 4) 收起判定：光标远离整圈
        if self.expanded {
            let far = match self.cursor {
                None => true,
                Some(p) => {
                    let center = self.center();
                    let limit = self.ring.radius + self.ring.bubble_radius * 2.0;
                    p.distance(center) > limit * self.config.interaction.collapse_radius_factor
                }
            };
            if far {
                self.set_expanded(false, now);
            }
        }
    }

    /// 呼吸起伏：一个完整的正弦包络，峰值 +2.5% 体积。
    fn breath_amount(&self, now: Instant) -> f32 {
        let Some(t0) = self.breath_start else {
            return 0.0;
        };
        let t = now.duration_since(t0).as_secs_f32() / (BREATH_MS.as_millis() as f32 / 1000.0);
        if t >= 1.0 {
            return 0.0;
        }
        (std::f32::consts::PI * t).sin() * 0.025
    }

    fn breath_active(&self, now: Instant) -> bool {
        self.breath_start
            .map(|t0| now.duration_since(t0) < BREATH_MS)
            .unwrap_or(false)
    }

    /// 不用 rand crate 的轻量抖动（LCG），只为了让眨眼的节奏别太机械。
    fn jitter_ms(&mut self, min: u32, max: u32) -> u32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let span = max.saturating_sub(min).max(1);
        min + (self.rng >> 8) % span
    }

    /// 交互态动画：需要满帧率（弹簧在跑、气泡在出场、画面刚被标记为脏）。
    fn interactive_animation(&self, now: Instant) -> bool {
        if self.dirty {
            return true;
        }
        if self.anims.iter().any(|s| !s.is_settled()) || !self.buddy_scale.is_settled() {
            return true;
        }
        if self.expanded {
            if let Some(t0) = self.expand_started {
                let stagger = self.config.ring.expand_stagger_ms;
                let end = t0 + Duration::from_millis(self.items.len() as u64 * stagger + 700);
                return end > now;
            }
        }
        false
    }

    /// 环境动画：眨眼与呼吸。它们是 1~2 秒尺度的慢变化，30fps 与 60fps 肉眼无差。
    fn ambient_animation(&self, now: Instant) -> bool {
        self.blink_active(now) || self.breath_active(now)
    }

    /// 本帧是否真的有东西要画。
    ///
    /// `RedrawRequested` 可能由系统（重设尺寸、被遮盖后重现）或轮询闹钟触发，
    /// 但这些都不是"画面变了"。空闲时的低开销承诺靠这道门守着：没有脏标记、
    /// 没有在跑的弹簧/眨眼/呼吸、没有待出场的气泡 → 直接跳过光栅化。
    fn needs_render(&self, now: Instant) -> bool {
        self.interactive_animation(now) || self.ambient_animation(now)
    }

    /// 这一帧到下一帧的间隔。
    ///
    /// 纯环境动画降到 30fps：常驻窗口的 CPU 开销直接减半，而慢速缩放/眨眼
    /// 看不出差别（实测单帧光栅约 0.5ms，30fps 下只占一个核的 1.5%）。
    fn frame_interval(&self, now: Instant) -> Duration {
        if self.interactive_animation(now) {
            Duration::from_secs_f32(zouwu_core::FRAME_60FPS)
        } else {
            Duration::from_secs_f32(1.0 / AMBIENT_FPS)
        }
    }

    /// 下一个"必须醒来"的时刻（None = 可以一直睡下去）。
    ///
    /// 关键坑：动画的 60fps 节拍**不能**写成 `now + 16ms` —— 事件循环会因为
    /// 任何原因醒来，每次醒来都重新算一个"16ms 之后"，deadline 就永远追不上，
    /// 于是 request_redraw 一次都不发、动画卡死、循环却空转在 60Hz。
    /// 所以节拍锚在 `last_frame`（上一次真正渲染的时刻），它只在渲染时前进。
    fn next_deadline(&self, now: Instant) -> Option<Instant> {
        if self.dirty {
            return Some(now);
        }
        let mut deadline: Option<Instant> = None;
        let mut consider = |t: Instant| {
            deadline = Some(match deadline {
                Some(cur) => cur.min(t),
                None => t,
            });
        };

        // 光标轮询闹钟：不是为了重绘，而是为了定期问一次光标位置。
        // 注意必须排在 `dirty` 后面、并且在上面的轮询之后计算，否则会反复到期。
        consider(self.next_poll);
        // 悬停满 hover_delay 要展开：必须定个闹钟把自己叫醒
        if !self.expanded {
            if let Some(since) = self.hover_since {
                consider(since + self.hover_delay());
            }
        }
        // 有东西在动（弹簧 / 气泡出场 / 眨眼 / 呼吸）→ 排下一帧
        if self.needs_render(now) {
            consider(self.last_frame + self.frame_interval(now));
        }
        // 空闲时的计划：下一次眨眼 / 下一次呼吸
        consider(self.next_blink);
        // 只在意"下一次呼吸什么时候开始"：正在呼吸时 next_breath 还停在过去，
        // 把它算进来会让 deadline 永远过期 → 每轮都 request_redraw（实测 380fps）。
        if self.config.buddy.breathe && self.breath_start.is_none() {
            consider(self.next_breath);
        }
        deadline
    }

    fn blink_active(&self, now: Instant) -> bool {
        self.blink_start
            .map(|t| now.duration_since(t) < Duration::from_millis(180))
            .unwrap_or(false)
    }

    fn blink_amount(&self, now: Instant) -> f32 {
        // 120ms 闭、60ms 睁，用三角波近似，足够自然且零成本
        let Some(t0) = self.blink_start else {
            return 0.0;
        };
        let ms = now.duration_since(t0).as_millis() as f32;
        if ms >= 180.0 {
            0.0
        } else if ms < 120.0 {
            ms / 120.0
        } else {
            (180.0 - ms) / 60.0
        }
    }

    fn hover_test(&self, p: Point) -> Option<usize> {
        if !self.expanded {
            return None;
        }
        self.ring
            .hit_test(&self.bubbles, p, self.config.interaction.hit_tolerance)
    }

    fn on_cursor_moved(&mut self, logical: Point, now: Instant) {
        self.cursor = Some(logical);
        self.relayout();

        let center = self.center();
        let inside_buddy = logical.distance(center)
            <= self.config.buddy.radius * self.config.interaction.collapse_radius_factor;

        let new_hover = self.hover_test(logical);
        if new_hover != self.hovered_bubble {
            self.hovered_bubble = new_hover;
            self.dirty = true;
        }

        if inside_buddy {
            if self.hover_since.is_none() {
                self.hover_since = Some(now);
                self.blink_start = Some(now);
                self.dirty = true;
            }
        } else if self.hover_since.is_some() && !self.expanded {
            self.hover_since = None;
            self.dirty = true;
        }
    }

    /// 位置落盘（节流 1s）。`Moved` 在拖动时可能每秒刷出上百个事件，
    /// 每次都原子写文件既浪费又伤磁盘。
    fn flush_state(&mut self, now: Instant) {
        if !self.state_pending || now.duration_since(self.state_saved_at) < Duration::from_secs(1) {
            return;
        }
        self.save_state();
    }

    fn save_state(&mut self) {
        if !self.state_pending {
            return;
        }
        let Some((x, y)) = self.state_pos else { return };
        zouwu_platform::save_window_state(WindowState { x, y });
        self.state_pending = false;
        self.state_saved_at = Instant::now();
    }

    /// 按自适应间隔轮询一次光标，并安排下一次轮询。
    fn poll_now(&mut self, now: Instant) {
        let inside = self.poll_pointer(now);
        // 窗口内高频（不漏点击）、窗口外低频（省 CPU）
        self.next_poll = now
            + if inside {
                CURSOR_POLL_INSIDE
            } else {
                CURSOR_POLL_OUTSIDE
            };
    }
    /// 主动问系统要光标/按键状态，作为窗口鼠标事件的兑底。
    ///
    /// 返回光标是否落在窗口矩形内。坐标换算：`GetCursorPos` 给的是屏幕物理像素，
    /// 减去窗口左上角（也是物理像素）再除以 DPI 缩放就是本地逻辑坐标。
    fn poll_pointer(&mut self, now: Instant) -> bool {
        let (Some((sx, sy)), Some(window)) = (
            zouwu_platform::layer::cursor_position(),
            self.window.clone(),
        ) else {
            return false;
        };

        let scale = window.scale_factor() as f32;
        let origin = window.outer_position().unwrap_or_default();
        let local = Point::new(
            (sx - origin.x) as f32 / scale,
            (sy - origin.y) as f32 / scale,
        );
        let size = window.inner_size();
        let (w, h) = (size.width as f32 / scale, size.height as f32 / scale);
        let inside = local.x >= 0.0 && local.y >= 0.0 && local.x < w && local.y < h;

        if !inside {
            if self.cursor.is_some() {
                self.on_cursor_left();
            }
            self.buttons_down = (false, false);
            return false;
        }

        let moved = match self.cursor {
            Some(prev) => prev.distance_sq(local) > 0.25,
            None => true,
        };
        if moved {
            self.on_cursor_moved(local, now);
        }

        // 点击兑底：窗口收不到 `MouseInput` 时靠 `GetAsyncKeyState` 识别按下沿
        let left = zouwu_platform::layer::left_button_down();
        let right = zouwu_platform::layer::right_button_down();
        if left && !self.buttons_down.0 {
            self.click(MouseButton::Left, now);
        }
        if right && !self.buttons_down.1 {
            self.click(MouseButton::Right, now);
        }
        self.buttons_down = (left, right);
        true
    }

    /// 统一的点击入口：同一个物理点击可能同时从轮询与窗口事件到达，去重。
    fn click(&mut self, button: MouseButton, now: Instant) {
        if let Some(last) = self.last_click {
            if now.duration_since(last) < CLICK_DEBOUNCE {
                return;
            }
        }
        self.last_click = Some(now);
        self.on_click(button, now);
    }

    fn on_cursor_left(&mut self) {
        self.cursor = None;
        self.hover_since = None;
        if self.hovered_bubble.take().is_some() {
            self.dirty = true;
        }
    }

    fn on_click(&mut self, button: MouseButton, now: Instant) {
        let Some(pos) = self.cursor else { return };
        let center = self.center();
        let on_buddy = pos.distance(center) <= self.config.buddy.radius;

        match button {
            MouseButton::Left => {
                if let Some(idx) = self.hover_test(pos) {
                    if let Some(item) = self.items.get(idx) {
                        // M0：先验证交互链路，真正的能力在 M3 接入
                        log::info!("bubble activated: {} ({})", item.id, item.label_key);
                    }
                } else if on_buddy {
                    // 点击 = 立即展开/收起（ADR-002）
                    self.set_expanded(!self.expanded, now);
                    self.hover_since = Some(now);
                }
            }
            MouseButton::Right => {
                // M1 换成托盘友好的上下文菜单；M0 先收起
                self.set_expanded(false, now);
            }
            _ => {}
        }
    }

    /// 诊断：`ZOUWU_PERF=1` 时每 60 帧上报一次实际帧率。
    /// 用来验收“空闲开销”这个指标：空闲时应当只有眨眼/呼吸在动，
    /// 会话帧率应当远低于交互时的 60fps。
    fn perf_trace_frame(&mut self, now: Instant) {
        if !self.perf_trace {
            return;
        }
        self.perf_frames += 1;
        if self.perf_frames % 60 == 0 {
            let span = now.duration_since(self.perf_mark).as_secs_f32();
            let total = now.duration_since(self.perf_started).as_secs_f32();
            log::info!(
                "perf: last 60 frames in {span:.2}s = {:.1} fps; session avg {:.1} fps over {total:.1}s",
                60.0 / span.max(1e-6),
                self.perf_frames as f32 / total.max(1e-6),
            );
            self.perf_mark = now;
        }
    }

    fn render_frame(&mut self, now: Instant) -> Result<(), String> {
        let Some(win) = self.window_arc() else {
            return Ok(());
        };
        let physical = win.inner_size();
        let (pw, ph) = (physical.width.max(1), physical.height.max(1));
        let scale = win.scale_factor() as f32;
        let (width, height) = self.logical_size();

        let need_new = match &self.pixmap {
            Some(p) => p.width() != pw || p.height() != ph,
            None => true,
        };
        if need_new {
            self.pixmap = Pixmap::new(pw, ph);
            if let Some(p) = &mut self.pixmap {
                p.fill(tiny_skia::Color::TRANSPARENT);
            }
        }
        let center = self.center();
        let mut paints = Vec::with_capacity(self.items.len());
        for (i, (bubble, spring)) in self.bubbles.iter().zip(self.anims.iter()).enumerate() {
            let scale_v = spring.value.max(0.0);
            let alpha = (scale_v).clamp(0.0, 1.0);
            // 极轻微的呼吸漂移（2px 幅度），用绝对时间计算，静止时可收敛
            let phase = bubble.float_phase
                + now.duration_since(self.boot).as_secs_f32() * std::f32::consts::TAU
                    / self.config.ring.float_period_s.max(0.5);
            let drift = self.config.ring.float_amplitude * phase.sin();
            paints.push(BubblePaint {
                center: Point::new(bubble.center.x, bubble.center.y + drift * 0.5),
                radius: bubble.radius,
                scale: scale_v,
                alpha,
                glyph: self.items[i].glyph,
                selected: self.hovered_bubble == Some(i),
            });
        }

        let scene = Scene {
            width,
            height,
            scale,
            buddy_center: center,
            buddy_radius: self.config.buddy.radius,
            buddy_scale: self.buddy_scale.value + self.breath_amount(now),
            buddy_alpha: 1.0,
            blink: self.blink_amount(now),
            theme: &self.theme,
            bubbles: &paints,
        };
        let Some(pixmap) = self.pixmap.as_mut() else {
            return Err("pixmap alloc failed".into());
        };
        render(&scene, pixmap);

        if let Some(presenter) = self.presenter.as_mut() {
            presenter.present(pixmap)?;
        }
        self.dirty = false;

        self.perf_trace_frame(now);
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let size = self.config.window.size;
        let attrs = Window::default_attributes()
            .with_title("zouwu")
            .with_inner_size(LogicalSize::new(size, size))
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(true);

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(err) => {
                log::error!("create window failed: {err}");
                event_loop.exit();
                return;
            }
        };

        // 位置：优先回到上次的位置；没有记录时停在主屏右侧偏下的"宠物位"
        let restored = if self.config.window.remember_position {
            zouwu_platform::load_window_state()
        } else {
            None
        };
        let pos = restored.map(|WindowState { x, y }| (x, y)).or_else(|| {
            let monitor = window
                .current_monitor()
                .or_else(|| event_loop.primary_monitor())?;
            let msize = monitor.size();
            let mpos = monitor.position();
            let wsize = window.inner_size();
            Some((
                mpos.x + msize.width as i32 - wsize.width as i32 - 48,
                mpos.y + (msize.height as f32 * 0.55) as i32 - wsize.height as i32 / 2,
            ))
        });
        if let Some((x, y)) = pos {
            window.set_outer_position(PhysicalPosition::new(x, y));
            self.state_pos = Some((x, y));
        }

        let physical = window.inner_size();
        match crate::presenter::create(&window, physical.width, physical.height) {
            Ok(p) => self.presenter = Some(p),
            Err(err) => {
                log::error!("presenter init failed: {err}");
                event_loop.exit();
                return;
            }
        }

        self.window = Some(window);
        self.relayout();
        self.last_frame = Instant::now();
        self.dirty = true;
        if let Some(w) = self.window_arc() {
            w.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let now = Instant::now();
        match event {
            WindowEvent::RedrawRequested => {
                self.update(now);
                if self.needs_render(now) {
                    if let Err(err) = self.render_frame(now) {
                        log::error!("render failed: {err}");
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self
                    .window
                    .as_ref()
                    .map(|w| w.scale_factor() as f32)
                    .unwrap_or(1.0)
                    .max(0.01);
                let p = Point::new(position.x as f32 / scale, position.y as f32 / scale);
                self.on_cursor_moved(p, now);
            }
            WindowEvent::CursorLeft { .. } => {
                self.on_cursor_left();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed {
                    self.click(button, now);
                }
            }
            WindowEvent::Resized(new_size) => {
                if let Some(p) = self.presenter.as_mut() {
                    p.resize(new_size.width.max(1), new_size.height.max(1));
                }
                self.dirty = true;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let Key::Named(NamedKey::Escape) = event.logical_key {
                        self.save_state();
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::Moved(pos) => {
                if self.config.window.remember_position {
                    self.state_pos = Some((pos.x, pos.y));
                    self.state_pending = true;
                }
            }
            WindowEvent::CloseRequested => {
                self.save_state();
                event_loop.exit();
            }
            _ => {}
        }
    }

    /// 空闲调度：把「下一次必须重绘的时刻」交给事件循环，到点自动醒来。
    /// 没有待办时进入 `Wait`（真正 0 帧、0 CPU），有动画/悬停计时/眨眼计划时
    /// 进入 `WaitUntil`。这是"空闲零开销"与"看起来活着"能同时成立的关键。
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();

        // 轮询只是一种"叫醒输入"，它本身不应该产生任何重绘：
        // 开一枪 GetCursorPos 几微秒，醒来发现画面没变就直接接着睡。
        if now >= self.next_poll {
            self.poll_now(now);
        }

        self.flush_state(now);

        match self.next_deadline(now) {
            Some(t) if t <= now => {
                if let Some(w) = self.window_arc() {
                    w.request_redraw();
                }
                // 不用 Wait：万一有哪个 deadline 因为状态没推进而反复到期，
                // 16ms 的军底定时器能把最坏情况钳在 ~60Hz，不会空转转满一个核。
                event_loop
                    .set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(16)));
            }
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}
