//! 剪贴板历史窗。
//!
//! 与 `report` 分开写，因为交互模型不一样：结果是"看/复制"，历史是"翻/挑"。
//! 挑东西必须能用键盘——鼠标还得去够那个窗口，而用户按快捷键打开它的本意
//! 就是不想离开键盘。
//!
//! 键盘映射（与常见剪贴板工具保持一致，换工具时不必重新学）：
//!
//! | 键 | 动作 |
//! |---|---|
//! | ↑ / ↓ | 移动选中 |
//! | Home / End | 跳到首/尾 |
//! | PageUp / PageDown | 翻一屏 |
//! | Enter | 复制选中并关闭 |
//! | Delete | 从历史里删掉这一条 |
//! | Esc | 关闭 |

use std::sync::Arc;

use tiny_skia::Pixmap;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use zouwu_core::history::Entry;
use zouwu_core::theme::{Rgba, Theme};

use crate::presenter::{create_soft, Presenter};
use crate::widget::{Painter, Rect, Weight};
use zouwu_core::geom::Point;

pub const WIDTH: f32 = 560.0;
pub const MAX_ROWS: usize = 10;
pub const ROW_H: f32 = 44.0;
const PAD: f32 = 16.0;
const HEADER_H: f32 = 8.0;
const FOOTER_H: f32 = 34.0;

/// 一次能在窗口里看到的条数。
pub fn visible_rows(total: usize) -> usize {
    total.min(MAX_ROWS)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub size: (f32, f32),
    pub header: Rect,
    pub list: Rect,
    pub footer: Rect,
    pub rows: Vec<Rect>,
}

pub fn layout(total: usize) -> Layout {
    let rows_n = visible_rows(total);
    let list_h = rows_n.max(1) as f32 * ROW_H;
    let h = HEADER_H + list_h + FOOTER_H;
    Layout {
        size: (WIDTH, h),
        header: Rect::new(PAD, 0.0, WIDTH - PAD * 2.0, HEADER_H),
        list: Rect::new(PAD, HEADER_H, WIDTH - PAD * 2.0, list_h),
        footer: Rect::new(PAD, HEADER_H + list_h, WIDTH - PAD * 2.0, FOOTER_H),
        rows: (0..rows_n)
            .map(|i| Rect::new(PAD, HEADER_H + i as f32 * ROW_H, WIDTH - PAD * 2.0, ROW_H))
            .collect(),
    }
}

/// 指针落在第几行（含滚动偏移）。
pub fn row_at(layout: &Layout, point: Point, scroll: usize) -> Option<usize> {
    layout
        .rows
        .iter()
        .position(|row| row.contains(point))
        .map(|i| i + scroll)
}

/// 窗口对宿主说的话。
pub enum Outcome {
    /// 复制了某一条（宿主只需关窗）。
    Copied(String),
    /// 用户要求删掉某一条（由宿主改数据并回填）。
    Delete(usize),
    Close,
}

pub struct HistoryWindow {
    window: Arc<Window>,
    presenter: Box<dyn Presenter>,
    pixmap: Option<Pixmap>,
    theme: Theme,
    entries: Vec<Entry>,
    /// 当前选中项在 `entries` 里的下标（不是屏幕行）。
    selected: usize,
    scroll: usize,
    hover: Option<usize>,
    layout: Layout,
    footer_help: String,
    footer_empty: String,
    empty_text: String,
    /// 第一帧已经补刷过暗色标题栏。
    frame_dark: bool,
}

impl HistoryWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_loop: &ActiveEventLoop,
        entries: Vec<Entry>,
        theme: Theme,
        title: String,
        footer_help: String,
        footer_empty: String,
        empty_text: String,
    ) -> Result<Self, String> {
        let layout = layout(entries.len());
        let attrs = Window::default_attributes()
            .with_title(&title)
            .with_inner_size(LogicalSize::new(layout.size.0, layout.size.1))
            .with_resizable(false)
            .with_visible(true);
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("create history window: {e}"))?,
        );
        let physical = window.inner_size();
        let presenter = create_soft(window.clone(), physical.width, physical.height)?;
        crate::frame::apply(&window, &theme);
        crate::frame::life("history", "created", &window);
        window.focus_window();
        let this = Self {
            window,
            presenter,
            pixmap: None,
            theme,
            entries,
            selected: 0,
            scroll: 0,
            hover: None,
            layout,
            footer_help,
            footer_empty,
            empty_text,
            frame_dark: false,
        };
        this.window.request_redraw();
        Ok(this)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn close(&self) {
        crate::frame::life("history", "hidden", &self.window);
        self.window.set_visible(false);
    }

    /// 重新上图（换肤后子窗口也得跟着变，否则旧配色会一直粘着）。
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        crate::frame::apply(&self.window, &self.theme);
        self.window.request_redraw();
    }

    /// 历史变了（新条目进来 / 删掉一条）：保位置、重排。
    pub fn refresh(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        self.clamp_selection();
        self.rebuild_layout();
        self.window.request_redraw();
    }

    /// 重新展示（窗口是复用的，不是重建的）。
    ///
    /// **不要改成“关掉就销毁”**：析构一个当时正持有焦点的窗口会让
    /// USER32 同步去通知另一个进程的窗口，那边慢一点整进程就冻住了。
    pub fn show(&mut self, entries: Vec<Entry>) {
        self.refresh(entries);
        self.window.set_visible(true);
        self.window.focus_window();
        self.window.request_redraw();
    }

    fn clamp_selection(&mut self) {
        if self.entries.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selected.min(self.entries.len() - 1);
        // 选中项必须可见：滚动偏移跟着选中走，但不要越过两端。
        let rows = visible_rows(self.entries.len());
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        let max_scroll = self.entries.len().saturating_sub(rows);
        self.scroll = self.scroll.min(max_scroll);
    }

    fn rebuild_layout(&mut self) {
        let next = layout(self.entries.len());
        if (next.size.0 - self.layout.size.0).abs() > 0.5
            || (next.size.1 - self.layout.size.1).abs() > 0.5
        {
            // 条数跨过 MAX_ROWS 时窗口要跟着长高/变矮。
            let _ = self
                .window
                .request_inner_size(LogicalSize::new(next.size.0, next.size.1));
        }
        self.layout = next;
    }

    fn move_by(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as isize - 1;
        let next = (self.selected as isize + delta).clamp(0, last);
        self.selected = next as usize;
        self.clamp_selection();
        self.window.request_redraw();
    }

    fn jump(&mut self, to_end: bool) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = if to_end { self.entries.len() - 1 } else { 0 };
        self.clamp_selection();
        self.window.request_redraw();
    }

    fn page(&mut self, down: bool) {
        let step = visible_rows(self.entries.len()).max(1) as isize;
        self.move_by(if down { step } else { -step });
    }

    fn copy_selected(&mut self) -> Vec<Outcome> {
        let Some(entry) = self.entries.get(self.selected) else {
            return Vec::new();
        };
        if zouwu_platform::clipboard::set_text(&entry.text) {
            vec![Outcome::Copied(entry.text.clone()), Outcome::Close]
        } else {
            // 剪贴板被别的程序占着：不要假装成功，也不要关窗。
            log::warn!("history: clipboard is busy, could not copy");
            self.window.request_redraw();
            Vec::new()
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
        let Some(pixmap) = self.pixmap.as_mut() else {
            return;
        };
        let footer = if self.entries.is_empty() {
            self.footer_empty.as_str()
        } else {
            self.footer_help.as_str()
        };
        draw(
            pixmap,
            &self.entries,
            &self.layout,
            &self.theme,
            View {
                selected: self.selected,
                scroll: self.scroll,
                hover: self.hover,
                footer,
                empty: &self.empty_text,
            },
        );

        if let Err(err) = self.presenter.present(pixmap) {
            log::error!("history: present failed: {err}");
        }
    }

    pub fn window_event(&mut self, event: &WindowEvent) -> Vec<Outcome> {
        match event {
            WindowEvent::RedrawRequested => {
                // 窗口第一次上屏时 DWM 会先按浅色画一帧边框，创建时设的属性可能被
                // 这次绘制覆盖，所以第一帧再补一次。
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
                let hover =
                    row_at(&self.layout, p, self.scroll).filter(|&i| i < self.entries.len());
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
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // 点哪条就选哪条并直接复制：鼠标用户不需要"先选中再回车"两下。
                if let Some(index) = self.hover {
                    self.selected = index;
                    self.clamp_selection();
                    return self.copy_selected();
                }
                Vec::new()
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return Vec::new();
                }
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => vec![Outcome::Close],
                    Key::Named(NamedKey::Enter) => self.copy_selected(),
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
                    Key::Named(NamedKey::Delete) => {
                        if self.entries.is_empty() {
                            Vec::new()
                        } else {
                            // 删除由宿主改数据再回填：窗口不掌握唯一真相。
                            vec![Outcome::Delete(self.selected)]
                        }
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }
}

/// 真析构。卡死问题就出在“把这个时刻放进了事件回调”，所以单独打一行。
impl Drop for HistoryWindow {
    fn drop(&mut self) {
        crate::frame::life("history", "destroyed", &self.window);
    }
}

/// 一次绘制的全部“可变”输入。抽出来是为了能离屏出图验收（本机截不了屏）。
pub struct View<'a> {
    pub selected: usize,
    pub scroll: usize,
    pub hover: Option<usize>,
    pub footer: &'a str,
    pub empty: &'a str,
}

pub fn draw(
    pixmap: &mut Pixmap,
    entries: &[Entry],
    layout: &Layout,
    theme: &Theme,
    view: View<'_>,
) {
    let scale = pixmap.width() as f32 / layout.size.0.max(1.0);
    let mut p = Painter::new(pixmap, scale);
    let pal = palette(theme);
    p.fill(
        Rect::new(0.0, 0.0, layout.size.0, layout.size.1),
        0.0,
        pal.bg,
    );

    // 列表
    p.set_clip(layout.list);
    if entries.is_empty() {
        p.text_in(view.empty, layout.list, 12.0, pal.dim, Weight::Regular);
    }
    for (row, rect) in layout.rows.iter().enumerate() {
        let index = row + view.scroll;
        let Some(entry) = entries.get(index) else {
            break;
        };
        let selected = index == view.selected;
        let hovered = view.hover == Some(index);
        if selected {
            p.fill(*rect, 7.0, pal.accent.with_alpha(0.18));
        } else if hovered {
            p.fill(*rect, 7.0, pal.raised);
        }
        let inner = rect.inset(10.0, 6.0);
        // 右侧先占地，中间文字用“行宽 - 时间列宽”去截断：
        // 反过来先算文字宽度，时间就会被碰掉。
        const TIME_W: f32 = 96.0;
        let preview = p.truncate(&entry.preview(), 12.0, inner.w - TIME_W);
        let line = if entry.is_multiline() {
            format!("{preview} ↵")
        } else {
            preview
        };
        let text_rect = Rect::new(inner.x, inner.y, inner.w - TIME_W, inner.h);
        p.text_in(
            &line,
            text_rect,
            12.0,
            if selected {
                pal.text
            } else {
                pal.text.with_alpha(0.86)
            },
            Weight::Regular,
        );
        // 相对时间：绝对时间戳对“找刚才那条”没有帮助。
        let ago = ago_label(entry.copied_at, now_secs());
        let right = Rect::new(inner.right() - TIME_W + 8.0, inner.y, TIME_W - 8.0, inner.h);
        p.text_right(&ago, right, 11.0, pal.dim, Weight::Regular);
    }
    p.clear_clip();

    p.text_in(view.footer, layout.footer, 11.0, pal.dim, Weight::Regular);
}

#[derive(Clone, Copy)]
struct Palette {
    bg: Rgba,
    raised: Rgba,
    text: Rgba,
    dim: Rgba,
    accent: Rgba,
}

fn palette(theme: &Theme) -> Palette {
    Palette {
        bg: theme.bubble_bg.with_alpha(0.98),
        raised: theme.bubble_bg_active.with_alpha(0.9),
        text: theme.glyph,
        dim: theme.glyph.with_alpha(0.62),
        accent: theme.accent,
    }
}

/// 相对时间："刚刚" / "3 分钟前" / "2 小时前" / "3 天前"。
///
/// 显示绝对时间戳没用：用户要判断的是"这是不是我刚才复制的那条"，
/// 而"刚刚"比 "14:03:27" 更能回答这个问题。
pub fn ago_label(copied_at: u64, now: u64) -> String {
    let delta = now.saturating_sub(copied_at);
    match delta {
        0..=30 => "刚刚".to_string(),
        31..=59 => "1 分钟内".to_string(),
        60..=3599 => format!("{} 分钟前", delta / 60),
        3600..=86_399 => format!("{} 小时前", delta / 3600),
        _ => format!("{} 天前", delta / 86_400),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_grows_with_rows_up_to_the_cap() {
        let two = layout(2);
        let ten = layout(10);
        let many = layout(80);
        assert!(ten.size.1 > two.size.1);
        assert_eq!(
            many.size.1, ten.size.1,
            "beyond MAX_ROWS the window must stop growing"
        );
        assert_eq!(many.rows.len(), MAX_ROWS);
    }

    #[test]
    fn rows_tile_the_list_without_gaps() {
        let l = layout(5);
        for (i, row) in l.rows.iter().enumerate() {
            assert!((row.y - (HEADER_H + i as f32 * ROW_H)).abs() < 0.01);
            assert!((row.h - ROW_H).abs() < 0.01);
        }
        let last = l.rows.last().unwrap();
        assert!((last.bottom() - l.list.bottom()).abs() < 0.01);
    }

    #[test]
    fn row_at_accounts_for_the_scroll_offset() {
        let l = layout(20);
        let third_row = l.rows[2].center();
        assert_eq!(row_at(&l, third_row, 0), Some(2));
        assert_eq!(
            row_at(&l, third_row, 3),
            Some(5),
            "scrolled list must map to the entry index, not the screen row"
        );
        // 页脚与头部不在可点范围内。
        assert_eq!(row_at(&l, Point::new(PAD + 2.0, 4.0), 0), None);
        assert_eq!(row_at(&l, Point::new(PAD + 2.0, l.size.1 - 4.0), 0), None);
    }

    #[test]
    fn relative_time_is_human() {
        assert_eq!(ago_label(100, 100), "刚刚");
        assert_eq!(ago_label(100, 140), "1 分钟内");
        assert_eq!(ago_label(100, 160), "1 分钟前");
        assert_eq!(ago_label(100, 100 + 300), "5 分钟前");
        assert_eq!(ago_label(0, 7200), "2 小时前");
        assert_eq!(ago_label(0, 86_400 * 3), "3 天前");
        // 时钟回拨：差值算成 0，不要显示"18446744073 秒前"。
        assert_eq!(ago_label(500, 100), "刚刚");
    }
}
