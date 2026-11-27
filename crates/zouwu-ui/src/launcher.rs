//! 启动器窗：一个搜索框 + 一排程序，敲字筛、回车开。
//!
//! 与剪贴板历史窗（`history.rs`）是一套骨架的两个用途，共用的部分（搜索框状态、
//! 布局、放大镜/×、行命中、滚动换算）全从那边拿，不复制：
//! 复制之后两边的"点哪落哪"一定会慢慢对不上。
//!
//! 一次只开一个窗，关掉只是隐藏（复用窗口），不销毁——理由见 `HistoryWindow`
//! 的注释：在事件回调里析构 `Window` 会让 USER32 同步通知别的窗口，可能整进程冻住。
//!
//! | 键 | 动作 |
//! |---|---|
//! | 直接打字 | 搜索（空格分隔多个关键词，全部命中才算匹配） |
//! | ↑ / ↓ / PageUp / PageDown / Home / End | 移动选中 |
//! | Enter | 打开选中项并关窗 |
//! | Esc | 先清空搜索；搜索为空时才关闭 |

use std::sync::Arc;
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use zouwu_core::geom::Point;
use zouwu_core::launcher::Program;
use zouwu_core::theme::Theme;

use crate::history::{
    accepts_typing, clear_button, draw_search, layout, palette, row_at, scroll_from_wheel,
    search_edit, search_inner, selected_entry, Layout, Search, SearchEdit, ROW_H,
};
use crate::linefield::{self, LineField};
use crate::presenter::{create_soft, Presenter};
use crate::widget::{Painter, Rect, Weight};

/// 程序路径那一列占的宽度（右侧淡字）。
const SUB_W: f32 = 150.0;

/// 窗口对宿主说的话。
pub enum Outcome {
    /// 打开这个程序（宿主负责 spawn，失败时它才知道该弹哪个窗）。
    Launch(Program),
    Close,
}

pub struct LauncherWindow {
    window: Arc<Window>,
    presenter: Box<dyn Presenter>,
    pixmap: Option<Pixmap>,
    theme: Theme,
    programs: Vec<Program>,
    /// 选中项在**筛后列表**里的下标（不是 `programs` 下标，也不是屏幕行）。
    selected: usize,
    scroll: usize,
    hover: Option<usize>,
    search: Search,
    search_focus: bool,
    drag_search: bool,
    last_click: Option<(Instant, f32, f32)>,
    click_count: u8,
    /// 开窗时刻：呼出快捷键那一下的字符会漏进来（见 `accepts_typing`）。
    opened_at: Instant,
    cursor: Point,
    modifiers: ModifiersState,
    layout: Layout,
    frame_dark: bool,
    placeholder: String,
    no_match_text: String,
    empty_text: String,
    footer_help: String,
}

impl LauncherWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_loop: &ActiveEventLoop,
        programs: Vec<Program>,
        theme: Theme,
        title: String,
        placeholder: String,
        footer: String,
        empty: String,
        no_match: String,
    ) -> Result<Self, String> {
        let layout = layout(programs.len());
        let attrs = Window::default_attributes()
            .with_title(&title)
            .with_window_icon(crate::icon::app_icon())
            .with_inner_size(LogicalSize::new(layout.size.0, layout.size.1))
            .with_resizable(false)
            .with_visible(false);
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("create launcher window: {e}"))?,
        );
        let physical = crate::frame::settle(
            &window,
            LogicalSize::new(layout.size.0, layout.size.1),
            None,
        );
        let presenter = create_soft(window.clone(), physical.width, physical.height)?;
        crate::frame::apply(&window, &theme);
        crate::frame::life("launcher", "created", &window);
        // 程序名里常有中文：开会话 IME，候选窗贴到搜索框（见 `update_ime_area`）。
        window.set_ime_allowed(true);
        let mut this = Self {
            window,
            presenter,
            pixmap: None,
            theme,
            programs,
            selected: 0,
            scroll: 0,
            hover: None,
            search: Search::default(),
            search_focus: false,
            drag_search: false,
            last_click: None,
            click_count: 0,
            opened_at: Instant::now(),
            cursor: Point::new(0.0, 0.0),
            modifiers: ModifiersState::empty(),
            layout,
            frame_dark: false,
            placeholder,
            no_match_text: no_match,
            empty_text: empty,
            footer_help: footer,
        };
        this.update_ime_area();
        // 先上屏再画：窗口隐藏时 `BitBlt` 的像素会被丢掉（实测开出来是一片空底）。
        crate::frame::show(&this.window);
        this.redraw();
        this.window.request_redraw();
        Ok(this)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible().unwrap_or(true)
    }

    pub fn close(&self) {
        crate::frame::life("launcher", "hidden", &self.window);
        self.window.set_visible(false);
    }

    /// 重新上图（换肤后子窗口也得跟着变）。
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        crate::frame::apply(&self.window, &self.theme);
        self.window.request_redraw();
    }

    /// 重新展示（窗口复用，不是重建）。清单每次都重扫：用户可能刚装了个程序。
    pub fn show(&mut self, programs: Vec<Program>) {
        self.opened_at = Instant::now();
        self.search.set("");
        self.programs = programs;
        self.selected = 0;
        self.scroll = 0;
        self.hover = None;
        self.sync_search_viewport();
        self.window.set_visible(true);
        self.window.focus_window();
        self.window.request_redraw();
    }

    /// 设置搜索词（测试/调试入口：`ZOUWU_LAUNCHER_QUERY`）。
    pub fn set_query(&mut self, query: &str) {
        if self.search.text() == query {
            return;
        }
        self.search.set(query);
        self.after_query_change();
    }

    pub fn visible_len(&self) -> usize {
        self.visible().len()
    }

    fn visible(&self) -> Vec<usize> {
        zouwu_core::launcher::search(&self.programs, self.search.text())
    }

    fn clamp_selection(&mut self) {
        let total = self.visible().len();
        if total == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selected.min(total - 1);
        // 行数按窗口实际摆得下的算（开窗时定下的），不是按匹配数。
        let rows = self.visible_rows_cap().max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        self.scroll = self.scroll.min(total.saturating_sub(rows));
    }

    fn move_by(&mut self, delta: isize) {
        let total = self.visible().len();
        if total == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, total as isize - 1) as usize;
        self.clamp_selection();
        self.window.request_redraw();
    }

    fn jump(&mut self, to_end: bool) {
        let total = self.visible().len();
        if total == 0 {
            return;
        }
        self.selected = if to_end { total - 1 } else { 0 };
        self.clamp_selection();
        self.window.request_redraw();
    }

    fn page(&mut self, down: bool) {
        let step = self.visible_rows_cap().max(1) as isize;
        self.move_by(if down { step } else { -step });
    }

    /// 这个窗能摆几行（由开窗时的布局定下）。
    fn visible_rows_cap(&self) -> usize {
        self.layout.rows.len()
    }

    fn after_query_change(&mut self) {
        self.selected = 0;
        self.scroll = 0;
        self.update_ime_area();
        self.window.request_redraw();
    }

    fn sync_search_viewport(&mut self) {
        let scale = self.window.scale_factor() as f32;
        let scale = if scale > 0.01 { scale } else { 1.0 };
        let inner = search_inner(self.layout.search, !self.search.text().is_empty());
        self.search.set_scale(scale);
        self.search.set_width(inner.w.max(1.0));
        linefield::ensure_caret_visible(&mut self.search);
    }

    fn clear_query(&mut self) {
        if self.search.text().is_empty() {
            return;
        }
        self.search.set("");
        self.after_query_change();
    }

    fn update_ime_area(&self) {
        let scale = self.window.scale_factor() as f32;
        let box_ = self.layout.search;
        self.window.set_ime_cursor_area(
            winit::dpi::PhysicalPosition::new(
                ((box_.x + 26.0) * scale) as i32,
                ((box_.y + box_.h) * scale) as i32,
            ),
            winit::dpi::PhysicalSize::new(1, (box_.h * scale) as u32),
        );
    }

    fn next_search_click(&mut self) -> u8 {
        let now = Instant::now();
        let near = self.last_click.is_some_and(|(at, x, y)| {
            now.duration_since(at) < Duration::from_millis(400)
                && (x - self.cursor.x).abs() <= 4.0
                && (y - self.cursor.y).abs() <= 4.0
        });
        self.click_count = if near {
            (self.click_count + 1).min(3)
        } else {
            1
        };
        self.last_click = Some((now, self.cursor.x, self.cursor.y));
        self.click_count
    }

    fn typing_grace(&self) -> bool {
        !accepts_typing(
            self.opened_at.elapsed(),
            self.modifiers.control_key() || self.modifiers.alt_key(),
        )
    }

    /// 搜索框先吃键。返回 true ＝ 这个键已被它吃掉。
    fn search_key(&mut self, event: &KeyEvent) -> bool {
        match search_edit(
            &event.logical_key,
            event.text.as_deref(),
            self.modifiers.control_key(),
        ) {
            SearchEdit::Ignore => false,
            SearchEdit::Clear => {
                // 第一次 Esc 只清搜索，第二次才关窗：误触不该直接关掉。
                if self.search.text().is_empty() {
                    false
                } else {
                    self.clear_query();
                    true
                }
            }
            SearchEdit::Backspace => {
                if self.typing_grace() {
                    return true;
                }
                if linefield::backspace(&mut self.search) {
                    self.after_query_change();
                }
                self.search_focus = true;
                true
            }
            SearchEdit::Append(text) => {
                if self.typing_grace() || self.modifiers.control_key() || self.modifiers.alt_key() {
                    return true;
                }
                linefield::insert(&mut self.search, &text);
                self.search_focus = true;
                self.after_query_change();
                true
            }
            SearchEdit::Paste => {
                if self.typing_grace() {
                    return true;
                }
                self.search_focus = true;
                if linefield::paste_clipboard(&mut self.search) {
                    self.after_query_change();
                }
                true
            }
        }
    }

    fn launch_selected(&mut self) -> Vec<Outcome> {
        let visible = self.visible();
        let Some(index) = selected_entry(&visible, self.selected) else {
            return Vec::new();
        };
        match self.programs.get(index) {
            Some(program) => vec![Outcome::Launch(program.clone()), Outcome::Close],
            None => Vec::new(),
        }
    }

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
        // 先把"筛后列表"算好：拿 `pixmap` 的可变借用后再读 `self` 会借不了。
        let visible = self.visible();
        let empty = if !self.programs.is_empty() && !self.search.text().is_empty() {
            self.no_match_text.as_str()
        } else {
            self.empty_text.as_str()
        };
        let Some(pixmap) = self.pixmap.as_mut() else {
            return;
        };
        log::trace!(
            "launcher: pixmap {}x{}, layout {:?}, scale {}",
            pixmap.width(),
            pixmap.height(),
            self.layout.size,
            self.window.scale_factor()
        );
        draw(
            pixmap,
            &self.programs,
            &visible,
            &self.layout,
            &self.theme,
            View {
                selected: self.selected,
                scroll: self.scroll,
                hover: self.hover,
                search: &self.search,
                search_focus: self.search_focus,
                placeholder: &self.placeholder,
                footer: &self.footer_help,
                empty,
            },
        );
        if let Err(err) = self.presenter.present(pixmap) {
            log::error!("launcher: present failed: {err}");
        }
    }

    pub fn window_event(&mut self, event: &WindowEvent) -> Vec<Outcome> {
        match event {
            WindowEvent::RedrawRequested => {
                // 第一帧补一次暗色标题栏：DWM 会先按浅色画一帧。
                if !self.frame_dark {
                    self.frame_dark = true;
                    crate::frame::apply(&self.window, &self.theme);
                }
                self.redraw();
                Vec::new()
            }
            WindowEvent::Resized(size) => {
                crate::frame::apply(&self.window, &self.theme);
                self.presenter.resize(size.width.max(1), size.height.max(1));
                self.window.request_redraw();
                Vec::new()
            }
            WindowEvent::CloseRequested => vec![Outcome::Close],
            WindowEvent::Focused(true) => {
                crate::frame::apply(&self.window, &self.theme);
                Vec::new()
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.window.scale_factor() as f32;
                let scale = if scale > 0.01 { scale } else { 1.0 };
                let p = Point::new(position.x as f32 / scale, position.y as f32 / scale);
                self.cursor = p;
                if self.drag_search {
                    let inner = search_inner(self.layout.search, !self.search.text().is_empty());
                    linefield::set_caret_from_x(&mut self.search, inner, p.x, true);
                    self.window.request_redraw();
                    return Vec::new();
                }
                let total = self.visible().len();
                let hover = row_at(&self.layout, p, self.scroll).filter(|&i| i < total);
                if hover != self.hover {
                    self.hover = hover;
                    self.window.set_cursor(if hover.is_some() {
                        CursorIcon::Pointer
                    } else {
                        CursorIcon::Default
                    });
                    self.window.request_redraw();
                }
                Vec::new()
            }
            WindowEvent::CursorLeft { .. } => {
                if self.hover.take().is_some() {
                    self.window.request_redraw();
                }
                Vec::new()
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                self.drag_search = false;
                Vec::new()
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !self.search.text().is_empty()
                    && clear_button(&self.layout).contains(self.cursor)
                {
                    self.clear_query();
                    return Vec::new();
                }
                if self.layout.search.contains(self.cursor) {
                    let has_text = !self.search.text().is_empty();
                    let inner = search_inner(self.layout.search, has_text);
                    let count = self.next_search_click();
                    self.search_focus = true;
                    match count {
                        1 => {
                            linefield::set_caret_from_x(
                                &mut self.search,
                                inner,
                                self.cursor.x,
                                false,
                            );
                            self.drag_search = true;
                        }
                        2 => linefield::select_word(&mut self.search, inner, self.cursor.x),
                        _ => linefield::select_all(&mut self.search),
                    }
                    self.window.request_redraw();
                    return Vec::new();
                }
                self.search_focus = false;
                // 点哪条就开哪条：鼠标用户不需要"先选中再回车"两下。
                if let Some(index) = self.hover {
                    self.selected = index;
                    self.clamp_selection();
                    return self.launch_selected();
                }
                Vec::new()
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / ROW_H,
                };
                if self.layout.search.contains(self.cursor)
                    && linefield::text_overflow(&self.search)
                {
                    linefield::scroll(&mut self.search, -lines * 48.0);
                    self.window.request_redraw();
                    return Vec::new();
                }
                let total = self.visible().len();
                let rows = self.visible_rows_cap().max(1);
                let next = scroll_from_wheel(self.scroll, total, rows, lines);
                if next != self.scroll {
                    self.scroll = next;
                    self.window.request_redraw();
                }
                Vec::new()
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                Vec::new()
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if self.typing_grace() {
                    return Vec::new();
                }
                let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
                if !cleaned.is_empty() {
                    linefield::insert(&mut self.search, &cleaned);
                    self.search_focus = true;
                    self.after_query_change();
                }
                Vec::new()
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return Vec::new();
                }
                if self.search_key(event) {
                    return Vec::new();
                }
                // 焦点在搜索框里：方向键/Home/End/Ctrl+A/C/X 都是编辑动作，别去翻列表。
                if self.search_focus {
                    let extend = self.modifiers.shift_key();
                    let motion = match &event.logical_key {
                        Key::Named(NamedKey::ArrowLeft) => Some(linefield::Motion::Left),
                        Key::Named(NamedKey::ArrowRight) => Some(linefield::Motion::Right),
                        Key::Named(NamedKey::Home) => Some(linefield::Motion::Home),
                        Key::Named(NamedKey::End) => Some(linefield::Motion::End),
                        _ => None,
                    };
                    if let Some(motion) = motion {
                        linefield::move_caret(&mut self.search, motion, extend);
                        self.window.request_redraw();
                        return Vec::new();
                    }
                    if self.modifiers.control_key() {
                        let mut redraw = false;
                        match &event.logical_key {
                            Key::Character(ch) if ch.as_str().eq_ignore_ascii_case("a") => {
                                linefield::select_all(&mut self.search);
                                redraw = true;
                            }
                            Key::Character(ch) if ch.as_str().eq_ignore_ascii_case("c") => {
                                linefield::copy(&self.search);
                            }
                            Key::Character(ch) if ch.as_str().eq_ignore_ascii_case("x") => {
                                if linefield::cut(&mut self.search) {
                                    self.after_query_change();
                                }
                            }
                            _ => {}
                        }
                        if redraw {
                            self.window.request_redraw();
                        }
                        if matches!(&event.logical_key, Key::Character(_)) {
                            return Vec::new();
                        }
                    }
                }
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => vec![Outcome::Close],
                    Key::Named(NamedKey::Enter) => self.launch_selected(),
                    Key::Named(NamedKey::ArrowUp) => {
                        self.move_by(-1);
                        Vec::new()
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        self.move_by(1);
                        Vec::new()
                    }
                    Key::Named(NamedKey::PageUp) => {
                        self.page(false);
                        Vec::new()
                    }
                    Key::Named(NamedKey::PageDown) => {
                        self.page(true);
                        Vec::new()
                    }
                    Key::Named(NamedKey::Home) => {
                        self.jump(false);
                        Vec::new()
                    }
                    Key::Named(NamedKey::End) => {
                        self.jump(true);
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }
}

impl Drop for LauncherWindow {
    fn drop(&mut self) {
        crate::frame::life("launcher", "destroyed", &self.window);
    }
}

/// 一次绘制的全部"可变"输入。
pub struct View<'a> {
    pub selected: usize,
    pub scroll: usize,
    pub hover: Option<usize>,
    pub search: &'a Search,
    pub search_focus: bool,
    pub placeholder: &'a str,
    pub footer: &'a str,
    pub empty: &'a str,
}

/// `visible` 是筛后的 `programs` 下标序列：画的是它，选中/滚动也是对它的下标。
pub fn draw(
    pixmap: &mut Pixmap,
    programs: &[Program],
    visible: &[usize],
    layout: &Layout,
    theme: &Theme,
    view: View<'_>,
) {
    let scale = pixmap.width() as f32 / layout.size.0.max(1.0);
    // 先擦干净：背景色是 98% 不透明的，直接往上盖会把上一帧的残留透出来
    // （搜索一变，行数就变，留下的那几行"鬼影"在暗色主题下很明显）。
    pixmap.fill(tiny_skia::Color::TRANSPARENT);
    let mut p = Painter::new(pixmap, scale);
    let pal = palette(theme);
    p.fill(
        Rect::new(0.0, 0.0, layout.size.0, layout.size.1),
        0.0,
        pal.bg,
    );
    draw_search(
        &mut p,
        layout.search,
        view.search,
        view.search_focus,
        view.placeholder,
        &pal,
    );

    p.set_clip(layout.list);
    if visible.is_empty() {
        p.text_in(view.empty, layout.list, 12.0, pal.dim, Weight::Regular);
    }
    for (row, rect) in layout.rows.iter().enumerate() {
        let row_index = row + view.scroll;
        let Some(index) = visible.get(row_index).copied() else {
            break;
        };
        let Some(program) = programs.get(index) else {
            break;
        };
        let selected = row_index == view.selected;
        if selected {
            p.fill(*rect, 7.0, pal.accent.with_alpha(0.18));
        } else if view.hover == Some(row_index) {
            p.fill(*rect, 7.0, pal.raised);
        }
        let inner = rect.inset(10.0, 6.0);
        // 右侧先占地：中间的名字用"行宽 - 路径列宽"去截断，反过来会把路径挤没。
        let folder = folder_label(&program.path);
        let name_rect = Rect::new(inner.x, inner.y, inner.w - SUB_W, inner.h);
        let name = p.truncate(&program.name, 12.5, name_rect.w);
        p.text_in(
            &name,
            name_rect,
            12.5,
            if selected {
                pal.text
            } else {
                pal.text.with_alpha(0.86)
            },
            Weight::Regular,
        );
        if !folder.is_empty() {
            let sub = Rect::new(inner.right() - SUB_W + 8.0, inner.y, SUB_W - 8.0, inner.h);
            p.text_right(&folder, sub, 11.0, pal.dim, Weight::Regular);
        }
    }
    p.clear_clip();

    p.text_in(view.footer, layout.footer, 11.0, pal.dim, Weight::Regular);
}

/// 路径的上一级目录名（右侧那列淡字）。
///
/// 直接用整条路径太长也太空——用户是靠"它在哪个文件夹里"认程序的。
/// 开始菜单根目录下的那条没有信息量，干脆不显示。
fn folder_label(path: &str) -> String {
    let Some(parent) = std::path::Path::new(path).parent() else {
        return String::new();
    };
    let name = parent
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.eq_ignore_ascii_case("Programs") {
        return String::new();
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Program> {
        vec![
            Program::new("Visual Studio Code", r"C:\Menu\Microsoft VS Code\Code.lnk"),
            Program::new("Notepad", r"C:\Menu\Notepad.lnk"),
        ]
    }

    #[test]
    fn folder_label_shows_the_parent_folder_and_hides_the_menu_root() {
        assert_eq!(
            folder_label(r"C:\Menu\Microsoft VS Code\Code.lnk"),
            "Microsoft VS Code"
        );
        // 开始菜单根目录本身没有信息量。
        assert_eq!(folder_label(r"C:\Menu\Programs\Notepad.lnk"), "");
        assert_eq!(folder_label(""), "");
    }

    #[test]
    fn layout_lists_visible_rows() {
        let programs = sample();
        let hits = zouwu_core::launcher::search(&programs, "note");
        assert_eq!(hits, vec![1]);
        let l = layout(programs.len());
        assert_eq!(l.rows.len(), programs.len());
    }

    #[test]
    fn draw_produces_pixels_offline() {
        // 本机截不了屏：靠离屏出图确认这一帧真的画出了东西（不是一片空底）。
        let programs = sample();
        let visible = zouwu_core::launcher::search(&programs, "");
        let l = layout(visible.len());
        let scale = 1.0;
        let mut pixmap = Pixmap::new((l.size.0 * scale) as u32, (l.size.1 * scale) as u32).unwrap();
        let theme = Theme::fallback();
        let search = Search::default();
        draw(
            &mut pixmap,
            &programs,
            &visible,
            &l,
            &theme,
            View {
                selected: 0,
                scroll: 0,
                hover: None,
                search: &search,
                search_focus: false,
                placeholder: "搜索程序",
                footer: "",
                empty: "",
            },
        );
        assert!(pixmap.pixels().iter().any(|px| px.alpha() != 0));
    }
}
