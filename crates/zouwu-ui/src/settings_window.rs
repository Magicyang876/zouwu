//! 设置面板的窗口封装：winit 窗口 + softbuffer 呈现 + 输入分发。
//!
//! 为什么是**第二个真窗口**而不是在主窗口上"画一层"：
//! 主窗口是 `WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE` 的分层挂件，
//! 它按 alpha 做命中测试、拿不到键盘焦点（ADR-009 里已经吃过这个亏）。
//! 设置面板要输入框、要 Tab 键、要标题栏，天生属于一个正常窗口。
//!
//! 这个文件只做"窗口与输入"，界面长什么样全在 [`crate::settings`] 里，
//! 所以面板可以离屏渲染成 PNG 做验收（见 `examples/settings_shot.rs`）。

use std::sync::Arc;

use tiny_skia::Pixmap;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{CursorIcon, Window, WindowId};
use zouwu_core::config::{ChangeKind, Config};
use zouwu_core::geom::Point;
use zouwu_core::hotkey::{Hotkey, Key as HotkeyKey};
use zouwu_core::theme::Theme;

use crate::presenter::{create_soft, Presenter};
use crate::settings::{self, Hit, Model, Paths};

/// 面板对宿主（[`crate::app`]）说的事。
pub enum Outcome {
    /// 关掉面板（小兽继续跑）。
    Close,
    /// 配置有变，需要宿主落地到运行期对象与磁盘。
    Changed(Vec<ChangeKind>),
}

pub struct SettingsWindow {
    window: Arc<Window>,
    presenter: Box<dyn Presenter>,
    pixmap: Option<Pixmap>,
    pub model: Model,
    /// 最近一次布局：输入处理直接复用，保证"看到的"与"点到的"一致。
    layout: Option<settings::Layout>,
    hover: Option<usize>,
    cursor: Point,
    modifiers: ModifiersState,
    /// 已提交的帧数（诊断"面板是不是在空转重绘"用）。
    frames: u64,
    /// 第一帧已经补刷过暗色标题栏。
    frame_dark: bool,
    /// 输入法是否已开（避免每次都打一次 `set_ime_allowed`）。
    ime_on: bool,
    /// 输入框里正按着左键拖选文本。
    ///
    /// 鼠标移出框外也要继续选（和系统输入框一样），所以不能只看“落在行里”。
    drag_text: bool,
    /// 正在拖输入框自己的水平滚动条。
    drag_scroll: bool,
    /// 上一次点在哪个输入框的什么位置（双击/三击判定用）。
    last_click: Option<(std::time::Instant, f32, f32, settings::EditTarget)>,
    /// 连击计数：1 单击、2 双击选词、3 三击全选。
    click_count: u8,
}

impl SettingsWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        config: Config,
        themes: Vec<Theme>,
        skins: Vec<(String, String)>,
    ) -> Result<Self, String> {
        let (w, h) = settings::DEFAULT_SIZE;
        let attrs = Window::default_attributes()
            .with_title("zouwu")
            .with_inner_size(LogicalSize::new(w, h))
            .with_min_inner_size(LogicalSize::new(settings::MIN_SIZE.0, settings::MIN_SIZE.1))
            .with_resizable(true)
            .with_visible(true);

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("create settings window: {e}"))?,
        );

        let physical = window.inner_size();
        let presenter = create_soft(window.clone(), physical.width, physical.height)?;

        let paths = Paths {
            config: zouwu_platform::config_path().display().to_string(),
            log: zouwu_platform::logging::log_path().display().to_string(),
        };
        let mut model = Model::new(config, themes, skins, paths);
        // 字体包列表从磁盘扫（`Model::new` 不碰磁盘，测试才好写）。
        model.refresh_fonts();
        window.set_title(&model.i18n.t("settings.title"));
        // 面板是暗色的，白标题栏很扎眼：让 DWM 一起变暗（失败也无所谓）。
        crate::frame::apply(&window, &model.theme());
        crate::frame::life("settings", "created", &window);
        window.focus_window();

        let mut this = Self {
            window,
            presenter,
            pixmap: None,
            model,
            layout: None,
            hover: None,
            cursor: Point::new(0.0, 0.0),
            modifiers: ModifiersState::empty(),
            frames: 0,
            frame_dark: false,
            ime_on: false,
            drag_text: false,
            drag_scroll: false,
            last_click: None,
            click_count: 0,
        };
        this.rebuild_layout();
        this.request_redraw();
        log::info!("settings panel opened");
        Ok(this)
    }

    /// 重新展示（窗口是复用的，不是重建的）。
    ///
    /// 面板关掉后配置可能被别处改过（热键、皮肤……），所以重建整个 `Model`
    /// 而不是指望熟面板自己知道。参数和 [`Self::new`] 一致。
    ///
    /// **不要改成“关掉就销毁”**：析构一个当时正持有焦点的窗口会让 USER32
    /// 同步去通知别的进程的窗口，那边慢一点整个进程就冻住了（ADR-035）。
    pub fn reload(&mut self, config: Config, themes: Vec<Theme>, skins: Vec<(String, String)>) {
        let paths = Paths {
            config: zouwu_platform::config_path().display().to_string(),
            log: zouwu_platform::logging::log_path().display().to_string(),
        };
        let mut model = Model::new(config, themes, skins, paths);
        model.refresh_fonts();
        self.window.set_title(&model.i18n.t("settings.title"));
        self.model = model;
        self.layout = None;
        self.hover = None;
        crate::frame::apply(&self.window, &self.model.theme());
        self.window.set_visible(true);
        self.window.focus_window();
        self.rebuild_layout();
        self.request_redraw();
        log::info!("settings panel reopened");
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible().unwrap_or(true)
    }

    pub fn focus(&self) {
        self.window.focus_window();
        self.request_redraw();
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// 开/关输入法。
    ///
    /// 平时一律关着：面板上没有编辑框时，输入法弹出候选窗只会遮掉设置项。
    /// 只有真的在填名字/命令时才打开，否则中文名字根本敲不进去。
    fn set_ime(&self, on: bool) {
        if self.ime_on == on {
            return;
        }
        self.window.set_ime_allowed(on);
        let pos = self.window.inner_size();
        let _ = pos;
    }

    /// 面板也需要"Esc 关掉"之外的关闭入口：宿主（托盘再次点"设置"）用它。
    pub fn close(&mut self) {
        crate::frame::life("settings", "hidden", &self.window);
        // 关面板时揃定顺手收掉输入法和未提交的编辑：
        // 留着输入法开着，下次开面板会有候选窗无端弹出来。
        settings::commit_edit(&mut self.model);
        self.set_ime(false);
        self.window.set_visible(false);
    }

    /// 当前逻辑尺寸。
    fn logical_size(&self) -> (f32, f32) {
        let size = self.window.inner_size();
        let scale = self.window.scale_factor() as f32;
        let scale = if scale > 0.01 { scale } else { 1.0 };
        (size.width as f32 / scale, size.height as f32 / scale)
    }

    fn rebuild_layout(&mut self) {
        let (w, h) = self.logical_size();
        let layout = settings::layout(&self.model, w, h);
        self.model.scroll = self.model.scroll.clamp(0.0, layout.max_scroll());
        self.layout = Some(layout);
        // 布局一变，输入框宽度/DPI 都可能变（换标签页、拖窗口、换显示器）。
        self.sync_edit_viewport();
    }

    /// 把“当前的 DPI 与输入框宽度”告诉模型（水平滚动与光标可见全靠它）。
    fn sync_edit_viewport(&mut self) {
        let Some(inner) = self.text_inner_of_editing() else {
            return;
        };
        let scale = self.window.scale_factor() as f32;
        let scale = if scale > 0.01 { scale } else { 1.0 };
        settings::set_edit_viewport(&mut self.model, inner, scale);
    }

    /// 画一帧并提交。返回是否成功（失败只记日志，面板不该因一次绘制失败而消失）。
    pub fn redraw(&mut self) {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let need_new = match &self.pixmap {
            Some(p) => p.width() != size.width || p.height() != size.height,
            None => true,
        };
        if need_new {
            self.pixmap = Pixmap::new(size.width, size.height);
            self.presenter.resize(size.width.max(1), size.height.max(1));
        }
        // 形象页的缩略图是按需算的：只有主题或皮肤列表变了才重画。
        if self.model.tab == settings::Tab::Skins {
            self.model.ensure_thumbs();
        }
        self.rebuild_layout();
        let Some(layout) = &self.layout else {
            return;
        };
        let Some(pixmap) = self.pixmap.as_mut() else {
            log::error!("settings: pixmap alloc failed");
            return;
        };
        settings::draw(pixmap, layout, &self.model);
        if let Err(err) = self.presenter.present(pixmap) {
            log::error!("settings: present failed: {err}");
            return;
        }
        self.frames += 1;
        if self.frames <= 2 || self.frames % 120 == 0 {
            log::debug!("settings: present #{}", self.frames);
        }
    }

    pub fn window_event(&mut self, event: &WindowEvent) -> Vec<Outcome> {
        match event {
            WindowEvent::RedrawRequested => {
                // 窗口第一次上屏时 DWM 会先按浅色画一帧边框，创建时设的属性可能被
                // 这次绘制覆盖，所以第一帧再补一次。
                if !self.frame_dark {
                    self.frame_dark = true;
                    crate::frame::apply(&self.window, &self.model.theme());
                }
                self.redraw();
                Vec::new()
            }
            WindowEvent::Resized(size) => {
                crate::frame::apply(&self.window, &self.model.theme());
                self.presenter.resize(size.width.max(1), size.height.max(1));
                self.request_redraw();
                Vec::new()
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.request_redraw();
                Vec::new()
            }
            WindowEvent::CloseRequested => vec![Outcome::Close],
            WindowEvent::Focused(true) => {
                crate::frame::apply(&self.window, &self.model.theme());
                Vec::new()
            }
            WindowEvent::Focused(false) => {
                // 切走时放弃热键捕获，否则回来会以为还在等键。
                if self.model.capturing.is_some() {
                    self.model.capturing = None;
                    self.request_redraw();
                }
                // 编辑框则**写回**再失焦：用户点了别的窗口不等于放弃刚打的字。
                if let Some(kinds) = settings::commit_edit(&mut self.model) {
                    self.set_ime(false);
                    return self.after_change(kinds);
                }
                Vec::new()
            }
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => {
                // 中文/日文输入法上屏：只有编辑框在用时才收。
                if self.model.editing.is_some() {
                    settings::edit_text(&mut self.model, &text);
                    self.request_redraw();
                }
                Vec::new()
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                Vec::new()
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.window.scale_factor() as f32;
                let scale = if scale > 0.01 { scale } else { 1.0 };
                self.cursor = Point::new(position.x as f32 / scale, position.y as f32 / scale);
                // 正在拖输入框的水平滚动条：鼠标到哪，可视起点就到哪。
                if self.drag_scroll {
                    if let Some(inner) = self.text_inner_of_editing() {
                        settings::set_offset_from_x(&mut self.model, inner, self.cursor.x);
                        self.request_redraw();
                    }
                    return Vec::new();
                }
                // 正在拖选：鼠标到哪，选区就到哪（不更新 hover，不然路过别的行会闪）。
                if self.drag_text {
                    let inner = self.text_inner_of_editing();
                    if let Some(inner) = inner {
                        settings::set_caret_from_x(
                            &mut self.model,
                            inner,
                            self.cursor.x,
                            true,
                            scale,
                        );
                        self.request_redraw();
                    }
                    return Vec::new();
                }
                let hover = self
                    .layout
                    .as_ref()
                    .and_then(|l| settings::hit(l, self.cursor))
                    .and_then(|hit| match hit {
                        Hit::Row(i) | Hit::Step(i, _) | Hit::Press(i) => Some(i),
                        Hit::RingMove(i, _) | Hit::RingHotkey(i) => Some(i),
                        _ => None,
                    });
                // 缩略图墙的悬停单独记：它记的是"第几套皮肤"，不是"第几行"。
                let hover_dot = self
                    .layout
                    .as_ref()
                    .and_then(|l| settings::hit(l, self.cursor))
                    .and_then(|hit| match hit {
                        Hit::GalleryDot(dot) => Some(dot),
                        _ => None,
                    });
                if hover_dot != self.model.hover_skin {
                    self.model.hover_skin = hover_dot;
                    self.request_redraw();
                }
                let interactive = hover.is_some()
                    || hover_dot.is_some()
                    || self
                        .layout
                        .as_ref()
                        .and_then(|l| settings::hit(l, self.cursor))
                        .is_some_and(|h| matches!(h, Hit::Tab(_) | Hit::Scrollbar));
                self.window.set_cursor(if interactive {
                    CursorIcon::Pointer
                } else {
                    CursorIcon::Default
                });
                if hover != self.hover {
                    self.hover = hover;
                    self.model.hover = hover;
                    self.request_redraw();
                }
                Vec::new()
            }
            WindowEvent::CursorLeft { .. } => {
                self.hover = None;
                if self.model.hover_skin.take().is_some() {
                    self.request_redraw();
                }
                self.model.hover = None;
                self.request_redraw();
                Vec::new()
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 40.0,
                };
                // 鼠标悬在“正在编辑且放不下”的输入框上：横着滚（那里竖直翻页没意义）。
                if self.text_box_under_cursor().is_some() {
                    settings::scroll_edit(&mut self.model, -lines * 48.0);
                    self.request_redraw();
                    return Vec::new();
                }
                self.scroll_by(-lines * 56.0);
                Vec::new()
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                // 松手就不拖了（选区/滚动位置会一直留着，直到下一次点击或按键）。
                self.drag_text = false;
                self.drag_scroll = false;
                Vec::new()
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.click(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed && !event.repeat {
                    self.key(event)
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    fn scroll_by(&mut self, delta: f32) {
        let Some(layout) = &self.layout else {
            return;
        };
        let max = layout.max_scroll();
        let next = (self.model.scroll + delta).clamp(0.0, max);
        if (next - self.model.scroll).abs() > 0.01 {
            self.model.scroll = next;
            self.request_redraw();
        }
    }

    /// 正在编辑的那个输入框的内框（模型里存的只是“哪个字段”，矩形要去布局里找）。
    fn text_inner_of_editing(&self) -> Option<crate::widget::Rect> {
        let target = self.model.editing.as_ref()?.target;
        let layout = self.layout.as_ref()?;
        layout.rows.iter().find_map(|(row, rl)| match &row.control {
            settings::Control::Text { target: t, .. } if *t == target => {
                Some(settings::text_inner(rl.hit))
            }
            _ => None,
        })
    }

    /// 光标是否压在“正在编辑的那个输入框”的水平滚动条 thumb 上（是则返回内框）。
    fn text_scrollbar_hit(&self) -> Option<crate::widget::Rect> {
        let editing = self.model.editing.as_ref()?;
        let target = editing.target;
        let layout = self.layout.as_ref()?;
        let inner = layout
            .rows
            .iter()
            .find_map(|(row, rl)| match &row.control {
                settings::Control::Text { target: t, .. } if *t == target => {
                    Some(settings::text_inner(rl.hit))
                }
                _ => None,
            })?;
        settings::text_scrollbar_at(editing, inner, self.cursor).then_some(inner)
    }

    /// 鼠标是不是悬在“正在编辑且内容放不下”的输入框上（滚轮横滚用）。
    fn text_box_under_cursor(&self) -> Option<crate::widget::Rect> {
        let editing = self.model.editing.as_ref()?;
        if !settings::text_overflow(editing) {
            return None;
        }
        let target = editing.target;
        let layout = self.layout.as_ref()?;
        layout.rows.iter().find_map(|(row, rl)| match &row.control {
            settings::Control::Text { target: t, .. } if *t == target => rl
                .rect
                .contains(self.cursor)
                .then(|| settings::text_inner(rl.hit)),
            _ => None,
        })
    }

    /// 连击计数：同一个输入框、400ms 内、位移不超过 4px 才算“又点了一下”。
    fn next_click_count(&mut self, target: settings::EditTarget) -> u8 {
        let now = std::time::Instant::now();
        let (x, y) = (self.cursor.x, self.cursor.y);
        let again = self.last_click.is_some_and(|(at, px, py, prev)| {
            prev == target
                && now.duration_since(at) < std::time::Duration::from_millis(400)
                && (px - x).abs() <= 4.0
                && (py - y).abs() <= 4.0
        });
        self.click_count = if again {
            (self.click_count + 1).min(3)
        } else {
            1
        };
        self.last_click = Some((now, x, y, target));
        self.click_count
    }

    /// 点在输入框上：把光标落到鼠标位置，并准备好拖选。
    ///
    /// “点同一个输入框”不能先 `commit_edit`：那会把编辑状态收掉、光标回到文末，
    /// 于是“点中间改一个字”变成了“点了就把光标跳到最后”。
    /// 从名字框直接点到命令框则是**提交名字 + 接着编辑命令**，不用点两次。
    fn click_into_text(&mut self, row: usize, target: settings::EditTarget) -> Vec<Outcome> {
        let Some(inner) = self
            .layout
            .as_ref()
            .and_then(|l| l.rows.get(row))
            .map(|(_, rl)| settings::text_inner(rl.hit))
        else {
            return Vec::new();
        };
        let scale = self.window.scale_factor() as f32;
        let scale = if scale > 0.01 { scale } else { 1.0 };
        let same = self
            .model
            .editing
            .as_ref()
            .is_some_and(|e| e.target == target);
        let mut outcome = Vec::new();
        if !same {
            // 先把上一个框的字写回（用户点过来 = 上一个框结束了）。
            if let Some(kinds) = settings::commit_edit(&mut self.model) {
                self.set_ime(false);
                outcome = self.after_change(kinds);
            }
            settings::begin_edit(&mut self.model, target);
        }
        // 连击：1 单击定位光标、2 双击选词、3 三击全选（与系统输入框一致）。
        let count = self.next_click_count(target);
        match count {
            1 => settings::set_caret_from_x(&mut self.model, inner, self.cursor.x, false, scale),
            2 => settings::select_word(&mut self.model, inner, self.cursor.x, scale),
            _ => settings::select_all(&mut self.model),
        }
        self.model.focus = Some(row);
        // 双击/三击之后不跟着拖选：那两次按下的微小位移是手抖，不是拖。
        self.drag_text = count == 1;
        self.set_ime(true);
        self.request_redraw();
        outcome
    }

    fn click(&mut self) -> Vec<Outcome> {
        // 每次新的按下都算“重新开始”：上一次若在窗口外松手，收不到 Released，
        // 残留的 drag_text 会让接下来的鼠标移动莫名续选。
        self.drag_text = false;
        self.drag_scroll = false;
        // 输入框自己的水平滚动条：按住 thumb 拖。
        // 这一步必须先于“点进输入框”，否则压在滚动条上只会把光标挪到那一行末尾。
        if let Some(inner) = self.text_scrollbar_hit() {
            self.drag_scroll = true;
            settings::set_offset_from_x(&mut self.model, inner, self.cursor.x);
            self.request_redraw();
            return Vec::new();
        }
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        let p = self.cursor;
        let hit = settings::hit(layout, p);
        // 先处理“点进输入框”：它不能走下面那个“先提交再处理”的岔。
        if let Some(Hit::Press(i)) = hit {
            if let Some((row, _)) = layout.rows.get(i) {
                if let settings::Control::Text { target, .. } = &row.control {
                    return self.click_into_text(i, *target);
                }
            }
        }
        // 点别处 = 这次输入结束。
        if let Some(kinds) = settings::commit_edit(&mut self.model) {
            self.set_ime(false);
            return self.after_change(kinds);
        }
        let Some(hit) = hit else {
            self.model.focus = None;
            self.request_redraw();
            return Vec::new();
        };
        self.model.keyboard_used = false;
        match hit {
            Hit::Tab(tab) => {
                self.model.set_tab(tab);
                self.request_redraw();
                Vec::new()
            }
            Hit::Scrollbar => {
                // 点轨道 = 翻一屏（不实现拖拽：面板内容通常只有一屏半）。
                let page = layout.content.h * 0.9;
                let before = self.model.scroll;
                self.scroll_by(page);
                if (self.model.scroll - before).abs() < 0.01 {
                    self.model.scroll = 0.0;
                }
                Vec::new()
            }
            Hit::Step(i, dir) => {
                self.model.focus = Some(i);
                let Some(field) = layout.rows[i].0.field else {
                    return Vec::new();
                };
                let kinds = settings::step(&mut self.model, field, dir);
                self.after_change(kinds)
            }
            Hit::Press(i) => {
                self.model.focus = Some(i);
                let kinds = settings::activate(&mut self.model, layout, i);
                self.set_ime(self.model.editing.is_some());
                self.after_change(kinds)
            }
            Hit::RingMove(i, dir) => {
                self.model.focus = Some(i);
                let kinds = settings::ring_move(&mut self.model, i, dir);
                self.after_change(kinds)
            }
            Hit::RingHotkey(i) => {
                self.model.focus = Some(i);
                settings::begin_capture(&mut self.model, settings::CaptureTarget::Item(i));
                self.request_redraw();
                Vec::new()
            }
            Hit::GalleryDot(dot) => {
                let kinds = settings::select_skin(&mut self.model, dot);
                self.after_change(kinds)
            }
            Hit::Row(i) => {
                self.model.focus = Some(i);
                // 点开关行的任意位置 = 切换（开关太小，不该要求精准命中）。
                let is_toggle =
                    matches!(layout.rows[i].0.control, settings::Control::Toggle { .. });
                if is_toggle {
                    let kinds = settings::activate(&mut self.model, layout, i);
                    self.after_change(kinds)
                } else {
                    self.request_redraw();
                    Vec::new()
                }
            }
        }
    }

    fn key(&mut self, event: &winit::event::KeyEvent) -> Vec<Outcome> {
        // ── 热键捕获模式：所有按键都归它 ──
        if self.model.capturing.is_some() {
            match event.logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.model.capturing = None;
                    self.request_redraw();
                    return Vec::new();
                }
                Key::Named(NamedKey::Backspace) => {
                    // 清除热键 = 关掉这个开关（而不是留下一个"按了就报错"的串）。
                    let outcome = settings::clear_hotkey(&mut self.model).unwrap_or_default();
                    self.request_redraw();
                    return self.after_change(outcome);
                }
                _ => {}
            }
            let Some(key) = hotkey_key(event) else {
                return Vec::new();
            };
            let mods = self.modifiers;
            if mods.is_empty() {
                self.model.notice = Some(self.model.i18n.t("settings.hotkey.need-modifier"));
                self.request_redraw();
                return Vec::new();
            }
            let hotkey = Hotkey {
                ctrl: mods.control_key(),
                shift: mods.shift_key(),
                alt: mods.alt_key(),
                meta: mods.super_key(),
                key,
            };
            let outcome =
                settings::capture_hotkey(&mut self.model, Some(hotkey)).unwrap_or_default();
            if let Some(notice) = &self.model.notice {
                log::warn!("hotkey capture rejected: {notice}");
            }
            return self.after_change(outcome);
        }

        // ── 文本编辑模式：只认编辑用的那几个键 ──
        if self.model.editing.is_some() {
            use settings::{EditKey, EditMotion};
            // 方向键 / Home / End：Shift 拉选区，Ctrl+A 全选，其余字符自己拼。
            let motion = match event.logical_key {
                Key::Named(NamedKey::ArrowLeft) => Some(EditMotion::Left),
                Key::Named(NamedKey::ArrowRight) => Some(EditMotion::Right),
                Key::Named(NamedKey::Home) => Some(EditMotion::Home),
                Key::Named(NamedKey::End) => Some(EditMotion::End),
                _ => None,
            };
            if let Some(motion) = motion {
                settings::edit_move(&mut self.model, motion, self.modifiers.shift_key());
                self.request_redraw();
                return Vec::new();
            }
            if self.modifiers.control_key() {
                if let Key::Character(ch) = &event.logical_key {
                    match ch.to_lowercase().as_str() {
                        "a" => {
                            settings::select_all(&mut self.model);
                            self.request_redraw();
                            return Vec::new();
                        }
                        "c" => {
                            // 没选区就整段拷（单行框“全选再拷”太笨）。
                            settings::copy_selection(&self.model);
                            return Vec::new();
                        }
                        "x" => {
                            // 没选区就什么都不干：整段剪切 = 清空，太危险。
                            if settings::cut_selection(&mut self.model) {
                                self.request_redraw();
                            }
                            return Vec::new();
                        }
                        "v" => {
                            settings::paste_clipboard(&mut self.model);
                            self.request_redraw();
                            return Vec::new();
                        }
                        _ => {}
                    }
                }
            }
            let edit_key = match event.logical_key {
                Key::Named(NamedKey::Escape) => Some(EditKey::Cancel),
                Key::Named(NamedKey::Enter) => Some(EditKey::Commit),
                Key::Named(NamedKey::Backspace) => Some(EditKey::Backspace),
                Key::Named(NamedKey::Delete) => Some(EditKey::Delete),
                _ => None,
            };
            if let Some(edit_key) = edit_key {
                let outcome = settings::edit_key(&mut self.model, edit_key);
                self.set_ime(self.model.editing.is_some());
                self.request_redraw();
                return match outcome {
                    Some(kinds) => self.after_change(kinds),
                    None => Vec::new(),
                };
            }
            // 普通字符：走 `text`（已按当前键盘布局/修饰键折算好）。
            // Ctrl/Alt 组合（含 Ctrl+C/X/V）已在上面处理完，这里不重复插字符。
            if let Some(text) = event.text.as_deref() {
                if !text.is_empty() && !self.modifiers.control_key() && !self.modifiers.alt_key() {
                    settings::edit_text(&mut self.model, text);
                    self.request_redraw();
                }
            }
            return Vec::new();
        }

        match event.logical_key {
            Key::Named(NamedKey::Escape) => vec![Outcome::Close],
            Key::Named(NamedKey::Tab) => {
                // Ctrl+Tab 切标签页：键盘用户不该为了换页去摸鼠标。
                if self.modifiers.control_key() {
                    self.cycle_tab(!self.modifiers.shift_key());
                } else {
                    self.move_focus(!self.modifiers.shift_key());
                }
                Vec::new()
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.move_focus(true);
                Vec::new()
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.move_focus(false);
                Vec::new()
            }
            Key::Named(NamedKey::ArrowLeft) => self.adjust_focus(-1),
            Key::Named(NamedKey::ArrowRight) => self.adjust_focus(1),
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => self.press_focus(),
            _ => Vec::new(),
        }
    }

    fn cycle_tab(&mut self, forward: bool) {
        let tabs = crate::settings::Tab::ALL;
        let index = tabs.iter().position(|t| *t == self.model.tab).unwrap_or(0);
        let len = tabs.len();
        let next = if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        };
        let tab = tabs[next];
        self.model.set_tab(tab);
        self.request_redraw();
    }

    fn move_focus(&mut self, forward: bool) {
        let Some(layout) = &self.layout else {
            return;
        };
        let Some(next) = layout.move_focus(forward) else {
            return;
        };
        self.model.focus = Some(next);
        self.model.keyboard_used = true;
        self.ensure_focus_visible();
        self.request_redraw();
    }

    fn ensure_focus_visible(&mut self) {
        let (Some(layout), Some(focus)) = (&self.layout, self.model.focus) else {
            return;
        };
        let Some((_, rl)) = layout.rows.get(focus) else {
            return;
        };
        let (top, bottom) = (layout.content.y, layout.content.bottom());
        if rl.rect.y < top {
            self.model.scroll -= top - rl.rect.y;
        } else if rl.rect.bottom() > bottom {
            self.model.scroll += rl.rect.bottom() - bottom;
        }
        self.model.scroll = self.model.scroll.max(0.0);
    }

    fn adjust_focus(&mut self, dir: i32) -> Vec<Outcome> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        let Some(focus) = self.model.focus else {
            return Vec::new();
        };
        let Some(field) = layout.rows.get(focus).and_then(|(row, _)| row.field) else {
            return Vec::new();
        };
        let kinds = settings::step(&mut self.model, field, dir);
        self.after_change(kinds)
    }

    fn press_focus(&mut self) -> Vec<Outcome> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        let Some(focus) = self.model.focus else {
            return Vec::new();
        };
        let kinds = settings::activate(&mut self.model, layout, focus);
        self.after_change(kinds)
    }

    fn after_change(&mut self, kinds: Vec<ChangeKind>) -> Vec<Outcome> {
        self.request_redraw();
        if kinds.is_empty() {
            Vec::new()
        } else {
            vec![Outcome::Changed(kinds)]
        }
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        crate::frame::life("settings", "destroyed", &self.window);
    }
}

/// winit 的物理键 → 领域层的热键主键。
fn hotkey_key(event: &winit::event::KeyEvent) -> Option<HotkeyKey> {
    let PhysicalKey::Code(code) = event.physical_key else {
        return None;
    };
    let key = match code {
        KeyCode::Space => HotkeyKey::Space,
        KeyCode::Enter | KeyCode::NumpadEnter => HotkeyKey::Enter,
        KeyCode::Tab => HotkeyKey::Tab,
        KeyCode::Escape => HotkeyKey::Escape,
        KeyCode::Backspace => HotkeyKey::Backspace,
        KeyCode::Delete => HotkeyKey::Delete,
        KeyCode::ArrowUp => HotkeyKey::Up,
        KeyCode::ArrowDown => HotkeyKey::Down,
        KeyCode::ArrowLeft => HotkeyKey::Left,
        KeyCode::ArrowRight => HotkeyKey::Right,
        KeyCode::KeyA => HotkeyKey::Letter('a'),
        KeyCode::KeyB => HotkeyKey::Letter('b'),
        KeyCode::KeyC => HotkeyKey::Letter('c'),
        KeyCode::KeyD => HotkeyKey::Letter('d'),
        KeyCode::KeyE => HotkeyKey::Letter('e'),
        KeyCode::KeyF => HotkeyKey::Letter('f'),
        KeyCode::KeyG => HotkeyKey::Letter('g'),
        KeyCode::KeyH => HotkeyKey::Letter('h'),
        KeyCode::KeyI => HotkeyKey::Letter('i'),
        KeyCode::KeyJ => HotkeyKey::Letter('j'),
        KeyCode::KeyK => HotkeyKey::Letter('k'),
        KeyCode::KeyL => HotkeyKey::Letter('l'),
        KeyCode::KeyM => HotkeyKey::Letter('m'),
        KeyCode::KeyN => HotkeyKey::Letter('n'),
        KeyCode::KeyO => HotkeyKey::Letter('o'),
        KeyCode::KeyP => HotkeyKey::Letter('p'),
        KeyCode::KeyQ => HotkeyKey::Letter('q'),
        KeyCode::KeyR => HotkeyKey::Letter('r'),
        KeyCode::KeyS => HotkeyKey::Letter('s'),
        KeyCode::KeyT => HotkeyKey::Letter('t'),
        KeyCode::KeyU => HotkeyKey::Letter('u'),
        KeyCode::KeyV => HotkeyKey::Letter('v'),
        KeyCode::KeyW => HotkeyKey::Letter('w'),
        KeyCode::KeyX => HotkeyKey::Letter('x'),
        KeyCode::KeyY => HotkeyKey::Letter('y'),
        KeyCode::KeyZ => HotkeyKey::Letter('z'),
        KeyCode::Digit0 => HotkeyKey::Digit('0'),
        KeyCode::Digit1 => HotkeyKey::Digit('1'),
        KeyCode::Digit2 => HotkeyKey::Digit('2'),
        KeyCode::Digit3 => HotkeyKey::Digit('3'),
        KeyCode::Digit4 => HotkeyKey::Digit('4'),
        KeyCode::Digit5 => HotkeyKey::Digit('5'),
        KeyCode::Digit6 => HotkeyKey::Digit('6'),
        KeyCode::Digit7 => HotkeyKey::Digit('7'),
        KeyCode::Digit8 => HotkeyKey::Digit('8'),
        KeyCode::Digit9 => HotkeyKey::Digit('9'),
        KeyCode::F1 => HotkeyKey::Function(1),
        KeyCode::F2 => HotkeyKey::Function(2),
        KeyCode::F3 => HotkeyKey::Function(3),
        KeyCode::F4 => HotkeyKey::Function(4),
        KeyCode::F5 => HotkeyKey::Function(5),
        KeyCode::F6 => HotkeyKey::Function(6),
        KeyCode::F7 => HotkeyKey::Function(7),
        KeyCode::F8 => HotkeyKey::Function(8),
        KeyCode::F9 => HotkeyKey::Function(9),
        KeyCode::F10 => HotkeyKey::Function(10),
        KeyCode::F11 => HotkeyKey::Function(11),
        KeyCode::F12 => HotkeyKey::Function(12),
        _ => return None,
    };
    Some(key)
}
