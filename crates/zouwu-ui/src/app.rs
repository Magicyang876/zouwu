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

use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

// 托盘应用的窗口不该占任务栏位置。winit 把这条挂在 Windows 扩展 trait 上，
// 只能按平台引入（ADR-001 允许在“窗口属性”这种边界上做最小 cfg）。
#[cfg(target_os = "windows")]
use winit::platform::windows::WindowAttributesExtWindows;

use zouwu_core::action::ActionItem;
use zouwu_core::anim::Spring;
use zouwu_core::beam::{self, Beam, BeamInput};
use zouwu_core::config::{ChangeKind, Config, WindowState};
use zouwu_core::drag::{Drag, DragOutcome};
use zouwu_core::geom::{Point, Rect};
use zouwu_core::history::{History, Push};
use zouwu_core::hotkey::Hotkey;
use zouwu_core::i18n::I18n;
use zouwu_core::qr;
use zouwu_core::ring::{solve_rotation, Bubble, RingConfig};
use zouwu_core::skin::PROCEDURAL_SKIN_ID;

use crate::history::{HistoryWindow, Outcome as HistoryOutcome};
use crate::settings_window::{Outcome as SettingsOutcome, SettingsWindow};
use crate::skin::Skin;
use zouwu_core::theme::Theme;

use zouwu_platform::autostart;
use zouwu_platform::hotkey::{HotkeyRegistrar, Slot};
use zouwu_platform::reload::{ConfigWatcher, ReloadEvent};
use zouwu_platform::single_instance::InstanceSignal;
use zouwu_platform::tray::{Tray, TrayCommand, TrayOptions};

use crate::paint::{render, BeamPaint, BubblePaint, Scene, CHIP_FONT};

/// 开激光条时窗口往外扩的余量（逻辑像素，单侧）。
///
/// 牌子是横排文字，绕着一颗球往外射，需要的空间比气泡环大得多：默认 340 的
/// 正方形窗口里，环外只剩十几像素，牌子必然挤成一堆。所以开激光条就把窗口
/// 撑大一圈、关掉再收回——空闲时不占地方。
const BEAM_ROOM: f32 = 18.0;
/// 排在“理想大窗口”里用的假余量：先不算窗口限制，排出真实需求再撑窗口。
const BEAM_SEED: f32 = 400.0;
use crate::presenter::Presenter;
use crate::report::{Entry, Outcome as ReportOutcome, Report, ReportWindow};

/// 启动 zouwu。
///
/// 顺序有讲究：
/// 1. **先抢单实例**，抢不到就通知已在跑的那个并立刻返回（用户双击两次时
///    不应该出现两只驺吾）；
/// 2. 再建事件循环；3. 读配置决定语言/热键/是否直接显示。
pub fn run() -> Result<(), String> {
    // 日志要**最先**起来：后面每一步都可能失败，而失败的原因只在这里留得下。
    // 注意此时还没读配置，所以先按 RUST_LOG / 默认 info 起，读完配置再 apply。
    zouwu_platform::logging::init();
    log::info!(
        "zouwu {} starting ({} / {})",
        env!("CARGO_PKG_VERSION"),
        zouwu_platform::platform_name(),
        std::env::consts::ARCH
    );

    // `ZOUWU_INSTANCE` 让并行的调试实例不与正式实例互相"叫醒"：
    // 单实例守卫按名字区分，换个名字就能同时开两只驺吾。
    let instance_id = std::env::var("ZOUWU_INSTANCE").unwrap_or_else(|_| "zouwu".into());
    let instance = InstanceSignal::acquire(&instance_id)?;
    let Some(instance) = instance else {
        log::info!("another zouwu is running; asked it to show up");
        return Ok(());
    };

    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);

    // 官方皮肤随程序内嵌发布，首次启动（或用户删掉后）落到磁盘上，
    // 之后就以磁盘那份为准——用户改配色/换图不会被下一次启动覆盖。
    // 必须排在读配置之前：配置里可能就指名了某个内置皮肤。
    zouwu_platform::assets::ensure_builtin_skins();
    // 字体包必须在任何文本绘制之前注册（设置面板、托盘提示都要用）。
    zouwu_platform::fontpack::register_all();

    // 配置在启动时读一次（缺文件会写出一份默认配置，方便用户手改）
    let config = zouwu_platform::load_config();
    // 配置里的日志级别/落盘策略现在才生效（RUST_LOG 仍然优先）。
    zouwu_platform::logging::apply(&config.log);
    // 卡死是"看不见现场"的问题，看门狗从这一刻起就一直盯着主线程卡在哪。
    crate::watchdog::spawn();
    // 主题/皮肤都是"用户可能写错"的输入：加载失败一律退回内置，不让程序打不开。
    let themes = zouwu_platform::assets::load_themes();
    let theme = zouwu_platform::assets::theme_by_id(&themes, &config.appearance.theme);
    let mut app = App::new(config, theme, themes, instance);
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

/// 单边物理像素上限：超过就缩窗口。
/// 1200² ≈ 1.44 Mpx，一帧 RGBA 约 5.8 MB；再大光栅+合成会明显拖慢。
const MAX_PHYSICAL_SIDE: u32 = 1200;

/// 至少要有这么多比例的面积落在真实屏幕上，才认为「这个位置还活着」。
///
/// 场景：笔记本插着外接屏时把驺吾放到外接屏上，拔掉外接屏后那个坐标就落在
/// 不存在的桌面上——窗口还在、进程活着、托盘也在，但用户什么都看不见，
/// 而且重启也回不来（坏坐标被记进了 state.toml）。
///
/// 0.5 是刻意的宽：允许小工具被屏幕边缘切掉一半，不允许它整个消失。
const MIN_VISIBLE_RATIO: f32 = 0.5;

/// 多久检查一次「我还在屏幕上吗」。
///
/// 拔屏、改分辨率不会通知我们（winit 不暴露 WM_DISPLAYCHANGE），
/// 但枚举显示器只要几微秒，所以跟光标轮询同节拍做一次就够。
const MONITOR_CHECK: Duration = Duration::from_secs(3);

/// 重新落位时离屏幕右下边缘的距离。
const EDGE_MARGIN: f32 = 48.0;

/// 气泡环里「二维码」那一项的动作 id。
///
/// 与 `zouwu_core::action::default_ring()` 里的 `builtin.qrcode` 对应：
/// 气泡只负责"说出用户点了哪一项"，真正的执行在这里分派。
const QRCODE_ACTION: &str = "builtin.qrcode";
/// 剪贴板历史气泡的 id。
const CLIPBOARD_ACTION: &str = "builtin.clipboard";

/// 剪贴板序号轮询间隔。
///
/// `GetClipboardSequenceNumber` 只是读一个计数器（约 0.1 µs），250ms 一次
/// 的代价完全不可测。再快没有意义：人不会 250ms 内连复两次。
const CLIPBOARD_POLL: Duration = Duration::from_millis(250);

struct App {
    config: Config,
    theme: Theme,
    /// 全部可用主题（内置 + 外部）。托盘勾选、换主题都从这里取。
    themes: Vec<Theme>,
    /// 当前皮肤（`procedural` 时没有图层，走代码绘制）
    skin: Skin,
    items: Vec<ActionItem>,
    /// 自定义球的图标，下标与 [`App::items`] 对齐；内置球恒为 `None`。
    icons: Vec<Option<Pixmap>>,

    window: Option<Arc<Window>>,
    presenter: Option<Box<dyn Presenter>>,
    pixmap: Option<Pixmap>,

    /// 逻辑坐标下的光标位置
    cursor: Option<Point>,
    /// 光标进入小工具的时刻（悬停计时起点）
    hover_since: Option<Instant>,
    /// 光标「离开环带」的起始时刻。收起不是瞬时的：要连续远离 `collapse_delay_ms`
    /// 才真的收，否则手从本体划向气泡的半路上就会被判死亡。
    far_since: Option<Instant>,
    /// 用户意图：展开 / 收起
    expanded: bool,
    /// 展开动画起点（用于气泡逐个延迟出场）
    expand_started: Option<Instant>,

    ring: RingConfig,
    bubbles: Vec<Bubble>,
    anims: Vec<Spring>,
    buddy_scale: Spring,
    hovered_bubble: Option<usize>,

    /// 哪颗球的激光条正开着（下标指向 `items`）
    open_children: Option<usize>,
    /// 激光条的几何（只在 `open_children` 有值时非空）
    beams: Vec<Beam>,
    /// 鼠标停在哪条激光条的文字牌上
    hovered_child: Option<usize>,
    /// 激光条的生长弹簧（0 = 收，1 = 完全射出）
    child_anim: Spring,
    /// 呼出即展开后的"钉住"状态：不因鼠标离远自动收起，点一下本体才收。
    pinned: bool,

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
    /// 拖动进行中的状态（按住本体 → 越过阈值 → 窗口跟着光标走）
    drag: Option<Drag>,

    /// 窗口位置：`Moved` 事件只记在这里，节流后再落盘（拖动会刷出大量事件）
    state_pos: Option<(i32, i32)>,
    /// 托盘点了「回到主屏」：窗口句柄只有 `about_to_wait` 里拿得到，先记个意愿
    pending_recenter: bool,
    /// 上一次检查显示器的时刻
    last_monitor_check: Instant,
    /// 上一次打「输入诊断」日志的时刻（`ZOUWU_INPUT=1` 才用）。
    last_input_trace: Instant,
    /// 位置待落盘
    state_pending: bool,
    /// 上一次位置落盘的时刻（节流）
    state_saved_at: Instant,

    /// 诊断开关（环境变量 `ZOUWU_PERF=1`）：周期性上报实际帧率与单帧耗时
    // ---- 常驻形态（M1）：托盘、热键、单实例、语言 ----
    i18n: I18n,
    /// 单实例守卫：放着别动，drop 就等于放开"我在跑"的声明。
    instance: Option<InstanceSignal>,
    /// 下一次启动的信号（第二实例 → 呼出窗口）。
    tray: Option<Tray>,
    hotkeys: Option<HotkeyRegistrar>,
    /// 待执行的动作（点击气泡时记下，下一轮事件循环里执行）。
    ///
    /// 不直接在 `on_click` 里执行：有些动作要建窗口，而建窗口需要
    /// `&ActiveEventLoop`，那个引用只在 `about_to_wait` / `window_event` 里拿得到。
    pending_action: Option<String>,
    /// 激光条里点中的子项：`(父球下标, 子项下标)`。与 `pending_action` 一样
    /// 只是"记下来"，真正执行要等 `about_to_wait` 拿到 `&ActiveEventLoop`。
    pending_child: Option<(usize, usize)>,
    /// 结果小窗（二维码扫描结果等）。
    report: Option<ReportWindow>,
    /// 剪贴板历史窗。
    history_window: Option<HistoryWindow>,
    /// 剪贴板历史数据。
    history: History,
    /// 上一次看到的剪贴板序号（`None` = 还没有基线）。
    clipboard_seq: Option<u32>,
    next_clipboard: Instant,
    /// 窗口当前是否可见（隐藏 = 缩进托盘，进程仍在）。
    visible: bool,
    /// 上一次子窗口（设置/历史/报告）关闭的时刻。
    ///
    /// 用户在子窗口按 Esc 关闭时，Win32 前台切换可能把正在按下的 Esc 键事件
    /// 泄漏给刚拿到焦点的主窗口。500ms 内到达主窗口的 Esc 绝不能把小兽藏进托盘。
    last_child_closed_at: Instant,

    perf_trace: bool,
    perf_frames: u32,
    perf_mark: Instant,
    perf_started: Instant,

    /// 配置文件监视器。借已有的输入轮询节拍顺手看一眼 `config.toml`，
    /// 不另开线程、不用 `notify`（理由见 `zouwu_platform::reload`）。
    config_watcher: ConfigWatcher,

    /// 设置面板。是**第二个真窗口**（托盘/热键可能把它叫出来，用户随手关掉）。
    settings: Option<SettingsWindow>,
    /// 托盘点了"设置"，但当时手里没有 `ActiveEventLoop`（不能在轮询里建窗口），
    /// 于是先记个意愿，下一拍再开。
    pending_settings: bool,
    /// `ZOUWU_SETTINGS=1` 已消费过（一次性）。
    settings_env_done: bool,
    /// `ZOUWU_SCAN=1` 启动即扫一次二维码（只一次）。托盘与热键都没法脚本化，
    /// 而"截屏→找码→弹窗"这条链路最该被一键回归。
    scan_env_done: bool,
    /// `ZOUWU_HISTORY=1` 启动即开一次剪贴板历史窗（只一次）。
    history_env_done: bool,
    /// 待销毁的子窗口。
    ///
    /// **不要在 winit 的事件回调里 drop 掉 `Window`**：Windows 下析构会
    /// `DestroyWindow`，那会同步派发窗口消息、重入事件派发；而此刻主窗事件
    /// 回调还没返回，重入一撞上就死锁。表现是“开过 A 再来 B 就卡死”。
    /// 所以关闭只 mark 一下，把结构体挪进这个墓地，统一在 `about_to_wait`
    /// （事件派发已结束）里清掉。
    retired: Vec<Box<dyn std::any::Any>>,
    /// `ZOUWU_CYCLE=<n>`：自动重复 n 轮"开历史→关历史→开二维码→关二维码"。
    ///
    /// 这份代码存在的唯一理由是：用户报的卡死在本机用合成按键复现不出来
    /// （`SendKeys` 不会触发 `RegisterHotKey`），只能让程序自己把那条序列走一遍。
    cycle_left: u32,
    cycle_step: u8,
    cycle_next: Option<std::time::Instant>,
}

/// 把自定义球的图标都解成位图，下标与 `items` 对齐。
///
/// 解码失败就退回 `None`（画字形）：用户选了一张坏 PNG，
/// 不该连球一起消失。读文件放启动时做一次，绘制帧不碰磁盘。
fn load_icons(items: &[ActionItem]) -> Vec<Option<Pixmap>> {
    items
        .iter()
        .map(|item| {
            let name = item.icon.as_deref()?;
            let path = zouwu_platform::assets::resolve_icon(name)?;
            match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| Pixmap::decode_png(&bytes).map_err(|e| e.to_string()))
            {
                Ok(pixmap) => Some(pixmap),
                Err(err) => {
                    log::warn!("icon `{name}` unusable ({err}); falling back to glyph");
                    None
                }
            }
        })
        .collect()
}

/// 读并解码当前皮肤。任何一步失败都退回 `procedural`：
///
/// 皮肤是用户的资源，坏了该少一层皮，不该让常驻程序起不来。
fn load_skin(id: &str) -> Skin {
    if id == PROCEDURAL_SKIN_ID {
        return Skin::procedural();
    }
    let decoded = zouwu_platform::assets::load_skin(id).and_then(|pkg| Skin::decode(&pkg));
    match decoded {
        Ok(skin) => {
            log::info!(
                "skin loaded: {} ({})",
                skin.manifest.id,
                skin.manifest.display_name()
            );
            skin
        }
        Err(err) => {
            log::warn!("skin `{id}` unavailable, using procedural: {err}");
            Skin::procedural()
        }
    }
}

impl App {
    /// 交给渲染层的皮肤引用（`procedural` 也照样传，由 paint 决定走哪条路）。
    fn skin_for_paint(&self) -> Option<&Skin> {
        Some(&self.skin)
    }

    fn new(config: Config, theme: Theme, themes: Vec<Theme>, instance: InstanceSignal) -> Self {
        let ring = config.ring.to_ring_config();
        let skin = load_skin(&config.appearance.skin);
        // 历史容量来自配置；从磁盘读出来的条目要按它裁一遍，
        // 否则用户把上限从 100 改成 10 之后，旧条目会一直赖着。
        let mut history = zouwu_platform::load_history();
        history.set_limit(config.clipboard.limit);
        let i18n = I18n::new(&config.general.language);
        // 环上显示哪几颗、什么顺序，完全看配置（ADR-042）。
        let items = zouwu_core::action::resolve(&config.ring.items, &i18n);
        let icons = load_icons(&items);
        let n = items.len();
        let visible = config.general.show_on_start;
        Self {
            i18n,
            instance: Some(instance),
            tray: None,
            hotkeys: None,
            pending_action: None,
            pending_child: None,
            report: None,
            history_window: None,
            history,
            // 启动时**只记基线**，不把当前剪贴板内容当成"刚复制"。
            clipboard_seq: zouwu_platform::clipboard::sequence(),
            next_clipboard: Instant::now(),
            visible,
            config,
            theme,
            themes,
            skin,
            items,
            icons,
            window: None,
            presenter: None,
            pixmap: None,
            cursor: None,
            hover_since: None,
            far_since: None,
            expanded: false,
            expand_started: None,
            ring,
            bubbles: Vec::with_capacity(n),
            anims: (0..n).map(|_| Spring::soft(0.0)).collect(),
            buddy_scale: Spring::soft(1.0),
            hovered_bubble: None,
            open_children: None,
            beams: Vec::new(),
            hovered_child: None,
            child_anim: Spring::soft(0.0),
            pinned: false,
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
            drag: None,
            state_pos: None,
            pending_recenter: false,
            last_monitor_check: Instant::now(),
            last_input_trace: Instant::now(),
            state_pending: false,
            state_saved_at: Instant::now(),
            last_child_closed_at: Instant::now() - Duration::from_secs(10),
            perf_trace: std::env::var("ZOUWU_PERF").is_ok(),
            perf_frames: 0,
            perf_mark: Instant::now(),
            perf_started: Instant::now(),
            config_watcher: ConfigWatcher::new(zouwu_platform::config_path()),
            settings: None,
            pending_settings: false,
            settings_env_done: false,
            scan_env_done: false,
            history_env_done: false,
            retired: Vec::new(),
            cycle_left: std::env::var("ZOUWU_CYCLE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            cycle_step: 0,
            cycle_next: None,
        }
    }

    /// 把子窗口推入墓地，等 `about_to_wait` 里再真正析构。
    ///
    /// 见 `retired` 字段的注释：在事件回调里析构 `Window` 会重入事件派发。
    fn retire<T: 'static>(&mut self, window: T) {
        log::info!("app: retire -> queue ({} pending)", self.retired.len() + 1);
        self.retired.push(Box::new(window));
    }

    fn window_arc(&self) -> Option<Arc<Window>> {
        self.window.clone()
    }

    /// 平台的原始窗口句柄（Windows 下是 HWND）。
    ///
    /// 托盘/显隐/置前这些都是**原生窗口概念**，所以在这里取一次句柄交给
    fn hwnd(&self) -> Option<isize> {
        let window = self.window.as_ref()?;
        let handle = window.window_handle().ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        }
    }

    /// 用完就放：把配置写回磁盘（语言、热键、自启这些用户改动）。
    fn save_config(&self) {
        // 我们正要覆盖磁盘上的文件：如果那上面还有没被读进来的手改，至少留个凭据。
        if self.config_watcher.has_external_change() {
            log::warn!("overwriting an external edit to config.toml");
        }
        zouwu_platform::save_config(&self.config);
        // 把监视器的戳对齐到“我刚写的内容”，否则下一次轮询会把我们自己的写
        // 当成外部修改再应用一遍（结果虽然幂等，但会白重画一帧、白写一条日志）。
        self.config_watcher.sync();
    }

    /// 把磁盘上新读到的配置应用到正在运行的对象上。
    ///
    /// 原则：**能热改的立刻改，改不了的安静跳过，坏文件一律不动现状**。
    /// 每一步都幂等，也不因为某一步失败就回滚整体 —— 宁可热键没换成功，
    /// 也不要让一只好好的驺吾退回旧配置。
    fn reload_config(&mut self, next: Config, now: Instant) {
        if self.config == next {
            return;
        }
        let changed = self.config.diff(&next);
        self.apply_config(next, &changed, now, "disk");
    }

    /// 把一份新配置接到运行期对象上。`origin` 只用于日志。
    ///
    /// 两条路径共用它：磁盘热重载（`reload_config`）与设置面板（用户在面板里
    /// 改一个开关）。共用而不是各写一遍，是因为"哪些字段要热应用"是**同一份
    /// 领域知识**，分叉两次必然有一边漏掉某个 `ChangeKind`。
    fn apply_config(
        &mut self,
        mut next: Config,
        changed: &[ChangeKind],
        now: Instant,
        origin: &str,
    ) {
        if changed.is_empty() {
            // 只改了“启动期才读”的字段（`show_on_start` 之类）：接管配置即可。
            log::debug!("config replaced from {origin} (no live effect)");
            self.config = next;
            return;
        }

        // 开机自启是唯一会把系统状态回填进配置的一项：先问系统真实结果，
        // 再接管配置。回填只改内存，**不写盘**（写盘会整份重写用户的注释和格式）。
        if changed.contains(&ChangeKind::Autostart) {
            let actual = autostart::set_enabled(next.general.start_on_boot);
            if actual != next.general.start_on_boot {
                log::warn!(
                    "autostart requested {} but system reports {}",
                    next.general.start_on_boot,
                    actual
                );
                next.general.start_on_boot = actual;
            }
        }

        self.config = next;

        if changed.contains(&ChangeKind::Language)
            && self.i18n.set_language(&self.config.general.language)
        {
            if let Some(tray) = &self.tray {
                tray.apply_language(&self.i18n, &self.config.general.language, self.visible);
            }
            // 内置球的名字来自 i18n：换语言要重新解一遍，否则环上还是旧语言。
            self.resolve_items();
            if self.window.is_some() {
                self.relayout();
            }
        }

        if changed.iter().any(|kind| touches_hotkeys(*kind)) {
            // 球的增删、改序、开关、每颗球自己的热键，全部归在 `ChangeKind::Ring`：
            // 它们同样会改变“哪些槽位应该被注册”。只盯 `Hotkey` 的话，面板里改了
            // 球的键只会存盘、不生效（看起来像“设置不保存”）。
            self.reapply_hotkeys();
        }

        if changed.contains(&ChangeKind::Autostart) {
            if let Some(tray) = &self.tray {
                tray.set_autostart_checked(self.config.general.start_on_boot);
            }
        }

        if changed.contains(&ChangeKind::Clipboard) {
            // 开关/容量/清空都在设置面板里改，但数据归这里管：重读一遍磁盘，
            // 以文件为唯一真相。改容量时 `set_limit` 会顺手裁掉超出的旧条目。
            self.history = zouwu_platform::load_history();
            self.history.set_limit(self.config.clipboard.limit);
            // 基线要重取：否则重新打开开关的下一拍会把“关闭期间的最后一次复制”补记进来。
            self.clipboard_seq = zouwu_platform::clipboard::sequence();
            if self.config.clipboard.enabled {
                if self.history_window.is_some() {
                    self.refresh_history_window();
                }
            } else if let Some(window) = self.history_window.take() {
                // 关掉功能就把窗也收掉，别留一个不会再更新的列表。
                self.retire(window);
            }
            log::info!(
                "clipboard: enabled={}, limit={}, {} entries loaded",
                self.config.clipboard.enabled,
                self.config.clipboard.limit,
                self.history.len()
            );
        }

        if changed.contains(&ChangeKind::Ring) {
            self.ring = self.config.ring.to_ring_config();
            // 球的数量/顺序/图标都可能变了：重新解一遍清单，
            // 图也重读（用户可能刚刚选了一张新图）。
            self.resolve_items();
            if self.window.is_some() {
                self.relayout();
            }
        }

        if changed.contains(&ChangeKind::WindowSize) {
            // 改尺寸交给窗口事件：`Resized` 会顺带 resize presenter。
            if let Some(window) = &self.window {
                let size = self.config.window.size;
                let _ = window.request_inner_size(LogicalSize::new(size, size));
            }
        }

        if changed.contains(&ChangeKind::WindowLevel) {
            // 必须走 `layer::set_always_on_top`（`SetWindowPos` 改 Z 序），
            // **不能**用 winit 的 `set_window_level`：后者会重建窗口、丢掉
            // `WS_EX_LAYERED`，下一帧起 `UpdateLayeredWindow` 永久失败（实测
            // 1172 条 render failed / CPU 66%）。
            if let Some(hwnd) = self.hwnd() {
                zouwu_platform::layer::set_always_on_top(hwnd, self.config.window.always_on_top);
            }
        }

        if changed.contains(&ChangeKind::Appearance) {
            // 手改 config.toml 里的 theme/skin/font_pack 也要当场生效，不必重启。
            self.set_theme(self.config.appearance.theme.clone(), now);
            self.set_skin(self.config.appearance.skin.clone(), now);
            self.set_font_pack(self.config.appearance.font_pack.clone(), now);
            // 子窗口是复用的（不再销毁重建），所以换肤也得主动告诉他们。
            let theme = self.theme.clone();
            if let Some(window) = self.history_window.as_mut() {
                window.set_theme(theme.clone());
            }
            if let Some(window) = self.report.as_mut() {
                window.set_theme(theme);
            }
        }

        if changed.contains(&ChangeKind::EyeCare) {
            // 面板里拖值时要实时看到变暖变淡，所以这里不等到重启。
            self.apply_eye_care("config", true);
        }

        if changed.contains(&ChangeKind::AltDrag) {
            self.apply_alt_drag("config");
        }

        if changed.contains(&ChangeKind::Log) {
            // 日志是**唯一在配置之前就启动**的子系统（坏配置的排查线索全靠它），
            // 所以这里不是"重启它"，而是把新级别/新落盘策略推给已在跑的那个。
            zouwu_platform::logging::apply(&self.config.log);
        }

        let names: Vec<&str> = changed.iter().map(|k| k.as_str()).collect();
        self.last_frame = now;
        self.dirty = true;
        log::info!("config applied from {origin}: {}", names.join(", "));
    }

    /// 把“Alt 拖动窗口”推到平台层（装 / 卸低层鼠标钩子）。
    ///
    /// 钩子跑在平台层自己的线程里（见 `zouwu_platform::altdrag` 的模块文档），
    /// 主线程只是开关它，所以开机、改配置、退出这三条路径都不会被它拖住。
    fn apply_alt_drag(&self, origin: &str) {
        let on = self.config.alt_drag.enabled;
        match zouwu_platform::altdrag::apply(on) {
            Ok(()) => log::info!("alt drag: {} ({origin})", if on { "on" } else { "off" }),
            Err(err) => log::warn!("alt drag unavailable: {err}"),
        }
    }

    /// 把护眼遮罩推到平台层。
    ///
    /// 失败只记一条 warn：遮罩是附加效果，不该因为“这台机器上建不了
    /// 窗口”而让挂件启动失败，也不该变成每次配置重载都刷屏的错误。
    fn apply_eye_care(&self, origin: &str, verbose: bool) {
        let care = self.config.eye_care;
        match zouwu_platform::eyecare::apply(care.enabled, care.alpha()) {
            Ok(()) => {
                if verbose {
                    // 关着的时候不报 alpha：那个数字只是“开着会是多不透明”，
                    // 混在日志里很容易被误读成“已经贴上了”。
                    if care.enabled {
                        log::info!(
                            "eye care: strength {} -> alpha {}/255 ({origin})",
                            care.strength,
                            care.alpha()
                        );
                    } else {
                        log::info!("eye care: off ({origin})");
                    }
                }
            }
            Err(err) => {
                if verbose {
                    log::warn!("eye care unavailable: {err}");
                } else {
                    log::debug!("eye care unavailable: {err}");
                }
            }
        }
    }

    /// 打开（或唤到前面）设置面板。
    fn open_settings(&mut self, event_loop: &ActiveEventLoop, _now: Instant) {
        crate::watchdog::enter(crate::watchdog::stage::OPEN_SETTINGS);
        if let Some(panel) = self.settings.as_mut() {
            let skins = zouwu_platform::assets::skin_choices(&self.i18n);
            panel.reload(self.config.clone(), self.themes.clone(), skins);
            return;
        }
        // 皮肤列表每次都重扫：用户刚往 `skins/` 里丢了新形象，打开面板就该看得见。
        let skins = zouwu_platform::assets::skin_choices(&self.i18n);
        match SettingsWindow::new(event_loop, self.config.clone(), self.themes.clone(), skins) {
            Ok(panel) => {
                self.settings = Some(panel);
                // 所有面板一视同仁：开面板**不动**小兽的 Z 序（ADR-039）。
                // 小兽留在它该在的那一层，面板开着也照样能拖能点。
            }
            Err(err) => log::error!("cannot open settings panel: {err}"),
        }
    }

    /// 面板窗口的事件分发。返回的 [`SettingsOutcome`] 在这里落地。
    fn settings_event(&mut self, _event_loop: &ActiveEventLoop, event: WindowEvent) {
        crate::watchdog::enter(crate::watchdog::stage::SETTINGS_EVENT);
        let mut pending: Vec<(Config, Vec<ChangeKind>)> = Vec::new();
        let mut close = false;
        {
            let Some(panel) = self.settings.as_mut() else {
                return;
            };
            for outcome in panel.window_event(&event) {
                match outcome {
                    SettingsOutcome::Close => close = true,
                    SettingsOutcome::Changed(kinds) => {
                        pending.push((panel.model.config.clone(), kinds));
                    }
                }
            }
            if close {
                crate::watchdog::enter(crate::watchdog::stage::CLOSE_WINDOW);
                panel.close();
            }
        }
        for (next, kinds) in pending {
            let now = Instant::now();
            self.apply_config(next, &kinds, now, "settings");
            // 面板里的改动是用户明确的意图，立即落盘（不像磁盘重载那样只接管内存）。
            self.save_config();
        }
        if close {
            // 关闭按钮的按下/松开不属于小兽，先同步按键基线再恢复轮询。
            self.buttons_down = (
                zouwu_platform::layer::left_button_down(),
                zouwu_platform::layer::right_button_down(),
            );
            self.next_poll = Instant::now();
            self.last_child_closed_at = Instant::now();
            log::info!("settings panel closed");
        }
    }

    /// 一个动作真的被执行。
    ///
    /// 目前只有二维码是完整链路，其余气泡仍是占位（日志里能看出来点到的是谁）。
    /// `ZOUWU_CYCLE` 的驱动器。见 `cycle_left` 字段的注释。
    ///
    /// 关闭走的是 `self.window_event(.., CloseRequested)`，而不是直接 `take()`：
    /// 要复现的正是"在事件分发里关窗"这个形状，绕过去就测不到。
    fn cycle_tick(&mut self, event_loop: &ActiveEventLoop, now: Instant) {
        if self.cycle_left == 0 {
            return;
        }
        if let Some(next) = self.cycle_next {
            if now < next {
                return;
            }
        }
        let step = self.cycle_step;
        self.cycle_step = (step + 1) % 7;
        match step {
            0 => {
                log::info!("cycle: open history ({} left)", self.cycle_left);
                // 顺手改一下剪贴板：真实使用中历史窗开着时剪贴板会变，
                // 那条路径会 rebuild_layout + request_inner_size。
                let text = format!("cycle {}", self.cycle_left);
                if !zouwu_platform::clipboard::set_text(&text) {
                    log::warn!("cycle: clipboard set failed");
                }
                self.open_history(event_loop);
            }
            // 不关历史窗，直接在它上面开二维码报告窗：用户就是这么用的。
            1 => {
                log::info!("cycle: open report on top of history");
                let report = Report {
                    headline: "cycle".into(),
                    detail: None,
                    entries: vec![crate::report::Entry {
                        text: "cycle".into(),
                        screen: "s".into(),
                    }],
                    footer: "cycle".into(),
                };
                self.open_report(event_loop, report);
            }
            2 => {
                if let Some(id) = self.report.as_ref().map(|w| w.id()) {
                    log::info!("cycle: close report");
                    self.window_event(event_loop, id, WindowEvent::CloseRequested);
                }
            }
            // 关键一步：历史窗还开着，再调一次 -> 复用已有窗口。
            3 => {
                log::info!("cycle: reopen history while it is still open");
                self.open_history(event_loop);
            }
            4 => {
                if let Some(id) = self.history_window.as_ref().map(|w| w.id()) {
                    log::info!("cycle: close history");
                    self.window_event(event_loop, id, WindowEvent::CloseRequested);
                }
            }
            // 设置面板也不能漏：它是普通可缩放窗口，关掉时同样会转交前台。
            5 => {
                log::info!("cycle: open settings");
                self.open_settings(event_loop, now);
            }
            _ => {
                if let Some(id) = self.settings.as_ref().map(|w| w.id()) {
                    log::info!("cycle: close settings");
                    self.window_event(event_loop, id, WindowEvent::CloseRequested);
                }
                self.cycle_left -= 1;
                if self.cycle_left == 0 {
                    log::info!("cycle: done");
                }
            }
        }
        self.cycle_next = Some(now + Duration::from_millis(150));
    }

    fn run_action(&mut self, event_loop: &ActiveEventLoop, id: &str) {
        crate::watchdog::enter(crate::watchdog::stage::RUN_ACTION);
        log::info!("app: run action `{id}`");
        match id {
            QRCODE_ACTION => self.scan_qr(event_loop),
            CLIPBOARD_ACTION => self.open_history(event_loop),
            _ => self.run_custom(event_loop, id),
        }
    }

    /// 自定义球：把命令拉起来。
    ///
    /// 拉不起来时弹结果窗而不是只写日志：球是自己配的，点了没反应
    /// 会让人以为是程序坏了；错在哪里（路径没了、命令拼错）必须看得见。
    fn run_custom(&mut self, event_loop: &ActiveEventLoop, id: &str) {
        let Some(item) = self.items.iter().find(|item| item.id == id) else {
            // 球已从配置里删了，但托盘/热键还指着它：安静走掉即可。
            log::info!("action `{id}` is no longer on the ring");
            return;
        };
        let Some(command) = item
            .command
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        else {
            log::warn!("action `{id}` has no command; nothing to run");
            return;
        };
        let name = item.label.clone();
        match zouwu_platform::launch::run(command) {
            Ok(()) => log::info!("action `{id}` launched: {command}"),
            Err(err) => {
                log::warn!("action `{id}` failed: {err}");
                let report = Report {
                    headline: self.i18n.t("report.launch.failed"),
                    detail: Some(name.clone()),
                    entries: vec![Entry {
                        text: err,
                        screen: command.to_string(),
                    }],
                    footer: self.i18n.t("report.launch.footer"),
                };
                self.open_report(event_loop, report);
            }
        }
    }

    /// 执行某颗球的某个子项命令。
    ///
    /// 与 `run_custom` 同一条失败路径：起不来就弹报告，而不是悄悄吞掉——
    /// 子项命令是用户手写的，写错很正常，必须让他看见错误。
    fn run_child(&mut self, event_loop: &ActiveEventLoop, parent: usize, child: usize) {
        let Some(item) = self.items.get(parent) else {
            return;
        };
        let Some(entry) = item.children.get(child) else {
            // 配置在运行期被改窄了（热重载）：安静走掉。
            return;
        };
        let command = entry.command.trim().to_string();
        if command.is_empty() {
            log::warn!("child `{}` of `{}` has no command", entry.label, item.id);
            return;
        }
        let name = entry.display_name().to_string();
        match zouwu_platform::launch::run(&command) {
            Ok(()) => log::info!("child `{name}` launched: {command}"),
            Err(err) => {
                log::warn!("child `{name}` failed: {err}");
                let report = Report {
                    headline: self.i18n.t("report.launch.failed"),
                    detail: Some(name),
                    entries: vec![Entry {
                        text: err,
                        screen: command,
                    }],
                    footer: self.i18n.t("report.launch.footer"),
                };
                self.open_report(event_loop, report);
            }
        }
    }

    /// 截全屏 → 找码 → 弹结果框。
    ///
    /// 截的是**整个虚拟桌面**而不是当前显示器：二维码很可能就开在另一块屏上的文档里，
    /// 让用户先把窗口拖过来再扫，等于把这个功能的手感毁掉一半。
    fn scan_qr(&mut self, event_loop: &ActiveEventLoop) {
        crate::watchdog::enter(crate::watchdog::stage::RUN_ACTION);
        let started = Instant::now();
        let capture = match zouwu_platform::screen::capture_virtual_screen() {
            Ok(capture) => capture,
            Err(err) => {
                log::warn!("qr: capture failed: {err}");
                self.open_report(event_loop, failure_report(&self.i18n, "qr.error", &err));
                return;
            }
        };
        let shot = qr::Shot {
            width: capture.width,
            height: capture.height,
            rgba: &capture.rgba,
        };
        let scan = qr::scan(&shot);
        let elapsed = started.elapsed().as_millis() as u64;
        log::info!(
            "qr: {} code(s), {} unreadable, {} ms, desktop {}x{}",
            scan.hits.len(),
            scan.unreadable,
            elapsed,
            capture.width,
            capture.height
        );
        let monitors = App::monitor_rects(event_loop);
        let report = build_report(&self.i18n, &scan, &capture, elapsed, &monitors);
        self.open_report(event_loop, report);
    }

    /// 摆出结果小窗。同一时刻只留一个 —— 连按两次快捷键不该叠出一摞窗。
    fn open_report(&mut self, event_loop: &ActiveEventLoop, report: Report) {
        crate::watchdog::enter(crate::watchdog::stage::OPEN_REPORT);
        log::info!("app: open report (was open: {})", self.report.is_some());
        if let Some(window) = self.report.as_mut() {
            window.show(report);
            log::info!("qr: report window reused");
            return;
        }
        let title = self.i18n.t("qr.window-title");
        let copied = self.i18n.t("qr.copied");
        match ReportWindow::new(event_loop, report, self.theme.clone(), &title, copied) {
            Ok(window) => {
                self.report = Some(window);
                log::info!("qr: report window opened");
            }
            Err(err) => log::error!("qr: cannot open report window: {err}"),
        }
    }

    fn report_event(&mut self, event: WindowEvent) {
        crate::watchdog::enter(crate::watchdog::stage::REPORT_EVENT);
        log::trace!("app: report event {event:?}");
        let mut close = false;
        {
            let Some(window) = self.report.as_mut() else {
                return;
            };
            for outcome in window.window_event(&event) {
                match outcome {
                    ReportOutcome::Close => close = true,
                }
            }
        }
        if close {
            crate::watchdog::enter(crate::watchdog::stage::CLOSE_WINDOW);
            if let Some(window) = self.report.as_mut() {
                window.close();
            }
            self.last_child_closed_at = Instant::now();
            self.buttons_down = (
                zouwu_platform::layer::left_button_down(),
                zouwu_platform::layer::right_button_down(),
            );
            self.next_poll = Instant::now();
            log::info!("qr: report window closed");
        }
    }

    /// 建（或重建）托盘图标。
    ///
    /// 单独成一个函数是因为**导入新皮肤后菜单要长出这一条**：托盘菜单是静态构建的，
    /// 只能丢掉重建。热键不能跟着重建（重复注册会 already registered），所以两者分开。
    fn init_tray(&mut self) {
        crate::watchdog::enter(crate::watchdog::stage::TRAY);
        let (rgba, w, h) = crate::paint::tray_icon(&self.theme, self.skin_for_paint(), 32);
        let autostart_on = autostart::is_enabled();
        // 主题/皮肤列表在启动时扫一次：换外观不是高频操作，为它挂目录监听不值得。
        // 例外是导入功能：它导入完会 `rebuild_tray`，否则新皮肤不会出现在菜单里。
        let opts = TrayOptions {
            icon_rgba: Some((rgba, w, h)),
            language_setting: &self.config.general.language,
            autostart: autostart_on,
            visible: self.visible,
            theme_id: &self.config.appearance.theme,
            skin_id: &self.config.appearance.skin,
        };
        match Tray::new(&self.i18n, &self.themes, opts) {
            Ok(tray) => self.tray = Some(tray),
            Err(err) => log::error!("tray init failed: {err}"),
        }
    }

    /// 丢掉旧托盘再建一个（皮肤列表变了之后调用）。
    fn rebuild_tray(&mut self) {
        self.tray = None;
        self.init_tray();
    }

    /// 托盘图标 + 全局热键。只做一次，且必须**在主线程**上做（见 platform::tray）。
    fn init_shell(&mut self) {
        if self.tray.is_none() {
            self.init_tray();
        }

        // 配置里的字体包在这里装上：字体文件已在 `run()` 里注册进本进程，
        // 但"用哪个家族"要等文本引擎第一次被问到之前定下来。
        let pack_id = self.config.appearance.font_pack.clone();
        if pack_id.is_empty() {
            log::debug!("font -> system default");
        } else {
            match zouwu_platform::fontpack::get(&pack_id) {
                Some(pack) => {
                    crate::text::set_family(Some(pack.family.clone()));
                    log::info!("font -> {} (pack `{pack_id}`)", pack.family);
                }
                None => {
                    log::warn!("font pack `{pack_id}` is missing; using the system font");
                    self.config.appearance.font_pack.clear();
                }
            }
        }

        if self.hotkeys.is_none() {
            match HotkeyRegistrar::new() {
                Ok(mut registrar) => {
                    if let Err(err) = self.apply_hotkey(&mut registrar) {
                        log::warn!("{err}");
                    }
                    self.hotkeys = Some(registrar);
                }
                Err(err) => log::error!("{err}"),
            }
        }

        // 配置说"要自启"但注册表里没有（比如换了路径）→ 补上
        if self.config.general.start_on_boot && !autostart::is_enabled() {
            let actual = autostart::set_enabled(true);
            if actual != self.config.general.start_on_boot {
                self.config.general.start_on_boot = actual;
                self.save_config();
            }
        }

        // 护眼遮罩：开机恢复上次的开关与强度。
        self.apply_eye_care("startup", true);

        // Alt 拖动窗口：开机按配置装（或是不装）那个低层鼠标钩子。
        self.apply_alt_drag("startup");

        // 一行交代清楚"以什么姿态在跑"：排查问题时先看这一行。
        log::info!(
            "ready: lang {} (setting `{}`), hotkey `{}`, autostart {}, window {} (tray {})",
            self.i18n.language(),
            self.config.general.language,
            self.config.general.hotkey,
            if autostart::is_enabled() { "on" } else { "off" },
            if self.visible { "visible" } else { "hidden" },
            if self.tray.is_some() { "ok" } else { "missing" },
        );
    }

    /// 按当前配置重新注册全部热键（呼出 + 每一颗球）。
    ///
    /// 热键注册表存在 `self.hotkeys` 里，而 `apply_hotkey` 要读 `self.config`：
    /// 先把它取出来，免得同一个 `self` 同时被可变借用与不可变借用。
    fn reapply_hotkeys(&mut self) {
        if let Some(mut registrar) = self.hotkeys.take() {
            let result = self.apply_hotkey(&mut registrar);
            self.hotkeys = Some(registrar);
            if let Err(err) = result {
                log::warn!("{err}");
            }
        }
    }

    /// 按配置注册热键；解析失败时回退到默认值并记日志。
    ///
    /// 两个档次：呼出差错必须自愈（否则用户打不开小兽），
    /// 球的差错只警告那一颗（一个写错的热键不该把别的球一起废掉）。
    fn apply_hotkey(&self, registrar: &mut HotkeyRegistrar) -> Result<(), String> {
        apply_hotkey_setting(Slot::Summon, &self.config.general.hotkey, registrar)?;
        // 先放掉已经不存在的那几颗球（删球/减球）占着的键位。
        registrar.retain_ring_items(self.config.ring.items.len());
        for (index, item) in self.config.ring.items.iter().enumerate() {
            // 关掉的球连热键一起让出来：否则用户关了一个功能，
            // 那个键位还被一个看不见的开关占着，查都没处查。
            if !item.enabled {
                registrar.clear(Slot::RingItem(index));
                continue;
            }
            if let Err(err) = apply_hotkey_setting(Slot::RingItem(index), &item.hotkey, registrar) {
                log::warn!("ring item `{}` hotkey skipped: {err}", item.id);
            }
        }
        Ok(())
    }

    /// 显示 / 隐藏小兽（隐藏 = 缩进托盘，不退出进程）。
    fn set_visible(&mut self, visible: bool, now: Instant) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            // 隐藏期间时间照样在走，dt 会很大：把帧节拍对齐到现在。
            self.last_frame = now;
            self.dirty = true;
            self.buttons_down = (
                zouwu_platform::layer::left_button_down(),
                zouwu_platform::layer::right_button_down(),
            );
            self.next_poll = now;
            if let Some(hwnd) = self.hwnd() {
                zouwu_platform::layer::set_visible(hwnd, true);
                if self.config.general.bring_to_front {
                    zouwu_platform::layer::bring_to_front(hwnd);
                }
            }
            // 呼出即展开：钉住（`pinned`）之后自动收起判定不再生效，
            // 要收起来只能点本体 / 右键 / 拖动 / 再按一次热键。
            if self.config.interaction.expand_on_summon && !self.items.is_empty() {
                self.set_expanded(true, now);
                self.pinned = true;
                log::debug!("summon: ring expanded and pinned");
            }
            log::info!("zouwu shown");
            if let Some(w) = self.window_arc() {
                w.request_redraw();
            }
        } else {
            // 先收起再躲起来，下次呼出时是"干净"的收起态。
            self.pinned = false;
            self.set_expanded(false, now);
            self.on_cursor_left();
            self.buttons_down = (false, false);
            self.save_state();
            if let Some(hwnd) = self.hwnd() {
                zouwu_platform::layer::set_visible(hwnd, false);
            }
            log::info!("zouwu hidden to tray");
        }
        if let Some(tray) = &self.tray {
            tray.set_visible_state(&self.i18n, self.visible);
        }
    }

    fn toggle_visible(&mut self, now: Instant) {
        self.set_visible(!self.visible, now);
    }

    fn set_language(&mut self, setting: &str) {
        if self.config.general.language == setting {
            return;
        }
        self.config.general.language = setting.to_string();
        self.i18n.set_language(setting);
        if let Some(tray) = &self.tray {
            tray.apply_language(&self.i18n, &self.config.general.language, self.visible);
        }
        self.save_config();
        self.dirty = true;
        log::info!(
            "language setting `{setting}` resolved to {}",
            self.i18n.language()
        );
    }

    fn set_autostart(&mut self, on: bool) {
        let actual = autostart::set_enabled(on);
        self.config.general.start_on_boot = actual;
        if let Some(tray) = &self.tray {
            tray.set_autostart_checked(actual);
        }
        self.save_config();
        log::info!("autostart -> {actual}");
    }

    /// 换主题：只换调色板，不动几何/皮肤。
    ///
    /// 幂等：id 没变就直接返回，所以配置热重载也能无脑调它。
    fn set_theme(&mut self, id: String, now: Instant) {
        if self.theme.id == id {
            return;
        }
        let Some(theme) = self.themes.iter().find(|t| t.id == id).cloned() else {
            log::warn!("theme `{id}` is not available");
            return;
        };
        self.theme = theme;
        self.config.appearance.theme = id.clone();
        if let Some(tray) = &self.tray {
            tray.apply_theme(&id);
        }
        self.refresh_tray_icon();
        self.save_config();
        self.last_frame = now;
        self.dirty = true;
        log::info!("theme -> {id}");
    }

    /// 换皮肤：失败就**保持现状**（而不是悄悄退回 procedural——
    /// 用户选了某张皮却看见代码绘制的小兽，会以为程序坏了）。
    fn set_skin(&mut self, id: String, now: Instant) {
        if self.skin.manifest.id == id {
            return;
        }
        let skin = load_skin(&id);
        if id != PROCEDURAL_SKIN_ID && skin.is_procedural() {
            log::warn!(
                "skin `{id}` failed to load; keeping `{}`",
                self.skin.manifest.id
            );
            return;
        }
        self.skin = skin;
        self.config.appearance.skin = id.clone();
        if let Some(tray) = &self.tray {
            tray.apply_skin(&id);
        }
        self.refresh_tray_icon();
        self.save_config();
        self.last_frame = now;
        self.dirty = true;
        log::info!("skin -> {id}");
    }

    /// 换字体包：`""` = 系统默认字体。
    ///
    /// 字体是**全局状态**（平台层的字体缓存），所以这里不需要碰 pixmap 之外的东西，
    /// 但必须把整幅重画：连气泡环里的数字、面板里的每个字都要换。
    /// 包不存在时退回系统字体并记日志——字体不像皮肤，退回去至少还是能读的。
    fn set_font_pack(&mut self, id: String, now: Instant) {
        if self.config.appearance.font_pack == id {
            return;
        }
        let family = if id.is_empty() {
            None
        } else {
            match zouwu_platform::fontpack::get(&id) {
                Some(pack) => {
                    // 运行期新导入的包还没注册进本进程，用之前先补一次。
                    let dir = zouwu_platform::fontpack::fonts_dir().join(&pack.id);
                    for file in &pack.files {
                        zouwu_platform::text_gdi::add_font_file(&dir.join(file));
                    }
                    Some(pack.family)
                }
                None => {
                    log::warn!("font pack `{id}` not found; falling back to the system font");
                    None
                }
            }
        };
        if !crate::text::set_family(family.clone()) {
            log::debug!("font family unchanged");
        }
        self.config.appearance.font_pack = if family.is_some() {
            id.clone()
        } else {
            String::new()
        };
        self.save_config();
        self.last_frame = now;
        self.dirty = true;
        log::info!("font -> {}", family.as_deref().unwrap_or("system default"));
    }

    /// 导入 `.zwskin`：选文件 → 校验 → 落盘 → 重建菜单 → 立刻换上。
    ///
    /// 成功后**不给弹窗**：小兽当场变了个样，比任何提示都直接。
    /// 失败必须弹窗——常驻托盘的工具没有控制台，用户不会去翻日志。
    fn import_skin(&mut self, now: Instant) {
        let Some(path) = zouwu_platform::dialogs::pick_skin_package() else {
            return;
        };
        match zouwu_platform::skinpack::import(&path) {
            Ok(id) => {
                log::info!("imported skin `{id}` from {}", path.display());
                // 菜单是静态构建的，新皮肤要重建托盘才看得见
                self.rebuild_tray();
                self.set_skin(id, now);
            }
            Err(err) => {
                zouwu_platform::dialogs::warn(&self.i18n.t("skin.import.failed"), &err);
            }
        }
    }

    /// 导出当前皮肤成 `.zwskin`（发给别人 / 备份自己手改过的那套）。
    fn export_skin(&mut self) {
        let id = self.skin.manifest.id.clone();
        if id == PROCEDURAL_SKIN_ID {
            // 代码绘制的小兽没有文件可打：这不是错误，但必须说清楚
            zouwu_platform::dialogs::warn(
                &self.i18n.t("skin.export.failed"),
                &self.i18n.t("skin.export.procedural"),
            );
            return;
        }
        let Some(path) = zouwu_platform::dialogs::pick_export_path(&format!("{id}.zwskin")) else {
            return;
        };
        match zouwu_platform::skinpack::export_to(&id, &path) {
            Ok(()) => log::info!("exported skin `{id}` to {}", path.display()),
            Err(err) => {
                zouwu_platform::dialogs::warn(&self.i18n.t("skin.export.failed"), &err);
            }
        }
    }

    /// 托盘那一小格用的是同一套画笔：换了颜色/形象要重画，否则托盘和窗口不一样。
    fn refresh_tray_icon(&self) {
        let Some(tray) = &self.tray else { return };
        let (rgba, w, h) = crate::paint::tray_icon(&self.theme, self.skin_for_paint(), 32);
        tray.set_icon(Some((rgba, w, h)));
    }

    /// 处理托盘命令；返回 `true` 表示要退出程序。
    fn handle_tray(&mut self, cmd: TrayCommand, now: Instant) -> bool {
        match cmd {
            TrayCommand::Toggle => self.toggle_visible(now),
            TrayCommand::Show => self.set_visible(true, now),
            TrayCommand::Hide => self.set_visible(false, now),
            TrayCommand::Settings => self.pending_settings = true,
            TrayCommand::OpenConfigDir => {
                // 手改配置的老路留着：面板不可能覆盖所有字段（注释、外部主题等）。
                if !zouwu_platform::open_config_dir() {
                    log::warn!("config dir: {}", zouwu_platform::config_dir().display());
                }
            }
            TrayCommand::OpenFontsDir => {
                zouwu_platform::fontpack::reveal_dir();
            }
            TrayCommand::OpenLogs => {
                if !zouwu_platform::logging::open_log_dir() {
                    log::warn!("log dir: {}", zouwu_platform::logging::log_dir().display());
                }
            }
            TrayCommand::Recenter => self.pending_recenter = true,
            // 扫二维码要建窗口，得拿到 `&ActiveEventLoop` —— 同气泡点击，走待办队列。
            TrayCommand::ScanQr => self.pending_action = Some(QRCODE_ACTION.to_string()),
            TrayCommand::Clipboard => self.pending_action = Some(CLIPBOARD_ACTION.to_string()),
            TrayCommand::SetTheme(id) => self.set_theme(id, now),
            TrayCommand::SetSkin(id) => self.set_skin(id, now),
            TrayCommand::ImportSkin => self.import_skin(now),
            TrayCommand::ExportSkin => self.export_skin(),
            TrayCommand::SetAutostart(on) => self.set_autostart(on),
            TrayCommand::SetLanguage(code) => self.set_language(code),
            TrayCommand::Quit => return true,
        }
        false
    }

    /// 一次"外壳"轮询：托盘菜单、全局热键、第二实例信号、配置文件。
    ///
    /// 它们都不用"回调 → 通知 UI"那套线程模型，而是在这里非阻塞地取一次。
    /// 原因：它们的唤醒都在别处（托盘自己的线程、消息队列、system event），
    /// 我们已经有 100ms 的轮询节拍在跑，借它一次把这几件事问完，省掉跨线程唤醒。
    fn poll_shell(&mut self, now: Instant) -> bool {
        let mut exit = false;

        // 配置文件热重载：内部按 500ms 节流，所以每拍都问一次也不贵。
        match self.config_watcher.poll() {
            ReloadEvent::Changed(next) => self.reload_config(*next, now),
            ReloadEvent::Invalid(err) => {
                // 坏的配置绝不能打断运行：用户可能正敲到一半。旧配置继续生效。
                log::warn!("config reload skipped: {err}");
            }
            ReloadEvent::Unchanged => {}
        }

        if self
            .instance
            .as_ref()
            .map(|i| i.take_signal())
            .unwrap_or(false)
        {
            log::info!("second instance launched -> showing window");
            self.set_visible(true, now);
        }

        if let Some(cmd) = self.tray.as_ref().and_then(|t| t.poll()) {
            exit |= self.handle_tray(cmd, now);
        }

        if let Some(slot) = self.hotkeys.as_ref().and_then(|h| h.poll_pressed()) {
            log::info!("hotkey: {slot:?} pressed");
            match slot {
                Slot::Summon => self.toggle_visible(now),
                Slot::RingItem(index) => {
                    // 按配置顺序取第 index 条：关掉的球不占位置，所以这里取的是
                    // **配置里的第几条**，与屏幕上第几颗由 `resolve` 保序对应。
                    if let Some(item) = self.config.ring.items.get(index) {
                        if item.enabled {
                            self.pending_action = Some(item.id.clone());
                        }
                    }
                }
            }
        }

        exit
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

    /// 重新按配置解出环上的球（数量/顺序/名字/图标）。
    ///
    /// 三个入口：启动、改 `[ring]` 配置（含改开关/排序/图标）、换语言。
    /// 动画弹簧要跟着数量对齐：多出来的球从 0 开始弹，少掉的直接丢掉，
    /// 否则 `anims[i]` 会和 `items[i]` 错位，出现"新球一开始就是开的"。
    fn resolve_items(&mut self) {
        self.items = zouwu_core::action::resolve(&self.config.ring.items, &self.i18n);
        self.icons = load_icons(&self.items);
        let n = self.items.len();
        if self.anims.len() < n {
            self.anims.resize_with(n, || Spring::soft(0.0));
        } else {
            self.anims.truncate(n);
        }
        self.bubbles.truncate(n);
        if self.hovered_bubble.is_some_and(|i| i >= n) {
            self.hovered_bubble = None;
        }
        // 打激光条的那颗球被删掉/关了子项 → 直接收起，别对着空气画光
        if self
            .open_children
            .is_some_and(|i| self.items.get(i).map_or(true, |item| !item.has_children()))
        {
            self.close_children();
        }
        log::info!(
            "ring items: {n} ({} configured)",
            self.config.ring.items.len()
        );
    }

    /// 重新计算气泡位置（含避让屏幕边缘的整体旋转）
    fn relayout(&mut self) {
        let center = self.center();
        let view = self.viewport();
        let rotation = solve_rotation(&self.ring, center, view, self.items.len());
        self.bubbles = self.ring.layout(self.items.len(), center, rotation);
        self.relayout_beams(center, view);
    }

    /// 算激光条的位置。文字宽度要按**真字体**量，否则牌子宽度和里面的字对不上：
    /// 量宽用与绘制同一个字号（`CHIP_FONT`），并根据 DPI 换算回逻辑像素。
    fn relayout_beams(&mut self, center: Point, view: Rect) {
        self.beams.clear();
        let Some(parent_idx) = self.open_children else {
            return;
        };
        let (Some(bubble), Some(item)) = (self.bubbles.get(parent_idx), self.items.get(parent_idx))
        else {
            return;
        };
        if !item.has_children() {
            return;
        }
        let labels: Vec<String> = item
            .children
            .iter()
            .map(|child| child.display_name().to_string())
            .collect();
        let scale = self
            .window
            .as_ref()
            .map(|w| w.scale_factor() as f32)
            .unwrap_or(1.0)
            .max(0.5);
        let mut measure = |text: &str| {
            crate::text::TextEngine::with(|engine| engine.measure(text, CHIP_FONT * scale, false))
                / scale
        };
        self.beams = beam::layout(
            &BeamInput {
                parent: bubble.center,
                parent_radius: bubble.radius,
                center,
                viewport: view,
                labels: &labels,
            },
            &mut measure,
        );
    }

    /// 收起激光条（不碰气泡环本身）。
    fn close_children(&mut self) {
        if self.open_children.take().is_some() {
            log::debug!("beams closed");
            self.hovered_child = None;
            // 窗口要立刻缩回原尺寸，几何留着会被裁掉一半、也不该继续接点击。
            self.beams.clear();
            self.dirty = true;
            // 为激光条撑大的窗口收回原样。
            self.apply_window_extent(self.config.window.size);
        }
        self.child_anim.set_target(0.0);
    }

    /// 打开某颗球的激光条。
    fn open_children_of(&mut self, index: usize) {
        if !self
            .items
            .get(index)
            .is_some_and(|item| item.has_children())
        {
            return;
        }
        if self.open_children != Some(index) {
            self.open_children = Some(index);
            // 从头生长：弹簧归零再射出去，否则会从上一颗球的位置滑过来。
            self.child_anim = Spring::soft(0.0);
            self.child_anim.set_target(1.0);
            log::debug!("beams opened for {}", self.items[index].id);
            self.dirty = true;
        }
        self.grow_for_beams();
    }

    /// 先按“窗口够大”的理想情况排一次，据此把窗口撑到刚好，再按真实窗口重排。
    ///
    /// 反过来的话牌子会先被夹回小窗口、叠成一团，再撑大也回不去了。
    fn grow_for_beams(&mut self) {
        let center = self.center();
        let view = self.viewport();
        self.relayout_beams(center, view.inset(-BEAM_SEED));
        self.fit_window_to_beams();
        self.relayout();
    }

    /// 激光条要的地方比窗口大时，把窗口临时长大（小兽在屏幕上的位置不动）。
    ///
    /// 牌子是**横排文字**，绕着一颗球往外射，需要的空间比气泡环大得多：
    /// 默认 340 的正方形窗口里，环外只剩十几像素，牌子必然会挤成一堆。
    /// 所以开着激光条就把窗口撑大，关掉再收回去——只在需要时才变大，
    /// 空闲时不占地方。
    fn fit_window_to_beams(&mut self) {
        if self.beams.is_empty() {
            return;
        }
        // 需要多大由几何自己说：牌子四角离环心最远的那个方向乘以二，再加一点
        // 静空。写死一个“光束预留空间”会两头不讨好——短子项把窗口撑得很大
        // （pixmap 白占内存），长子项又不够（牌子被夹回窗口、叠成一团）。
        let center = self.center();
        let mut half = 0.0f32;
        for beam in &self.beams {
            half = half.max(
                (beam.chip.x - center.x)
                    .abs()
                    .max((beam.chip.right() - center.x).abs()),
            );
            half = half.max(
                (beam.chip.y - center.y)
                    .abs()
                    .max((beam.chip.bottom() - center.y).abs()),
            );
        }
        let want = ((half + BEAM_ROOM) * 2.0).max(self.config.window.size);
        self.apply_window_extent(want);
    }

    /// 把窗口边长改成 `want`（逻辑像素），保持小兽的屏幕位置不变。
    ///
    /// 窗口以**中心**对称长大：小兽画在窗口中心，所以原点要往回收一半，
    /// 否则被撑大的这半格会把小兽推走。
    fn apply_window_extent(&mut self, want: f32) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let want = want.max(self.config.window.size);
        let scale = window.scale_factor() as f32;
        // 不变量是"小兽在屏幕上的中心点"。用**真实**窗口矩形来算：我们有时绕过
        // winit 直接 `SetWindowPos`，winit 的位置缓存会落后一次，按它反推就会把
        // 小兽在屏幕上挪走（实测收回窗口时偏 120px 级别，一眼就能看出）。
        let rect = self.hwnd().and_then(zouwu_platform::layer::window_rect);
        let cur = match rect {
            Some((_, _, w, _)) => w as f32 / scale,
            None => self.logical_size().0,
        };
        if (want - cur).abs() < 2.0 {
            return;
        }
        let phys = (want * scale).round() as i32;
        let (x, y) = match rect {
            Some((left, top, w, h)) => (left + w / 2 - phys / 2, top + h / 2 - phys / 2),
            None => {
                let before = window.outer_position().unwrap_or_default();
                let delta = ((want - cur) * scale * 0.5).round() as i32;
                (before.x - delta, before.y - delta)
            }
        };
        // 先让 winit 记下新尺寸（它内部有缓存），再用**一次** `SetWindowPos` 把
        // 位置和尺寸一起落实：分两步做实测会丢位置。
        let _ = window.request_inner_size(LogicalSize::new(want, want));
        if let Some(hwnd) = self.hwnd() {
            zouwu_platform::layer::set_bounds(hwnd, x, y, phys, phys);
        }
        // **不要**再补一次 `set_outer_position`：它也是一次 `SetWindowPos`，和上面
        // 那次谁后落地不确定，实测偶尔把窗口按 winit 的旧位置挪回去（就是"点完子项
        // 本体现在屏幕上跳一下"）。直接把几何改到位的 `SetWindowPos` 发 WM_MOVE 事件，
        // winit 自己会把位置缓存更新回来。
        log::debug!("window extent {cur} -> {want} at {x},{y} (buddy stays put)");
        self.dirty = true;
    }

    /// 光标落在哪条激光条的文字牌上。
    fn beam_hit(&self, p: Point) -> Option<usize> {
        if self.child_anim.value < 0.5 {
            return None;
        }
        beam::hit(&self.beams, p, 2.0)
    }

    fn set_expanded(&mut self, expanded: bool, now: Instant) {
        if self.expanded == expanded {
            return;
        }
        self.expanded = expanded;
        self.expand_started = Some(now);
        if !expanded {
            self.pinned = false;
            // 环收了，挂在球上的激光条没有立足之地。
            self.close_children();
        }
        log::debug!(
            "ring {} (hover delay {} ms)",
            if expanded { "expanded" } else { "collapsed" },
            self.config.interaction.hover_delay_ms
        );
        self.dirty = true;
    }

    /// 收起判定的半径上限。
    ///
    /// 必须比「气泡外沿」再留出余量：光标指到最外侧气泡时距离已经是 `radius`，
    /// 气泡自己还有 `bubble_radius` 的直径。判定线贴着气泡画，就会"明明指着
    /// 气泡却被判为离开"——这正是之前"一移动就收起"的根因。
    fn collapse_limit(&self) -> f32 {
        // 光标指到最外侧气泡时距离已经是 `radius`，气泡自身还有直径；
        // 判定线贴着气泡边缘画，就会"明明指着气泡却被判为离开"。
        let outer = self
            .config
            .interaction
            .collapse_radius(self.ring.radius, self.ring.bubble_radius);
        // 但窗口本身是有限的：判定半径不该超过窗口对角线的一半太多，
        // 否则光标早已在窗口外，判定却还说"没走"。
        let (w, h) = self.logical_size();
        let reachable = (w * w + h * h).sqrt() * 0.5;
        outer.min(reachable)
    }

    /// 收起延迟（`config.toml` 的 `collapse_delay_ms`，默认 250ms）。
    fn collapse_delay(&self) -> Duration {
        Duration::from_millis(self.config.interaction.collapse_delay_ms)
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

        // 2b) 激光条弹簧与子项悬停
        self.child_anim.step(dt);
        if self.open_children.is_some() {
            let hit = self.cursor.and_then(|p| self.beam_hit(p));
            if hit != self.hovered_child {
                self.hovered_child = hit;
                self.dirty = true;
            }
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

        // 4) 收起判定：光标远离整圈，且**持续**远离到 `collapse_delay_ms` 才收。
        //    钉住（呼出即展开）时跳过——需求就是"呼出后一直开着，直到手动收"。
        if self.expanded && !self.pinned {
            let center = self.center();
            let limit = self.collapse_limit();
            let dist = self.cursor.map(|p| p.distance(center));
            // MSRV 1.80：`is_none_or` 是 1.82 才稳定的
            if dist.map_or(true, |d| d > limit) {
                // 第一次判定"远了"只记时间，不收：给手一点穿过空隙的余量。
                let since = *self.far_since.get_or_insert(now);
                if now.duration_since(since) >= self.collapse_delay() {
                    log::debug!(
                        "collapse: far for {}ms (cursor={:?} dist={:?} limit={:.1})",
                        now.duration_since(since).as_millis(),
                        self.cursor,
                        dist,
                        limit
                    );
                    self.far_since = None;
                    self.set_expanded(false, now);
                }
            } else {
                self.far_since = None;
            }
        } else {
            self.far_since = None;
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
        // 缩进托盘时不光"看不见"，连光栅化都不做：隐藏的窗口没有输出，
        // 画了也是白算（分层窗口的内容不会自己留着）。
        if !self.visible {
            return false;
        }
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
    /// 所有显示器的工作区矩形（物理像素），**主屏排第一**。
    ///
    /// `available_monitors` 的顺序不保证稳定（换一个 DPI、换个版本就可能变），
    /// 而 `home_slot` 把第一个当主屏，所以这里显式把 `primary_monitor` 提到前面。
    fn monitor_rects(event_loop: &ActiveEventLoop) -> Vec<Rect> {
        fn to_rect(m: &winit::monitor::MonitorHandle) -> Rect {
            let p = m.position();
            let s = m.size();
            Rect::new(p.x as f32, p.y as f32, s.width as f32, s.height as f32)
        }
        let mut out = Vec::new();
        if let Some(m) = event_loop.primary_monitor() {
            out.push(to_rect(&m));
        }
        for m in event_loop.available_monitors() {
            let r = to_rect(&m);
            let dup = out
                .iter()
                .any(|o| o.x == r.x && o.y == r.y && o.w == r.w && o.h == r.h);
            if !dup {
                out.push(r);
            }
        }
        out
    }

    /// 窗口自己占的矩形（物理像素）。
    fn self_rect(window: &Window) -> Option<Rect> {
        let pos = window.outer_position().ok()?;
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return None;
        }
        Some(Rect::new(
            pos.x as f32,
            pos.y as f32,
            size.width as f32,
            size.height as f32,
        ))
    }

    /// 确认自己还在某块真实屏幕上；不在就搬回主屏。
    ///
    /// `had_position = true` 表示“本来就有位置（来自上次退出或当前状态）”，
    /// 这时只在可见比例不足时才动它——用户精心摆的位置不能因为一次启动就重置。
    /// `false` 表示“没有可信位置”（首次运行、或用户主动点了「回到主屏」）。
    ///
    /// 返回是否真的搬了。
    fn ensure_visible_on(
        &mut self,
        event_loop: &ActiveEventLoop,
        window: &Window,
        had_position: bool,
    ) -> bool {
        let monitors = Self::monitor_rects(event_loop);
        if monitors.is_empty() {
            // 拿不到显示器信息时不猜：宁可留在原地，也不要凭一个空的列表乱搬。
            return false;
        }
        let Some(me) = Self::self_rect(window) else {
            return false;
        };
        let ratio = zouwu_core::geom::visible_ratio(me, &monitors);
        if had_position && ratio >= MIN_VISIBLE_RATIO {
            return false;
        }
        let slot = zouwu_core::geom::home_slot(&monitors, me.w, EDGE_MARGIN);
        window.set_outer_position(PhysicalPosition::new(slot.x as i32, slot.y as i32));
        self.state_pos = Some((slot.x as i32, slot.y as i32));
        self.state_pending = true;
        if had_position {
            // 这条日志是“我的小工具刚才不见了”的唯一解释，值得说清楚。
            log::warn!(
                "position ({:.0}, {:.0}) only {:.0}% visible on {} monitor(s); moved to ({:.0}, {:.0})",
                me.x,
                me.y,
                ratio * 100.0,
                monitors.len(),
                slot.x,
                slot.y
            );
        }
        true
    }

    fn next_deadline(&self, now: Instant) -> Option<Instant> {
        // 藏在托盘里：只有一个闹钟有意义 —— 轮询外壳（热键/托盘/第二实例）。
        // 眨眼、呼吸、悬停计时全部停摆，否则一个"看不见"的程序还在按 20fps 空转。
        if !self.visible {
            return Some(self.next_poll);
        }
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

        // 剪贴板序号闹钟：只为“看一眼有没有变”，不涉及重绘。
        consider(self.next_clipboard);
        // 光标轮询闹钟：不是为了重绘，而是为了定期问一次光标位置。
        // 注意必须排在 `dirty` 后面、并且在上面的轮询之后计算，否则会反复到期。
        consider(self.next_poll);
        // 悬停满 hover_delay 要展开：必须定个闹钟把自己叫醒
        if !self.expanded {
            if let Some(since) = self.hover_since {
                consider(since + self.hover_delay());
            }
        }
        // 已经判定"远了"，等 collapse_delay 到点收起：不排这个闹钟就会永远展开。
        if self.expanded {
            if let Some(since) = self.far_since {
                consider(since + self.collapse_delay());
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
        // 悬停热区：只看本体大小，与环带宽度无关（`hover_radius_factor`）。
        let inside_buddy = logical.distance(center)
            <= self
                .config
                .interaction
                .hover_radius(self.config.buddy.radius);

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
        self.poll_clipboard(now);
        self.trace_input(now, inside);
        // 窗口内高频（不漏点击）、窗口外低频（省 CPU）
        self.next_poll = now
            + if inside {
                CURSOR_POLL_INSIDE
            } else {
                CURSOR_POLL_OUTSIDE
            };
        crate::watchdog::polled();
    }
    /// 看剪贴板有没有变。
    ///
    /// 不用 `AddClipboardFormatListener`（要找窗口收消息），而是间一次序号：
    /// 这是唯一一种能真正做到"空闲零成本"的监听方式。
    ///
    /// 两个必须小心的地方：
    /// 1. **启动时只记基线**。否则程序一开就把当前剪贴板内容当历史记上，
    ///    用户会觉得“这东西擅自把我复制的东西存下来了”。
    /// 2. **自己是也会写剪贴板**（复制二维码结果、从历史里挑一条），
    ///    那也会推进序号 —— 所以同一条内容可能被再记一次，靠历史里的去重
    ///    兜住（不会重复占一行）。
    fn poll_clipboard(&mut self, now: Instant) {
        crate::watchdog::enter(crate::watchdog::stage::POLL_CLIPBOARD);
        if now < self.next_clipboard {
            return;
        }
        self.next_clipboard = now + CLIPBOARD_POLL;

        if !self.config.clipboard.enabled {
            // 关掉时把基线也丢掉：重新打开后第一拍只记基线，
            // 否则关着的这段时间里复制的最后一条会被“补记”进来。
            self.clipboard_seq = None;
            return;
        }
        let Some(seq) = zouwu_platform::clipboard::sequence() else {
            return;
        };
        let previous = self.clipboard_seq;
        self.clipboard_seq = Some(seq);
        // MSRV 1.80：`is_none_or` 是 1.82 才稳定的，别用。
        if previous.map_or(true, |old| old == seq) {
            return;
        }
        // 图片/文件之类没有统一文本格式：这拍就当没看到，不报错。
        let Some(text) = zouwu_platform::clipboard::get_text() else {
            return;
        };
        let now_secs = unix_now();
        let outcome = self.history.push(&text, now_secs);
        if outcome == Push::Ignored {
            return;
        }
        // **任何变化都要落盘**，不只是新增：重复内容被提到最前也是一种变化，
        // 不写盘的话内存里顺序对了、重启又变回去。
        zouwu_platform::save_history(&self.history);
        if self.history_window.is_some() {
            self.refresh_history_window();
        }
        match outcome {
            Push::Added => log::info!(
                "clipboard: captured {} chars ({} entries)",
                text.chars().count(),
                self.history.len()
            ),
            Push::Moved => log::info!("clipboard: re-used an existing entry (moved to top)"),
            Push::Ignored => {}
        }
    }

    fn refresh_history_window(&mut self) {
        let entries = self.history.entries.clone();
        if let Some(window) = self.history_window.as_mut() {
            window.refresh(entries);
        }
    }

    /// 摆出剪贴板历史窗。
    fn open_history(&mut self, event_loop: &ActiveEventLoop) {
        crate::watchdog::enter(crate::watchdog::stage::OPEN_HISTORY);
        log::info!(
            "app: open history (was open: {})",
            self.history_window.is_some()
        );
        let entries = self.history.entries.clone();
        if let Some(window) = self.history_window.as_mut() {
            window.show(entries);
            log::info!(
                "clipboard: history window reused ({} entries)",
                self.history.len()
            );
            return;
        }
        let window = HistoryWindow::new(
            event_loop,
            entries,
            self.theme.clone(),
            self.i18n.t("history.title"),
            self.i18n.t("history.footer"),
            self.i18n.t("history.footer-empty"),
            self.i18n.t("history.empty"),
        );
        match window {
            Ok(window) => {
                self.history_window = Some(window);
                log::info!(
                    "clipboard: history window opened ({} entries)",
                    self.history.len()
                );
            }
            Err(err) => log::error!("cannot open history window: {err}"),
        }
    }

    fn history_event(&mut self, event: WindowEvent) {
        crate::watchdog::enter(crate::watchdog::stage::HISTORY_EVENT);
        log::trace!("app: history event {event:?}");
        let mut close = false;
        let mut remove = None;
        {
            let Some(window) = self.history_window.as_mut() else {
                return;
            };
            for outcome in window.window_event(&event) {
                match outcome {
                    HistoryOutcome::Copied(text) => {
                        log::info!("clipboard: re-copied {} chars", text.chars().count());
                        close = true;
                    }
                    HistoryOutcome::Delete(index) => remove = Some(index),
                    HistoryOutcome::Close => close = true,
                }
            }
        }
        if let Some(index) = remove {
            self.history.remove(index);
            zouwu_platform::save_history(&self.history);
            self.refresh_history_window();
        }
        if close {
            crate::watchdog::enter(crate::watchdog::stage::CLOSE_WINDOW);
            if let Some(window) = self.history_window.as_mut() {
                window.close();
            }
            self.last_child_closed_at = Instant::now();
            self.buttons_down = (
                zouwu_platform::layer::left_button_down(),
                zouwu_platform::layer::right_button_down(),
            );
            self.next_poll = Instant::now();
        }
    }

    /// 「为什么它没反应」的现场记录：`ZOUWU_INPUT=1` 时每 500ms 一行。
    ///
    /// 记录的是**判定依据**而不是结论：光标屏幕坐标、窗口原点、缩放、换算后的
    /// 本地坐标、是否在窗口内、按键状态、`visible`、`drag`、`expanded`。
    /// 光看结论（"没反应"）永远猜不出是哪一步断的。
    fn trace_input(&mut self, now: Instant, inside: bool) {
        if std::env::var_os("ZOUWU_INPUT").is_none() {
            return;
        }
        if now.duration_since(self.last_input_trace) < Duration::from_millis(500) {
            return;
        }
        self.last_input_trace = now;
        let cursor = zouwu_platform::layer::cursor_position();
        let window = self.window.clone();
        let (origin, scale) = match window.as_ref() {
            Some(w) => (w.outer_position().unwrap_or_default(), w.scale_factor()),
            None => (PhysicalPosition::new(0, 0), 1.0),
        };
        log::info!(
            "input: cursor {:?} origin {},{} scale {scale} inside {inside} left {} right {} visible {} expanded {} drag {} cursor_state {:?}",
            cursor,
            origin.x,
            origin.y,
            zouwu_platform::layer::left_button_down(),
            zouwu_platform::layer::right_button_down(),
            self.visible,
            self.expanded,
            self.drag.is_some(),
            self.cursor,
        );
    }

    /// 主动问系统要光标/按键状态，作为窗口鼠标事件的兑底。
    ///
    /// 返回光标是否落在窗口矩形内。坐标换算：`GetCursorPos` 给的是屏幕物理像素，
    /// 减去窗口左上角（也是物理像素）再除以 DPI 缩放就是本地逻辑坐标。
    fn poll_pointer(&mut self, now: Instant) -> bool {
        if !self.visible {
            // 窗口藏着的时候，光标相对窗口的位置没有意义（也不该触发悬停/点击）。
            self.on_cursor_left();
            self.buttons_down = (false, false);
            return false;
        }
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
        log::trace!("poll: screen {sx},{sy} origin {},{} size {w:.0}x{h:.0} local {:.1},{:.1} inside {inside}", origin.x, origin.y, local.x, local.y);

        let left = zouwu_platform::layer::left_button_down();
        let right = zouwu_platform::layer::right_button_down();

        // 拖动优先，而且必须在 `inside` 判定之前：拖快了光标一定跑到窗口外面，
        // 那时候如果先走“光标离开”分支，窗口会卡在半路。
        if let Some(mut drag) = self.drag.take() {
            if left {
                if let Some((nx, ny)) = drag.track((sx, sy)) {
                    window.set_outer_position(PhysicalPosition::new(nx, ny));
                    self.state_pos = Some((nx, ny));
                    log::trace!("drag: cursor {sx},{sy} -> {nx},{ny}");
                }
                self.drag = Some(drag);
                self.buttons_down = (left, right);
                // 拖动期间保持高频轮询：跟手比省 CPU 重要。
                return true;
            }
            // 松手：动过 = 搬完了；没动过 = 这就是一次点击。
            self.buttons_down = (left, right);
            let last = window.outer_position().unwrap_or_default();
            match drag.finish((last.x, last.y)) {
                DragOutcome::Moved { x, y } => {
                    self.state_pos = Some((x, y));
                    self.state_pending = true;
                    log::debug!("drag finished at {x},{y}");
                }
                DragOutcome::Click => self.toggle_from_click(now),
            }
        }

        // 仅阻止穿透到被我们自己的面板盖住的小兽；拖动一旦开始则继续跟踪。
        let covered_by_panel = inside
            && (self.settings.is_some() || self.report.is_some() || self.history_window.is_some())
            && zouwu_platform::layer::window_at(sx, sy).is_some_and(|hwnd| {
                let id = WindowId::from(hwnd as u64);
                self.settings
                    .as_ref()
                    .is_some_and(|p| p.is_visible() && p.id() == id)
                    || self
                        .report
                        .as_ref()
                        .is_some_and(|p| p.is_visible() && p.id() == id)
                    || self
                        .history_window
                        .as_ref()
                        .is_some_and(|p| p.is_visible() && p.id() == id)
            });
        if !inside || covered_by_panel {
            if self.cursor.is_some() {
                log::debug!(
                    "cursor outside: screen {sx},{sy} origin {:?} size {w}x{h} scale {scale} local {:?}",
                    (origin.x, origin.y),
                    (local.x, local.y)
                );
                self.on_cursor_left();
            }
            self.buttons_down = (left, right);
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
        if self.hovered_child.take().is_some() {
            self.dirty = true;
        }
    }

    /// 按住本体：记下起点，等后续轮询判断是「点击」还是「拖动」。
    fn begin_drag(&mut self) {
        let (Some((sx, sy)), Some(window)) = (
            zouwu_platform::layer::cursor_position(),
            self.window.clone(),
        ) else {
            return;
        };
        let origin = window.outer_position().unwrap_or_default();
        self.drag = Some(Drag::begin((sx, sy), (origin.x, origin.y)));
        // 手动介入 = 解除钉住：拖动时环会收起来（见下），之后照常自动收起。
        self.pinned = false;
        log::debug!(
            "drag start: cursor {sx},{sy} window {}x{} at {},{} scale {}",
            window.inner_size().width,
            window.inner_size().height,
            origin.x,
            origin.y,
            window.scale_factor()
        );
        // 握住它的那一刻先把圈收起来，否则气泡会追着窗口跑，看着很脏。
        self.set_expanded(false, Instant::now());
        self.hover_since = None;
    }

    /// 一次真正的点击（按下与松开之间没有越过拖动阈值）。
    fn toggle_from_click(&mut self, now: Instant) {
        if self.cursor.is_none() {
            return;
        }
        // 手动点击就是这个开关的主人：点开之后不再钉住，靠悬停维持；
        // 点收起之后也别再自动弹回来。
        self.pinned = false;
        self.set_expanded(!self.expanded, now);
        // 重置计时：刚收起时不该因为光标还停在那里又立刻展开。
        self.hover_since = Some(now);
        log::debug!("click: expanded -> {}", self.expanded);
    }

    fn on_click(&mut self, button: MouseButton, now: Instant) {
        let Some(pos) = self.cursor else { return };
        let center = self.center();
        let on_buddy = pos.distance(center) <= self.config.buddy.radius;

        match button {
            MouseButton::Left => {
                if let Some(child) = self.beam_hit(pos) {
                    // 激光条优先于气泡：牌子可能压在球上，用户点的是字。
                    if let Some(parent) = self.open_children {
                        log::info!("beam activated: item {parent} child {child}");
                        self.pending_child = Some((parent, child));
                        self.close_children();
                    }
                } else if let Some(idx) = self.hover_test(pos) {
                    if self.items.get(idx).is_some_and(|item| item.has_children()) {
                        // 有子项的球：点一下开激光条，再点一下收起，不执行它自己的命令。
                        if self.open_children == Some(idx) {
                            self.close_children();
                        } else {
                            self.open_children_of(idx);
                        }
                    } else if let Some(item) = self.items.get(idx) {
                        let (id, label) = (item.id.clone(), item.label.clone());
                        log::info!("bubble activated: {id} ({label})");
                        self.close_children();
                        self.pending_action = Some(id.to_string());
                    }
                } else if on_buddy {
                    // 按住本体先别急着展开：这次按住可能是想**搬动**它。
                    // 松手时若没越过拖动阈值，才当作一次点击（见 `toggle_from_click`）。
                    self.begin_drag();
                }
            }
            MouseButton::Right => {
                // M1 换成托盘友好的上下文菜单；M0 先收起
                self.pinned = false;
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
        crate::watchdog::enter(crate::watchdog::stage::RENDER);
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
                icon: self.icons.get(i).and_then(|icon| icon.as_ref()),
                selected: self.hovered_bubble == Some(i),
            });
        }

        // 激光条：只画当前张开的那一颗球的子项。文字牌上的字就是用户自己
        // 写的子项名，所以标签直接从条目里借，不复制。
        let mut beam_paints: Vec<BeamPaint<'_>> = Vec::new();
        if let Some(parent) = self.open_children {
            if let Some(item) = self.items.get(parent) {
                for beam in &self.beams {
                    let label = item
                        .children
                        .get(beam.index)
                        .map(|child| child.display_name())
                        .unwrap_or("");
                    beam_paints.push(BeamPaint {
                        base: beam.base,
                        progress: self.child_anim.value.max(0.0),
                        chip: beam.chip,
                        label,
                        hovered: self.hovered_child == Some(beam.index),
                    });
                }
            }
        }

        let scene = Scene {
            width,
            height,
            scale,
            buddy_center: center,
            buddy_radius: self.config.buddy.radius,
            // 呼吸拆成两半：**幅度**归皮肤（`body_radius` 里乘），相位归这里。
            buddy_scale: self.buddy_scale.value,
            buddy_alpha: 1.0,
            blink: self.blink_amount(now),
            breath: self.breath_amount(now),
            theme: &self.theme,
            bubbles: &paints,
            beams: &beam_paints,
            skin: Some(&self.skin),
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

/// 这类变更会让“哪些槽位该注册”发生变化。
///
/// `Ring` 也算：增删球、换次序、关掉一颗、改某颗球自己的热键，**全部**落在这个
/// 变更上（`RingCfg` 整段比较）。只盯 `Hotkey` 的话，面板里改球的键只会存盘、
/// 运行期永远不生效。
fn touches_hotkeys(kind: ChangeKind) -> bool {
    matches!(kind, ChangeKind::Hotkey | ChangeKind::Ring)
}

/// 注册 `raw`（形如 `"Alt+Space"`）描述的全局热键。
///
/// 单独抽出来是因为它同时被“启动”和“热重载”两条路径使用。
/// 配置里的热键写错时**不能**让用户失去唤醒手段，所以回退到默认热键；
/// 空串是合法的“我不要热键”，不注册也不抱怨。
fn apply_hotkey_setting(
    slot: Slot,
    raw: &str,
    registrar: &mut HotkeyRegistrar,
) -> Result<(), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        registrar.clear(slot);
        log::info!("hotkey for {slot:?} disabled by config");
        return Ok(());
    }
    let spec = match Hotkey::parse(raw) {
        Ok(spec) => spec,
        Err(err) => {
            log::warn!("invalid hotkey `{raw}` ({err}); falling back to default");
            match slot {
                // 呼出热键是"救命"通道，写错了必须回退到默认，否则用户打不开小兽。
                Slot::Summon => Hotkey::default_summon(),
                // 球的热键是锦上添花：配置写错就干脆不注册，别占一个用户没要的键。
                Slot::RingItem(_) => {
                    registrar.clear(slot);
                    return Err(format!("hotkey `{raw}` invalid ({err}); not registered"));
                }
            }
        }
    };
    registrar.set(slot, &spec)
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let size = self.config.window.size;
        // 高 DPI 下"逻辑 340"会变成上千物理像素，光栅成本按面积涨。
        // 超过上限就按上限开窗（画布变小、内容等比不变形——只是整个小工具小一点），
        // 4K × 400% 这种极端缩放才可能触发。
        let max_phys = MAX_PHYSICAL_SIDE as f64;
        let scale = event_loop
            .primary_monitor()
            .map(|m| m.scale_factor())
            .unwrap_or(1.0);
        let desired = size as f64 * scale;
        let logical = if desired > max_phys {
            let capped = max_phys / scale;
            log::warn!(
                "window {size} logical at {scale}x = {desired:.0} px exceeds cap {max_phys:.0};                  opening at {capped:.0} logical instead"
            );
            capped
        } else {
            size as f64
        };
        let attrs = Window::default_attributes()
            .with_title("zouwu")
            .with_inner_size(LogicalSize::new(logical, logical))
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_skip_taskbar(true)
            .with_visible(true);

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(err) => {
                log::error!("create window failed: {err}");
                event_loop.exit();
                return;
            }
        };

        // 位置：优先回到上次的位置；没有记录时停在主屏右侧偏下的「宠物位」。
        // 两种来源都要过一遍可见性检查——上次的位置可能属于一块已经不存在的屏。
        let saved = if self.config.window.remember_position {
            zouwu_platform::load_window_state()
        } else {
            None
        };
        if let Some(WindowState { x, y }) = saved {
            window.set_outer_position(PhysicalPosition::new(x, y));
            self.state_pos = Some((x, y));
        }
        self.ensure_visible_on(event_loop, &window, saved.is_some());

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

        // 窗口存在之后才能拿 HWND：托盘、隐藏态、置前都依赖它。
        if !self.visible {
            if let Some(hwnd) = self.hwnd() {
                zouwu_platform::layer::set_visible(hwnd, false);
            }
        }
        self.init_shell();

        if self.visible {
            // 启动就显示也算“呼出”一次：`set_visible` 走不到（`visible` 一开始就是
            // true），所以这里补一次同样的“呼出即展开 + 钉住”。
            if self.config.interaction.expand_on_summon && !self.items.is_empty() {
                self.set_expanded(true, Instant::now());
                self.pinned = true;
                log::debug!("startup: ring expanded and pinned");
            }
            if let Some(w) = self.window_arc() {
                w.request_redraw();
            }
        } else {
            log::info!("zouwu started hidden (tray only)");
        }
    }

    /// 事件循环收尾：位置与配置都落盘（托盘"退出"、Ctrl+C、系统注销都会走到这里）。
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.save_state();
        self.save_config();
        // 遮罩窗口是我们自己建的，退出前收掉（不依赖进程退出时的系统清理）。
        zouwu_platform::eyecare::shutdown();
        // 鼠标钩子同理：给钩子线程投个退出消息，让它自己卸钩子。
        zouwu_platform::altdrag::shutdown();
        log::info!("zouwu exiting");
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        crate::watchdog::enter(crate::watchdog::stage::WINDOW_EVENT);
        // 面板是另一个窗口：按 id 分流，别把它的鼠标事件当成分层窗口的。
        if self.settings.as_ref().is_some_and(|p| p.id() == id) {
            self.settings_event(event_loop, event);
            return;
        }
        // 结果小窗同理。
        if self.report.as_ref().is_some_and(|r| r.id() == id) {
            self.report_event(event);
            return;
        }
        // 剪贴板历史窗同理。
        if self.history_window.as_ref().is_some_and(|h| h.id() == id) {
            self.history_event(event);
            return;
        }
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
                // 分层窗口（尤其是 `UpdateLayeredWindow` 之后）会**误报** CursorLeft：
                // 实测刚展开就收到一个，然后把光标当成消失了 → 立刻收起 → 气泡永远点不到。
                // 所以这里只把它当成"现在值得复核一下"，真假交给 16ms 的光标轮询判断。
                self.next_poll = now;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed {
                    self.click(button, now);
                } else if button == MouseButton::Left {
                    // 事件与轮询两条路都会到达这里；先动手的那个把 `drag` 取走，
                    // 后到的就 naturally no-op（免去再加一个去重窗口）。
                    if let Some(drag) = self.drag.take() {
                        let last = self
                            .window
                            .as_ref()
                            .and_then(|w| w.outer_position().ok())
                            .unwrap_or_default();
                        match drag.finish((last.x, last.y)) {
                            DragOutcome::Moved { x, y } => {
                                self.state_pos = Some((x, y));
                                self.state_pending = true;
                            }
                            DragOutcome::Click => self.toggle_from_click(now),
                        }
                    }
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
                        // 子窗口刚关闭或仍处于打开状态时，Esc 不属于小兽本体。
                        if now.duration_since(self.last_child_closed_at)
                            < Duration::from_millis(500)
                            || self.settings.as_ref().is_some_and(|p| p.is_visible())
                            || self.report.as_ref().is_some_and(|p| p.is_visible())
                            || self.history_window.as_ref().is_some_and(|p| p.is_visible())
                        {
                            return;
                        }
                        if self.expanded {
                            self.set_expanded(false, now);
                        } else {
                            // 小工具的心智：Esc 是"收起来"，不是"杀掉我"。
                            // 真的退出走托盘菜单（上面写着"退出"）。
                            self.set_visible(false, now);
                        }
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
                // 关窗 = 缩进托盘（ADR-010）
                self.set_visible(false, now);
            }
            _ => {}
        }
    }

    /// 空闲调度：把「下一次必须重绘的时刻」交给事件循环，到点自动醒来。
    /// 没有待办时进入 `Wait`（真正 0 帧、0 CPU），有动画/悬停计时/眨眼计划时
    /// 进入 `WaitUntil`。这是"空闲零开销"与"看起来活着"能同时成立的关键。
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        crate::watchdog::enter(crate::watchdog::stage::ABOUT_TO_WAIT);
        if std::env::var_os("ZOUWU_HANG").is_some_and(|v| !v.is_empty()) {
            log::warn!("debug: sleeping 20s to test the watchdog");
            std::thread::sleep(Duration::from_secs(20));
        }
        let now = Instant::now();

        // 清墓地：这里已经出了事件派发（不在窗口过程里），析构 `Window` 不会重入。
        // 见 `retired` 字段的注释——这是修“开过二维码窗再开历史窗就卡死”的关键。
        if !self.retired.is_empty() {
            log::info!("app: draining {} retired window(s)", self.retired.len());
            self.retired.clear();
            log::info!("app: drain done");
        }

        // 轮询只是一种"叫醒输入"，它本身不应该产生任何重绘：
        // 开一枪 GetCursorPos 几微秒，醒来发现画面没变就直接接着睡。
        if now >= self.next_poll {
            self.poll_now(now);

            // 显示器状态搭同一班车：拔屏/改分辨率不会发通知给 winit，
            // 而这条 100ms 的闹钟本来就在跑，搭车的代价是每 3 秒一次枚举。
            if now.duration_since(self.last_monitor_check) >= MONITOR_CHECK {
                self.last_monitor_check = now;
                if let Some(window) = self.window_arc() {
                    // 激光条把窗口撑大了就别查位置：撑大的窗口天生伸出屏幕，
                    // 按它算"露出多少"会把本体重摆一遍（实测每 3 秒的这只闹钟
                    // 会在牌子开着时把本体拽走 20~80px，而且看运气——取决于点开
                    // 牌子离下一次闹钟多近）。
                    if self.open_children.is_none() {
                        let had_position = self.state_pos.is_some();
                        if self.ensure_visible_on(event_loop, &window, had_position) {
                            self.dirty = true;
                            window.request_redraw();
                        }
                    }
                }
                // 遮罩盖的是整个虚拟桌面：换了分辨率/插拔了屏，几何得重贴一次。
                if self.config.eye_care.enabled {
                    self.apply_eye_care("monitors", false);
                }
            }
        }

        // 托盘点了「回到主屏」：无视当前可见性，强制回宠物位。
        if std::mem::take(&mut self.pending_recenter) {
            if let Some(window) = self.window_arc() {
                self.ensure_visible_on(event_loop, &window, false);
                self.dirty = true;
                window.request_redraw();
            }
        }

        // 气泡/热键记下的动作在这里执行：建窗口要 `&ActiveEventLoop`，
        // 而点击处理（`on_click`）与热键轮询（`poll_shell`）都拿不到它。
        if let Some((parent, child)) = self.pending_child.take() {
            self.run_child(event_loop, parent, child);
        }
        if let Some(id) = self.pending_action.take() {
            self.run_action(event_loop, &id);
        }

        // 外壳（托盘/热键/第二实例）与光标轮询同节拍：一次唤醒问完所有事。
        if self.poll_shell(now) {
            self.save_state();
            self.save_config();
            event_loop.exit();
            return;
        }

        self.flush_state(now);

        // `ZOUWU_SETTINGS=1` 启动即开面板：托盘菜单没法脚本化，而"面板能不能
        // 打开"是回归时最该被一键验证的一条。**只触发一次**——否则用户关掉
        // 面板后它会被立刻重新打开，看起来就像关不掉。
        // 只认非空值：`ZOUWU_SETTINGS=`（空字符串）在 shell 里很常见，
        // 不该因此弹出一个用户没要的面板。
        let want_panel = std::env::var_os("ZOUWU_SETTINGS").is_some_and(|v| !v.is_empty());
        if !self.scan_env_done && std::env::var_os("ZOUWU_SCAN").is_some_and(|v| !v.is_empty()) {
            self.scan_env_done = true;
            self.pending_action = Some(QRCODE_ACTION.to_string());
        }
        if !self.history_env_done
            && std::env::var_os("ZOUWU_HISTORY").is_some_and(|v| !v.is_empty())
        {
            self.history_env_done = true;
            self.pending_action = Some(CLIPBOARD_ACTION.to_string());
        }
        if !self.settings_env_done && want_panel {
            self.settings_env_done = true;
            self.pending_settings = true;
        }
        if std::mem::take(&mut self.pending_settings) {
            self.open_settings(event_loop, now);
        }
        self.cycle_tick(event_loop, now);

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

/// 把一次扫描结果翻成人话。
fn build_report(
    i18n: &I18n,
    scan: &qr::Scan,
    capture: &zouwu_platform::screen::Capture,
    elapsed_ms: u64,
    monitors: &[Rect],
) -> Report {
    let elapsed = i18n.t_args("qr.elapsed", &[("ms", &elapsed_ms.to_string())]);
    if scan.hits.is_empty() {
        let detail = i18n.t_args(
            "qr.none-detail",
            &[
                ("width", &capture.width.to_string()),
                ("height", &capture.height.to_string()),
            ],
        );
        return Report {
            headline: i18n.t("qr.none"),
            detail: Some(format!("{detail} · {elapsed}")),
            entries: Vec::new(),
            footer: i18n.t("qr.footer"),
        };
    }

    let headline = i18n.t_args("qr.found", &[("count", &scan.hits.len().to_string())]);
    let mut detail = elapsed;
    if scan.unreadable > 0 {
        // "找到了但读不出来"和"一个都没有"是两件事，混成一句会让用户到处找码。
        detail = format!(
            "{detail} · {}",
            i18n.t_args("qr.unreadable", &[("count", &scan.unreadable.to_string())])
        );
    }
    let entries = scan
        .hits
        .iter()
        .map(|hit| {
            let center_x = hit.rect.0 + hit.rect.2 / 2;
            let center_y = hit.rect.1 + hit.rect.3 / 2;
            let (dx, dy) = capture.to_desktop(center_x, center_y);
            Entry {
                text: hit.text.clone(),
                screen: screen_label(i18n, monitors, dx, dy),
            }
        })
        .collect();
    Report {
        headline,
        detail: Some(detail),
        entries,
        footer: i18n.t("qr.footer"),
    }
}

fn failure_report(i18n: &I18n, headline_key: &str, reason: &str) -> Report {
    Report {
        headline: i18n.t(headline_key),
        detail: Some(reason.to_string()),
        entries: Vec::new(),
        footer: i18n.t("qr.footer"),
    }
}

/// 命中的码落在第几块屏上。多屏时用户才知道该去关哪个窗口。
/// 当前 Unix 秒。剪贴板历史只需要"多久以前"这个粒度。
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn screen_label(i18n: &I18n, monitors: &[Rect], x: i32, y: i32) -> String {
    let p = Point::new(x as f32, y as f32);
    let index = monitors.iter().position(|m| m.contains(p));
    match index {
        Some(i) => i18n.t_args("qr.screen", &[("index", &(i + 1).to_string())]),
        None => String::new(),
    }
}

/// 环上的球完全由配置决定（ADR-042）。
///
/// 这里最容易出的错不是"少了球"，而是 `items` / `anims` / `icons` / `bubbles`
/// 四份数组长度不同步 —— 少一颗球就 index panic，多一颗球就画到屏幕外。
#[cfg(test)]
mod ring_tests {
    use super::*;
    use zouwu_core::action::{CLIPBOARD_ID, QRCODE_ID};
    use zouwu_core::config::{RingChild, RingItem};

    fn app_with(items: Vec<RingItem>) -> App {
        let mut config = Config::default();
        config.ring.items = items;
        app_from(config)
    }

    fn app_from(mut config: Config) -> App {
        config.clipboard.enabled = false;
        let theme = Theme::builtin("zouwu-warm").unwrap();
        let instance = InstanceSignal::acquire(&format!(
            "ring-test-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ))
        .unwrap()
        .unwrap();
        App::new(config, theme.clone(), vec![theme], instance)
    }

    fn custom_with_children() -> RingItem {
        let mut custom = RingItem::custom("custom.1");
        custom.label = "编辑器".into();
        custom.command = "notepad.exe".into();
        custom.children = vec![
            RingChild {
                label: "开 IDE".into(),
                command: "code".into(),
            },
            RingChild {
                // 名字留空 → 牌子上直接用命令，不能显示成空白
                label: String::new(),
                command: "wt.exe".into(),
            },
        ];
        custom
    }

    /// 点开有子项的球 → 激光条几何算出来；关掉就清空。
    #[test]
    fn opening_a_bubble_lays_out_its_children_as_beams() {
        let mut app = app_with(vec![custom_with_children()]);
        assert!(app.items[0].has_children());
        assert_eq!(app.items[0].children[1].display_name(), "wt.exe");

        app.set_expanded(true, Instant::now());
        app.relayout();
        app.open_children_of(0);
        assert_eq!(app.open_children, Some(0));
        assert_eq!(app.beams.len(), 2, "两个子项就该有两条激光条");
        assert_eq!(app.child_anim.target, 1.0);

        // 还在长的过程中不该能点中：牌子上半截还是空的
        let chip = app.beams[0].chip.center();
        assert_eq!(app.beam_hit(chip), None);
        app.child_anim.snap(1.0);
        assert_eq!(app.beam_hit(chip), Some(0));
        assert_eq!(app.beam_hit(Point::new(-50.0, -50.0)), None);

        app.close_children();
        assert_eq!(app.open_children, None);
        assert_eq!(app.child_anim.target, 0.0);
    }

    /// 点牌子 → 排队执行那个子项、并且把激光条收掉（片子不该赖在屏幕上）。
    #[test]
    fn clicking_a_beam_queues_its_child() {
        let mut app = app_with(vec![custom_with_children()]);
        app.set_expanded(true, Instant::now());
        app.relayout();
        app.open_children_of(0);
        app.child_anim.snap(1.0);

        let chip = app.beams[1].chip.center();
        app.cursor = Some(chip);
        app.on_click(MouseButton::Left, Instant::now());

        assert_eq!(app.pending_child, Some((0, 1)), "点第二块牌子要排第二子项");
        assert_eq!(app.open_children, None, "执行完就没必要继续摆着");
        assert!(app.beams.is_empty());
        assert_eq!(app.pending_action, None, "子项不该顺手触发父项命令");
    }

    /// 再点一下同一颗球是收起；点别的球是换一颗展开。
    #[test]
    fn clicking_the_bubble_again_closes_and_another_one_switches() {
        let mut other = custom_with_children();
        other.id = "custom.2".into();
        let mut app = app_with(vec![custom_with_children(), other]);
        app.set_expanded(true, Instant::now());
        app.relayout();

        let first = app.bubbles[0].center;
        app.cursor = Some(first);
        app.on_click(MouseButton::Left, Instant::now());
        assert_eq!(app.open_children, Some(0), "第一次点：展开");
        assert_eq!(app.pending_action, None, "有子项的球不执行自己的命令");

        app.cursor = Some(app.bubbles[0].center);
        app.on_click(MouseButton::Left, Instant::now());
        assert_eq!(app.open_children, None, "再点一下：收起");

        app.cursor = Some(app.bubbles[1].center);
        app.on_click(MouseButton::Left, Instant::now());
        assert_eq!(app.open_children, Some(1), "点另一颗：换它展开");
    }

    /// 内置球没有子项：点它不能凭空长出激光条。
    #[test]
    fn a_bubble_without_children_has_no_beams() {
        let mut app = app_with(vec![RingItem::builtin(QRCODE_ID)]);
        app.set_expanded(true, Instant::now());
        app.open_children_of(0);
        assert_eq!(app.open_children, None);
        assert!(app.beams.is_empty());

        // 下标越界也要安静走掉（热重载删了球、菜单还指着旧下标）
        app.open_children_of(9);
        assert_eq!(app.open_children, None);
    }

    /// 收起环时激光条必须跟着走：它挂在球上，球都没了。
    #[test]
    fn collapsing_the_ring_closes_the_beams() {
        let mut app = app_with(vec![custom_with_children()]);
        app.set_expanded(true, Instant::now());
        app.open_children_of(0);
        assert_eq!(app.open_children, Some(0));
        app.set_expanded(false, Instant::now());
        assert_eq!(app.open_children, None);
        assert_eq!(app.child_anim.target, 0.0);
    }

    /// 呼出即展开：`expand_on_summon` 打开后，呼出就展开并钉住。
    #[test]
    fn summon_unfolds_and_pins_the_ring_when_configured() {
        let mut config = Config::default();
        config.general.show_on_start = false;
        config.interaction.expand_on_summon = true;
        config.ring.items = vec![custom_with_children()];
        let mut app = app_from(config);
        let now = Instant::now();

        app.set_visible(true, now);
        assert!(
            app.visible && app.expanded && app.pinned,
            "呼出即展开并钉住"
        );

        // 钉住之后光标飞远也不收：需求就是"直到手动收"
        app.cursor = Some(Point::new(4000.0, 4000.0));
        app.far_since = Some(now);
        app.update(now + Duration::from_secs(1));
        assert!(app.expanded, "钉住时自动收起必须失效");

        // 再点小兽 → 收起，并且不再钉住
        app.cursor = Some(app.center());
        app.toggle_from_click(now);
        assert!(!app.expanded && !app.pinned);
    }

    /// 默认（`expand_on_summon = false`）呼出不展开，行为与以前一致。
    #[test]
    fn summon_keeps_the_ring_closed_by_default() {
        let mut config = Config::default();
        config.general.show_on_start = false;
        config.ring.items = vec![RingItem::builtin(QRCODE_ID)];
        let mut app = app_from(config);
        app.set_visible(true, Instant::now());
        assert!(app.visible);
        assert!(!app.expanded, "默认不该自动展开");
        assert!(!app.pinned);
    }

    /// 呼出时不展开、但环上没球（全部关掉）也不该钉住一个空环。
    #[test]
    fn summon_pins_nothing_when_the_ring_is_empty() {
        let mut config = Config::default();
        config.general.show_on_start = false;
        config.interaction.expand_on_summon = true;
        config.ring.items = vec![RingItem::builtin(QRCODE_ID)];
        let mut app = app_from(config);
        for item in &mut app.config.ring.items {
            item.enabled = false;
        }
        app.resolve_items();
        app.set_visible(true, Instant::now());
        assert!(!app.expanded && !app.pinned);
    }

    /// 躲进托盘时一切归零：下次呼出是干净状态。
    #[test]
    fn hiding_clears_the_pin_and_the_beams() {
        let mut config = Config::default();
        config.general.show_on_start = false;
        config.interaction.expand_on_summon = true;
        config.ring.items = vec![custom_with_children()];
        let mut app = app_from(config);
        let now = Instant::now();
        app.set_visible(true, now);
        app.open_children_of(0);
        assert_eq!(app.open_children, Some(0));

        app.set_visible(false, now);
        assert!(!app.visible && !app.expanded && !app.pinned);
        assert_eq!(app.open_children, None, "隐藏时激光条也要关掉");
    }

    #[test]
    fn disabled_items_never_reach_the_ring() {
        let mut off = RingItem::builtin(CLIPBOARD_ID);
        off.enabled = false;
        let app = app_with(vec![RingItem::builtin(QRCODE_ID), off]);
        assert_eq!(app.items.len(), 1, "关掉的球不该上环");
        assert_eq!(app.items[0].id, QRCODE_ID);
        assert_eq!(app.anims.len(), app.items.len());
        assert_eq!(app.icons.len(), app.items.len());
    }

    #[test]
    fn custom_items_keep_their_order_and_command() {
        let mut custom = RingItem::custom("custom.1");
        custom.label = "记事本".into();
        custom.command = "notepad.exe".into();
        custom.icon = "missing.png".into();
        let app = app_with(vec![
            custom,
            RingItem::builtin(CLIPBOARD_ID),
            RingItem::builtin(QRCODE_ID),
        ]);
        // 顺序 = 配置顺序（不是内置顺序）：用户排了就得照排。
        let ids: Vec<&str> = app.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["custom.1", CLIPBOARD_ID, QRCODE_ID]);
        assert_eq!(app.items[0].label, "记事本");
        assert_eq!(app.items[0].command.as_deref(), Some("notepad.exe"));
        // 图标文件不存在：只是这颗球退回字形，不能连球一起没。
        assert_eq!(app.icons.len(), 3);
        assert!(app.icons.iter().all(|icon| icon.is_none()));
    }

    #[test]
    fn changing_the_list_keeps_every_array_in_step() {
        let mut off = RingItem::builtin(CLIPBOARD_ID);
        off.enabled = false;
        let mut app = app_with(vec![RingItem::builtin(QRCODE_ID), off]);
        assert_eq!(app.items.len(), 1);

        // 打开第二颗：多出来那颗必须有一份 0 起步的弹簧，
        // 否则它会"一出生就是展开的"，看着像上帧的残留。
        app.config.ring.items[1].enabled = true;
        app.resolve_items();
        assert_eq!(app.items.len(), 2);
        assert_eq!(app.anims.len(), 2);
        assert_eq!(app.icons.len(), 2);

        // 全部关掉：环上什么都不剩，也不能 panic。
        for item in &mut app.config.ring.items {
            item.enabled = false;
        }
        app.resolve_items();
        assert!(app.items.is_empty());
        assert!(app.anims.is_empty());
        app.relayout();
        assert!(app.bubbles.is_empty());
    }

    /// 面板里改一颗球的热键，`diff` 出来的是 `Ring` 而不是 `Hotkey`。
    /// 若“重注册热键”只认 `Hotkey`，改完只会存盘、运行期永远不生效
    /// （用户看到的是“设置没保存”）。这条测试把那个口径钉死。
    #[test]
    fn every_ring_edit_that_moves_hotkeys_triggers_a_reregistration() {
        let base = Config::default();
        assert!(touches_hotkeys(ChangeKind::Hotkey), "呼出热键");

        let mut retuned = base.clone();
        retuned.ring.items[1].hotkey = "Ctrl+Alt+7".into();
        let changed = base.diff(&retuned);
        assert_eq!(changed, vec![ChangeKind::Ring]);
        assert!(
            changed.iter().any(|kind| touches_hotkeys(*kind)),
            "改了球的键"
        );

        let mut disabled = base.clone();
        disabled.ring.items[0].enabled = false;
        let changed = base.diff(&disabled);
        assert!(
            changed.iter().any(|kind| touches_hotkeys(*kind)),
            "关掉一颗球"
        );

        let mut removed = base.clone();
        removed.ring.items.pop();
        let changed = base.diff(&removed);
        assert!(
            changed.iter().any(|kind| touches_hotkeys(*kind)),
            "删掉一颗球"
        );

        let mut reordered = base.clone();
        reordered.ring.items.reverse();
        let changed = base.diff(&reordered);
        assert!(changed.iter().any(|kind| touches_hotkeys(*kind)), "换次序");

        // 跟热键无关的变更不该顺手把整圈键重新注册（每次注册都有系统开销，
        // 而且“重新注册失败”会让一个本来好用的键暂时失效）。
        let mut quiet = base.clone();
        quiet.interaction.hover_delay_ms += 10;
        assert!(!base.diff(&quiet).iter().any(|kind| touches_hotkeys(*kind)));
    }
}

#[cfg(all(test, target_os = "windows"))]
mod settings_pointer_tests {
    use super::*;
    use winit::dpi::PhysicalSize;
    use winit::platform::windows::EventLoopBuilderExtWindows;

    #[link(name = "user32")]
    extern "system" {
        fn SetWindowPos(
            hwnd: isize,
            after: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
        fn GetWindowLongW(hwnd: isize, index: i32) -> i32;
    }

    /// 窗口现在是不是 `WS_EX_TOPMOST`。桌面上有其他窗口时这是判定
    /// “开面板有没有把小兽弄下来”最可靠的办法。
    fn is_topmost(hwnd: isize) -> bool {
        const GWL_EXSTYLE: i32 = -20;
        const WS_EX_TOPMOST: i32 = 0x0000_0008;
        unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST != 0 }
    }
    fn move_panel(hwnd: isize, x: i32, y: i32) {
        const HWND_TOPMOST: isize = -1;
        const SWP_NOSIZE: u32 = 0x0001;
        const SWP_NOACTIVATE: u32 = 0x0010;
        assert_ne!(
            unsafe { SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE) },
            0
        );
    }

    /// 只改面板的 Z 序层级（置顶/取消置顶），位置尺寸不动。
    fn panel_z(hwnd: isize, topmost: bool) {
        const HWND_TOPMOST: isize = -1;
        const HWND_NOTOPMOST: isize = -2;
        const SWP_NOSIZE: u32 = 0x0001;
        const SWP_NOMOVE: u32 = 0x0002;
        const SWP_NOACTIVATE: u32 = 0x0010;
        let after = if topmost {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        assert_ne!(
            unsafe {
                SetWindowPos(
                    hwnd,
                    after,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
                )
            },
            0
        );
    }

    // 需要交互式 Windows 桌面，会短暂打开设置窗；普通测试不应打扰用户。
    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn pointer_polling_works_with_open_or_reused_panels() {
        struct Test;
        impl ApplicationHandler for Test {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                let (x, y) = zouwu_platform::layer::cursor_position().expect("cursor position");
                assert!(
                    !zouwu_platform::layer::left_button_down(),
                    "release mouse first"
                );
                let instance =
                    InstanceSignal::acquire(&format!("settings-test-{}", std::process::id()))
                        .unwrap()
                        .unwrap();
                let mut config = Config::default();
                config.clipboard.enabled = false;
                let theme = Theme::builtin("zouwu-warm").unwrap();
                let mut app = App::new(config, theme.clone(), vec![theme], instance);
                let window = event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_visible(false)
                            .with_decorations(false)
                            .with_inner_size(PhysicalSize::new(400, 400))
                            .with_window_level(WindowLevel::AlwaysOnTop)
                            .with_position(PhysicalPosition::new(x - 200, y - 200)),
                    )
                    .unwrap();
                app.window = Some(Arc::new(window));
                app.visible = true;

                let now = Instant::now();
                app.poll_now(now);
                assert_eq!(app.next_poll, now + CURSOR_POLL_INSIDE);
                for _ in 0..2 {
                    for kind in 0..3 {
                        match kind {
                            0 => app.open_settings(event_loop, now),
                            1 => app.open_history(event_loop),
                            _ => app.open_report(event_loop, Report::default()),
                        }
                        let id = match kind {
                            0 => app.settings.as_ref().unwrap().id(),
                            1 => app.history_window.as_ref().unwrap().id(),
                            _ => app.report.as_ref().unwrap().id(),
                        };
                        let hwnd = u64::from(id) as isize;
                        // ADR-039：面板打开/复用不得改变小兽的置顶层。
                        // （不能用 `window_at` 验层级：分层窗口未渲染时对命中测试
                        // 是全透明的，`WindowFromPoint` 会直接跳过它。）
                        {
                            panel_z(hwnd, false);
                            if let Some(pet) =
                                app.window.as_ref().map(|w| u64::from(w.id()) as isize)
                            {
                                assert!(
                                    is_topmost(pet),
                                    "opening a panel must not drop the pet from topmost (kind={kind})"
                                );
                            }
                        }
                        for covered in [false, true] {
                            // 这个测试跑在真人桌面上，鼠标随时会动：每次都把小兽
                            // 摆到光标下，判定就不再依赖进入测试那一刻的鼠标位置。
                            let (x, y) = zouwu_platform::layer::cursor_position().unwrap();
                            app.window
                                .as_ref()
                                .unwrap()
                                .set_outer_position(PhysicalPosition::new(x - 200, y - 200));
                            let offset = if covered { -100 } else { 100 };
                            move_panel(hwnd, x + offset, y + offset);
                            assert_eq!(
                                zouwu_platform::layer::window_at(x, y) == Some(hwnd),
                                covered,
                                "test requires panel to be the topmost window at the cursor"
                            );
                            // 有无面板遮挡都要继续处理已经开始的拖动和释放。
                            app.drag = Some(Drag {
                                moved: true,
                                ..Drag::begin((x, y), (x - 200, y - 200))
                            });
                            app.poll_now(Instant::now());
                            assert!(
                                app.drag.is_none(),
                                "drag release must be processed with panel open"
                            );
                            assert!(app.state_pending, "dragged position must be saved");
                            app.state_pending = false;

                            let poll_at = Instant::now();
                            app.poll_now(poll_at);
                            if covered {
                                assert!(
                                    app.cursor.is_none(),
                                    "panel must block pointer input behind it"
                                );
                                assert_eq!(app.next_poll, poll_at + CURSOR_POLL_OUTSIDE);
                            } else {
                                assert!(
                                    app.cursor.is_some(),
                                    "uncovered pet must remain interactive: kind={kind}, cycle cursor={:?}, widget origin={:?}, size={:?}, top window={:?}",
                                    zouwu_platform::layer::cursor_position(),
                                    app.window.as_ref().unwrap().outer_position(),
                                    app.window.as_ref().unwrap().inner_size(),
                                    zouwu_platform::layer::window_at(x, y),
                                );
                                assert_eq!(app.next_poll, poll_at + CURSOR_POLL_INSIDE);
                            }
                        }
                        app.window_event(event_loop, id, WindowEvent::CloseRequested);
                        assert!(app.visible, "pet widget must remain visible");
                        {
                            let (x, y) = zouwu_platform::layer::cursor_position().unwrap();
                            app.window
                                .as_ref()
                                .unwrap()
                                .set_outer_position(PhysicalPosition::new(x - 200, y - 200));
                        }
                        app.poll_now(Instant::now());
                        assert!(
                            app.cursor.is_some(),
                            "polling must continue after panel closes"
                        );
                    }
                }
                event_loop.exit();
            }

            fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        }

        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut Test).unwrap();
    }
}

#[cfg(test)]
mod qr_report_tests {
    use super::*;

    fn i18n() -> I18n {
        I18n::new("zh-CN")
    }

    fn capture(w: usize, h: usize) -> zouwu_platform::screen::Capture {
        zouwu_platform::screen::Capture {
            x: 0,
            y: 0,
            width: w,
            height: h,
            rgba: vec![0; w * h * 4],
        }
    }

    /// `(内容, 命中框)` —— 命中框是 `(x, y, w, h)`，与 `qr::Hit` 一致。
    type HitFixture<'a> = (&'a str, (u32, u32, u32, u32));

    fn scan_with(hits: Vec<HitFixture<'_>>, unreadable: usize) -> qr::Scan {
        let mut scan = qr::Scan::default();
        for (text, rect) in hits {
            scan.hits.push(qr::Hit {
                text: text.to_string(),
                rect,
            });
        }
        scan.unreadable = unreadable;
        scan
    }

    #[test]
    fn no_code_says_so_and_states_what_was_scanned() {
        let cap = capture(1920, 1080);
        let report = build_report(&i18n(), &scan_with(Vec::new(), 0), &cap, 41, &[]);
        assert!(report.entries.is_empty());
        assert!(report.headline.contains("没有找到"), "{}", report.headline);
        let detail = report.detail.unwrap_or_default();
        // 用户最容易误判的情形是"我明明看到了码"——所以要写清扫的是多大一块桌面。
        assert!(detail.contains("1920"), "{detail}");
        assert!(detail.contains("41"), "{detail}");
    }

    #[test]
    fn each_hit_gets_its_own_row_and_screen() {
        let cap = capture(3840, 1080);
        let monitors = [
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Rect::new(1920.0, 0.0, 1920.0, 1080.0),
        ];
        // 第二个码落在第二块屏上（虚拟桌面坐标 2400 → 显示器 2）。
        let scan = scan_with(
            vec![
                ("https://a.example", (100, 100, 200, 200)),
                ("https://b.example", (2400, 400, 200, 200)),
            ],
            0,
        );
        let report = build_report(&i18n(), &scan, &cap, 90, &monitors);
        assert_eq!(report.entries.len(), 2);
        assert_eq!(report.entries[0].screen, "显示器 1");
        assert_eq!(report.entries[1].screen, "显示器 2");
        assert!(report.headline.contains('2'), "{}", report.headline);
    }

    #[test]
    fn unreadable_codes_are_reported_apart_from_none() {
        let cap = capture(1280, 720);
        let report = build_report(&i18n(), &scan_with(Vec::new(), 2), &cap, 30, &[]);
        // 有码但读不出来时，标题仍然是"没有找到"（确实没解出内容），
        // 但详情必须把"有 2 个读不出来"说出来，否则用户会以为工具坏了。
        let detail = report.detail.unwrap_or_default();
        assert!(detail.contains('2'), "{detail}");
    }

    #[test]
    fn missing_screen_still_produces_a_row() {
        let cap = capture(800, 600);
        let report = build_report(
            &i18n(),
            &scan_with(vec![("x", (10, 10, 50, 50))], 0),
            &cap,
            1,
            &[],
        );
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].screen, "");
    }

    #[test]
    fn failure_report_carries_the_reason_verbatim() {
        let report = failure_report(&i18n(), "qr.error", "no desktop to capture");
        assert!(report.headline.contains("失败"), "{}", report.headline);
        assert_eq!(report.detail.as_deref(), Some("no desktop to capture"));
    }
}
