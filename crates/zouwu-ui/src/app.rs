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

use zouwu_core::action::{default_ring, ActionItem};
use zouwu_core::anim::Spring;
use zouwu_core::config::{ChangeKind, Config, WindowState};
use zouwu_core::drag::{Drag, DragOutcome};
use zouwu_core::geom::{Point, Rect};
use zouwu_core::hotkey::Hotkey;
use zouwu_core::i18n::I18n;
use zouwu_core::ring::{solve_rotation, Bubble, RingConfig};
use zouwu_core::skin::PROCEDURAL_SKIN_ID;

use crate::settings_window::{Outcome as SettingsOutcome, SettingsWindow};
use crate::skin::Skin;
use zouwu_core::theme::Theme;

use zouwu_platform::autostart;
use zouwu_platform::hotkey::HotkeyRegistrar;
use zouwu_platform::reload::{ConfigWatcher, ReloadEvent};
use zouwu_platform::single_instance::InstanceSignal;
use zouwu_platform::tray::{Tray, TrayCommand, TrayOptions};

use crate::paint::{render, BubblePaint, Scene};
use crate::presenter::Presenter;

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

    let instance = InstanceSignal::acquire("zouwu")?;
    let Some(instance) = instance else {
        log::info!("another zouwu is running; asked it to show up");
        return Ok(());
    };

    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);

    // 配置在启动时读一次（缺文件会写出一份默认配置，方便用户手改）
    let config = zouwu_platform::load_config();
    // 配置里的日志级别/落盘策略现在才生效（RUST_LOG 仍然优先）。
    zouwu_platform::logging::apply(&config.log);
    // 主题/皮肤都是"用户可能写错"的输入：加载失败一律退回内置，不让程序打不开。
    let themes = zouwu_platform::assets::load_themes();
    let theme = zouwu_platform::assets::theme_by_id(&themes, &config.appearance.theme);
    let mut app = App::new(config, theme, themes, default_ring(), instance);
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

struct App {
    config: Config,
    theme: Theme,
    /// 全部可用主题（内置 + 外部）。托盘勾选、换主题都从这里取。
    themes: Vec<Theme>,
    /// 当前皮肤（`procedural` 时没有图层，走代码绘制）
    skin: Skin,
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
    /// 拖动进行中的状态（按住本体 → 越过阈值 → 窗口跟着光标走）
    drag: Option<Drag>,

    /// 窗口位置：`Moved` 事件只记在这里，节流后再落盘（拖动会刷出大量事件）
    state_pos: Option<(i32, i32)>,
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
    /// 窗口当前是否可见（隐藏 = 缩进托盘，进程仍在）。
    visible: bool,

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

    fn new(
        config: Config,
        theme: Theme,
        themes: Vec<Theme>,
        items: Vec<ActionItem>,
        instance: InstanceSignal,
    ) -> Self {
        let ring = config.ring.to_ring_config();
        let skin = load_skin(&config.appearance.skin);
        let n = items.len();
        let i18n = I18n::new(&config.general.language);
        let visible = config.general.show_on_start;
        Self {
            i18n,
            instance: Some(instance),
            tray: None,
            hotkeys: None,
            visible,
            config,
            theme,
            themes,
            skin,
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
            drag: None,
            state_pos: None,
            state_pending: false,
            state_saved_at: Instant::now(),
            perf_trace: std::env::var("ZOUWU_PERF").is_ok(),
            perf_frames: 0,
            perf_mark: Instant::now(),
            perf_started: Instant::now(),
            config_watcher: ConfigWatcher::new(zouwu_platform::config_path()),
            settings: None,
            pending_settings: false,
            settings_env_done: false,
        }
    }

    fn window_arc(&self) -> Option<Arc<Window>> {
        self.window.clone()
    }

    /// 平台的原始窗口句柄（Windows 下是 HWND）。
    ///
    /// 托盘/显隐/置前这些都是**原生窗口概念**，所以在这里取一次句柄交给
    /// `zouwu-platform`，UI 层不直接碰 win32。
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
        }

        if changed.contains(&ChangeKind::Hotkey) {
            if let Some(registrar) = self.hotkeys.as_mut() {
                if let Err(err) = apply_hotkey_setting(&self.config.general.hotkey, registrar) {
                    log::warn!("{err}");
                }
            }
        }

        if changed.contains(&ChangeKind::Autostart) {
            if let Some(tray) = &self.tray {
                tray.set_autostart_checked(self.config.general.start_on_boot);
            }
        }

        if changed.contains(&ChangeKind::Ring) {
            self.ring = self.config.ring.to_ring_config();
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
            // 手改 config.toml 里的 theme/skin 也要当场生效，不必重启。
            self.set_theme(self.config.appearance.theme.clone(), now);
            self.set_skin(self.config.appearance.skin.clone(), now);
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

    /// 打开（或唤到前面）设置面板。
    fn open_settings(&mut self, event_loop: &ActiveEventLoop, _now: Instant) {
        if let Some(panel) = self.settings.as_ref() {
            panel.focus();
            return;
        }
        // 皮肤列表每次都重扫：用户刚往 `skins/` 里丢了新形象，打开面板就该看得见。
        let skins = zouwu_platform::assets::skin_choices(&self.i18n);
        match SettingsWindow::new(event_loop, self.config.clone(), self.themes.clone(), skins) {
            Ok(panel) => {
                self.settings = Some(panel);
                // 小兽是置顶窗口，会飘在设置面板上面。面板打开期间暂时放开置顶，
                // 让小兽留在面板下方——但仍然看得见，改主题/皮肤能立刻看到效果。
                if let Some(hwnd) = self.hwnd() {
                    zouwu_platform::layer::set_always_on_top(hwnd, false);
                }
            }
            Err(err) => log::error!("cannot open settings panel: {err}"),
        }
    }

    /// 面板窗口的事件分发。返回的 [`SettingsOutcome`] 在这里落地。
    fn settings_event(&mut self, _event_loop: &ActiveEventLoop, event: WindowEvent) {
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
            self.settings = None;
            // 恢复配置里的置顶状态（面板期间被临时放开了）。
            if let Some(hwnd) = self.hwnd() {
                zouwu_platform::layer::set_always_on_top(hwnd, self.config.window.always_on_top);
            }
            log::info!("settings panel closed");
        }
    }

    /// 托盘图标 + 全局热键。只做一次，且必须**在主线程**上做（见 platform::tray）。
    fn init_shell(&mut self) {
        if self.tray.is_none() {
            let (rgba, w, h) = crate::paint::tray_icon(&self.theme, self.skin_for_paint(), 32);
            let autostart_on = autostart::is_enabled();
            // 主题/皮肤列表在启动时扫一次：换外观不是高频操作，
            // 为它挂目录监听不值得（新装的主题重启后出现）。
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

    /// 按配置注册热键；解析失败时回退到默认值并记日志。
    fn apply_hotkey(&self, registrar: &mut HotkeyRegistrar) -> Result<(), String> {
        apply_hotkey_setting(&self.config.general.hotkey, registrar)
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
            if let Some(hwnd) = self.hwnd() {
                zouwu_platform::layer::set_visible(hwnd, true);
                if self.config.general.bring_to_front {
                    zouwu_platform::layer::bring_to_front(hwnd);
                }
            }
            log::info!("zouwu shown");
            if let Some(w) = self.window_arc() {
                w.request_redraw();
            }
        } else {
            // 先收起再躲起来，下次呼出时是"干净"的收起态。
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
            TrayCommand::OpenLogs => {
                if !zouwu_platform::logging::open_log_dir() {
                    log::warn!("log dir: {}", zouwu_platform::logging::log_dir().display());
                }
            }
            TrayCommand::SetTheme(id) => self.set_theme(id, now),
            TrayCommand::SetSkin(id) => self.set_skin(id, now),
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

        if self
            .hotkeys
            .as_ref()
            .map(|h| h.poll_pressed())
            .unwrap_or(false)
        {
            self.toggle_visible(now);
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
        log::debug!(
            "ring {} (hover delay {} ms)",
            if expanded { "expanded" } else { "collapsed" },
            self.config.interaction.hover_delay_ms
        );
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
        // 设置面板打开时，小兽被压在面板下面，点不到它 —— 这时候还按 16ms
        // 轮询就只是白白吃掉 1~4% CPU（面板开着的实测值）。降到后台频率。
        let inside = self.settings.is_none() && self.poll_pointer(now);
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

        if !inside {
            if self.cursor.is_some() {
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
                if let Some(idx) = self.hover_test(pos) {
                    if let Some(item) = self.items.get(idx) {
                        // M0：先验证交互链路，真正的能力在 M3 接入
                        log::info!("bubble activated: {} ({})", item.id, item.label_key);
                    }
                } else if on_buddy {
                    // 按住本体先别急着展开：这次按住可能是想**搬动**它。
                    // 松手时若没越过拖动阈值，才当作一次点击（见 `toggle_from_click`）。
                    self.begin_drag();
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
            // 呼吸拆成两半：**幅度**归皮肤（`body_radius` 里乘），相位归这里。
            buddy_scale: self.buddy_scale.value,
            buddy_alpha: 1.0,
            blink: self.blink_amount(now),
            breath: self.breath_amount(now),
            theme: &self.theme,
            bubbles: &paints,
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

/// 注册 `raw`（形如 `"Alt+Space"`）描述的全局热键。
///
/// 单独抽出来是因为它同时被“启动”和“热重载”两条路径使用。
/// 配置里的热键写错时**不能**让用户失去唤醒手段，所以回退到默认热键；
/// 空串是合法的“我不要热键”，不注册也不抱怨。
fn apply_hotkey_setting(raw: &str, registrar: &mut HotkeyRegistrar) -> Result<(), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        registrar.clear();
        log::info!("hotkey disabled by config");
        return Ok(());
    }
    let spec = match Hotkey::parse(raw) {
        Ok(spec) => spec,
        Err(err) => {
            log::warn!("invalid hotkey `{raw}` ({err}); falling back to default");
            Hotkey::default_summon()
        }
    };
    registrar.set(&spec)
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

        // 窗口存在之后才能拿 HWND：托盘、隐藏态、置前都依赖它。
        if !self.visible {
            if let Some(hwnd) = self.hwnd() {
                zouwu_platform::layer::set_visible(hwnd, false);
            }
        }
        self.init_shell();

        if self.visible {
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
        log::info!("zouwu exiting");
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // 面板是另一个窗口：按 id 分流，别把它的鼠标事件当成分层窗口的。
        if self.settings.as_ref().is_some_and(|p| p.id() == id) {
            self.settings_event(event_loop, event);
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
                self.on_cursor_left();
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
                        // 小工具的心智：Esc 是"收起来"，不是"杀掉我"。
                        // 真的退出走托盘菜单（上面写着"退出"）。
                        self.set_visible(false, now);
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
        let now = Instant::now();

        // 轮询只是一种"叫醒输入"，它本身不应该产生任何重绘：
        // 开一枪 GetCursorPos 几微秒，醒来发现画面没变就直接接着睡。
        if now >= self.next_poll {
            self.poll_now(now);
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
        if !self.settings_env_done && want_panel {
            self.settings_env_done = true;
            self.pending_settings = true;
        }
        if std::mem::take(&mut self.pending_settings) {
            self.open_settings(event_loop, now);
        }

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
