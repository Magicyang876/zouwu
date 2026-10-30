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
//! | 直接打字 | 搜索（空格分隔多个关键词，全部命中才算匹配） |
//! | Backspace | 删掉搜索的最后一个字符 |
//! | Ctrl+V | 把剪贴板里的第一行粘进搜索框 |
//! | Enter | 复制选中并关闭 |
//! | Delete | 从历史里删掉这一条 |
//! | Esc | 先清空搜索；搜索为空时才关闭 |

use std::sync::Arc;

use tiny_skia::Pixmap;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use zouwu_core::history::Entry;
use zouwu_core::theme::{Rgba, Theme};

use crate::linefield::{self, LineField};
use crate::presenter::{create_soft, Presenter};
use crate::widget::{Painter, Rect, Weight};
use zouwu_core::geom::Point;

pub const WIDTH: f32 = 560.0;
pub const MAX_ROWS: usize = 10;
pub const ROW_H: f32 = 44.0;
const PAD: f32 = 16.0;
const HEADER_H: f32 = 8.0;
/// 搜索框那一条占的高度（含与列表之间的间隙）。
const SEARCH_H: f32 = 36.0;
/// 搜索框本体的高度（比 `SEARCH_H` 矮，留出间隙）。
const SEARCH_BOX_H: f32 = 28.0;
const FOOTER_H: f32 = 34.0;
/// 搜索框字号（逻辑像素）。
pub const SEARCH_PX: f32 = 12.5;

/// 搜索框的状态：内容 + 光标/选区/可视起点。
///
/// 编辑规则整体复用 `linefield`（与设置面板的文本框同一份实现）：
/// 点哪落哪、拖着选、双击选词、长文本横向滚、Ctrl+A/C/X/V 都在那边。
pub struct Search {
    buffer: String,
    cursor: usize,
    anchor: usize,
    offset: usize,
    width: f32,
    scale: f32,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            anchor: 0,
            offset: 0,
            // 没量过宽度就 `INFINITY`：`ensure_caret_visible` 会跳过，单测不用伪造视口。
            width: f32::INFINITY,
            scale: 1.0,
        }
    }
}

impl Search {
    pub fn set(&mut self, text: &str) {
        self.buffer = text.to_string();
        self.cursor = self.buffer.len();
        self.anchor = self.cursor;
        self.offset = 0;
    }

    fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.anchor = 0;
        self.offset = 0;
    }
}

impl LineField for Search {
    fn text(&self) -> &str {
        &self.buffer
    }
    fn text_mut(&mut self) -> &mut String {
        &mut self.buffer
    }
    fn caret(&self) -> usize {
        self.cursor
    }
    fn set_caret(&mut self, index: usize) {
        self.cursor = index;
    }
    fn anchor(&self) -> usize {
        self.anchor
    }
    fn set_anchor(&mut self, index: usize) {
        self.anchor = index;
    }
    fn offset(&self) -> usize {
        self.offset
    }
    fn set_offset(&mut self, index: usize) {
        self.offset = index;
    }
    fn width(&self) -> f32 {
        self.width
    }
    fn set_width(&mut self, width: f32) {
        self.width = width;
    }
    fn scale(&self) -> f32 {
        self.scale
    }
    fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }
    fn font_px(&self) -> f32 {
        SEARCH_PX
    }
}

/// 列表顶边：头部 + 搜索框。
fn list_top() -> f32 {
    HEADER_H + SEARCH_H
}

/// 一次能在窗口里看到的条数。
pub fn visible_rows(total: usize) -> usize {
    total.min(MAX_ROWS)
}

/// 滚轮一格滚几行。三行是"看得见变化、又不会一下冲过头"的折中。
pub const WHEEL_ROWS: f32 = 3.0;

/// 按键对搜索框的影响。抽成纯数据是为了能单测：窗口那一层的副作用（重排/重绘）
/// 没法在无头环境里验，但“按 Esc 到底是清搜索还是关窗”这种事必须验。
#[derive(Clone, Debug, PartialEq)]
pub enum SearchEdit {
    /// 不归搜索框管，让列表处理（↑↓/Enter/Delete…）。
    Ignore,
    /// 清空搜索词。
    Clear,
    /// 删掉最后一个字符。
    Backspace,
    /// 追加这些字符。
    Append(String),
    /// 把剪贴板第一行粘进来。
    Paste,
}

/// 这个按键在搜索框里应该干什么。
///
/// `text` 是 winit 给出的可打印字符（受布局/输入法影响，比自己映射键位可靠）。
pub fn search_edit(key: &Key, text: Option<&str>, control: bool) -> SearchEdit {
    if control {
        if let Key::Character(ch) = key {
            if ch.eq_ignore_ascii_case("v") {
                return SearchEdit::Paste;
            }
        }
        // Ctrl+A、Ctrl+C 之类交给别人（目前没有别的处理器，就是“没反应”）。
        return SearchEdit::Ignore;
    }
    match key {
        // Esc 先交给搜索框（清空）；空的时候外部才关窗。
        Key::Named(NamedKey::Escape) => SearchEdit::Clear,
        Key::Named(NamedKey::Backspace) => SearchEdit::Backspace,
        _ => {
            let cleaned: String = text
                .unwrap_or("")
                .chars()
                .filter(|c| !c.is_control())
                .collect();
            if cleaned.is_empty() {
                SearchEdit::Ignore
            } else {
                SearchEdit::Append(cleaned)
            }
        }
    }
}

/// 选中行 → `entries` 下标。
///
/// 搜索把列表缩短后"第 3 行"和"`entries[3]`"就不是同一个了，所以两边都得走这里。
pub fn selected_entry(visible: &[usize], selected: usize) -> Option<usize> {
    visible.get(selected).copied()
}

/// 滚轮带来的新滚动偏移（只动视口，不动选中项）。
///
/// `lines` 是向上的格数（winit 的约定：正数＝向上滚）。像素级滚动（触摸板）按行高折算。
/// 夹在 `[0, total - rows]`：滚到底再滚不会越界，也不会出现空白。
pub fn scroll_from_wheel(scroll: usize, total: usize, rows: usize, lines: f32) -> usize {
    let max = total.saturating_sub(rows);
    if max == 0 || lines == 0.0 {
        return scroll.min(max);
    }
    // 小于半行的滑动（触摸板的细微移动）按一格算，否则手感是"滚了没反应"。
    let step = if lines.abs() < 0.5 {
        // 向下滚（lines < 0）＝往后看，偏移加一
        if lines > 0.0 {
            -1
        } else {
            1
        }
    } else {
        (lines.abs() * WHEEL_ROWS).round() as isize * if lines > 0.0 { -1 } else { 1 }
    };
    (scroll as isize + step).clamp(0, max as isize) as usize
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub size: (f32, f32),
    pub header: Rect,
    /// 搜索框。
    pub search: Rect,
    pub list: Rect,
    pub footer: Rect,
    pub rows: Vec<Rect>,
}

pub fn layout(total: usize) -> Layout {
    let rows_n = visible_rows(total);
    let list_h = rows_n.max(1) as f32 * ROW_H;
    let top = list_top();
    let h = top + list_h + FOOTER_H;
    Layout {
        size: (WIDTH, h),
        header: Rect::new(PAD, 0.0, WIDTH - PAD * 2.0, HEADER_H),
        search: Rect::new(PAD, HEADER_H + 2.0, WIDTH - PAD * 2.0, SEARCH_BOX_H),
        list: Rect::new(PAD, top, WIDTH - PAD * 2.0, list_h),
        footer: Rect::new(PAD, top + list_h, WIDTH - PAD * 2.0, FOOTER_H),
        rows: (0..rows_n)
            .map(|i| Rect::new(PAD, top + i as f32 * ROW_H, WIDTH - PAD * 2.0, ROW_H))
            .collect(),
    }
}

/// 搜索框里真正放文字的那一段（左边让开放大镜、右边留出「×」）。
///
/// 点击换算、光标绘制、滚动条都用它——三处各写一份必然错位。
pub fn search_inner(search: Rect, has_text: bool) -> Rect {
    let right_pad = if has_text { 26.0 } else { 10.0 };
    let text_x = search.x + 26.0;
    Rect::new(
        text_x,
        search.y,
        (search.right() - right_pad - text_x).max(10.0),
        search.h,
    )
}

/// 搜索框右侧那个「×」的点击范围（给定搜索框的矩形）。
pub fn clear_button_in(search: Rect) -> Rect {
    Rect::new(search.right() - 26.0, search.y, 22.0, search.h)
}

/// 搜索框右侧那个「×」的点击范围。
pub fn clear_button(layout: &Layout) -> Rect {
    clear_button_in(layout.search)
}

/// 查询拆成关键词（空格分隔，统一小写）。
pub fn query_terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(|t| t.to_lowercase()).collect()
}

/// 一条记录是否命中：**每个**关键词都是大小写不敏感的子串。
///
/// 多关键词取"与"而不是"或"：搜 `npm build` 的人不想要只提到 build 的 100 条。
pub fn matches_query(text: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let lower = text.to_lowercase();
    terms.iter().all(|term| lower.contains(term.as_str()))
}

/// 命中的记录在 `entries` 里的下标，保持原顺序（筛选不改变"新→旧"）。
pub fn filter(entries: &[Entry], terms: &[String]) -> Vec<usize> {
    if terms.is_empty() {
        return (0..entries.len()).collect();
    }
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches_query(&entry.text, terms))
        .map(|(index, _)| index)
        .collect()
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
    /// 当前选中项在**筛后可视列表**里的下标（不是 `entries` 下标，也不是屏幕行）。
    /// 映射回 `entries` 一律走 `visible()`。
    selected: usize,
    scroll: usize,
    hover: Option<usize>,
    /// 搜索框（词、光标、选区、可视起点）。
    search: Search,
    /// 键盘焦点在搜索框里（点了框或刚打过字）：此时 ←→/Home/End 动光标而不是列表。
    search_focus: bool,
    /// 正在框里拖选。
    drag_search: bool,
    /// 上一次按下搜索框的时间/位置，用来数双击、三击。
    last_click: Option<(std::time::Instant, f32, f32)>,
    click_count: u8,
    placeholder: String,
    no_match_text: String,
    cursor: Point,
    modifiers: ModifiersState,
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
        placeholder: String,
        no_match_text: String,
    ) -> Result<Self, String> {
        let layout = layout(entries.len());
        let attrs = Window::default_attributes()
            .with_title(&title)
            .with_window_icon(crate::icon::app_icon())
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
        // 搜索要能输中文：开会话 IME，候选窗随后由 `update_ime_area()` 贴到搜索框。
        window.set_ime_allowed(true);
        let this = Self {
            window,
            presenter,
            pixmap: None,
            theme,
            entries,
            selected: 0,
            scroll: 0,
            hover: None,
            search: Search::default(),
            search_focus: false,
            drag_search: false,
            last_click: None,
            click_count: 0,
            placeholder,
            no_match_text,
            cursor: Point::new(0.0, 0.0),
            modifiers: ModifiersState::empty(),
            layout,
            footer_help,
            footer_empty,
            empty_text,
            frame_dark: false,
        };
        this.update_ime_area();
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
        // 每次打开都是新的一轮：带着上次的搜索词打开会让人以为“记录都没了”。
        self.search.clear();
        self.refresh(entries);
        self.selected = 0;
        self.scroll = 0;
        self.window.set_visible(true);
        self.window.focus_window();
        self.window.request_redraw();
    }

    /// 设置搜索词（测试/调试入口：`ZOUWU_HISTORY_QUERY`）。
    pub fn set_query(&mut self, query: &str) {
        if self.search.buffer == query {
            return;
        }
        self.search.clear();
        self.search.set(query);
        self.after_query_change();
    }

    /// 当前命中的条数（给交互/调试用）。
    pub fn visible_len(&self) -> usize {
        self.visible().len()
    }

    fn clamp_selection(&mut self) {
        let total = self.visible().len();
        if total == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selected.min(total - 1);
        // 选中项必须可见：滚动偏移跟着选中走，但不要越过两端。
        let rows = visible_rows(total);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        let max_scroll = total.saturating_sub(rows);
        self.scroll = self.scroll.min(max_scroll);
    }

    fn rebuild_layout(&mut self) {
        let next = layout(self.visible().len());
        if (next.size.0 - self.layout.size.0).abs() > 0.5
            || (next.size.1 - self.layout.size.1).abs() > 0.5
        {
            // 条数跨过 MAX_ROWS 时窗口要跟着长高/变矮。
            let _ = self
                .window
                .request_inner_size(LogicalSize::new(next.size.0, next.size.1));
        }
        self.layout = next;
        self.sync_search_viewport();
    }

    fn move_by(&mut self, delta: isize) {
        let total = self.visible().len();
        if total == 0 {
            return;
        }
        let last = total as isize - 1;
        let next = (self.selected as isize + delta).clamp(0, last);
        self.selected = next as usize;
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
        let step = visible_rows(self.visible().len()).max(1) as isize;
        self.move_by(if down { step } else { -step });
    }

    /// 命中的记录：搜索词筛过后、`entries` 里的下标（新→旧）。
    fn visible(&self) -> Vec<usize> {
        filter(&self.entries, &query_terms(&self.search.buffer))
    }

    /// 搜索词变了：选中回到第一条，行数变了就重排，并让输入法候选窗跟过来。
    fn after_query_change(&mut self) {
        self.selected = 0;
        self.scroll = 0;
        self.rebuild_layout();
        self.sync_search_viewport();
        self.update_ime_area();
        self.window.request_redraw();
    }

    /// 把"框有多宽、现在什么 DPI"告诉搜索框：光标换算与横向滚动都要它。
    fn sync_search_viewport(&mut self) {
        let scale = self.window.scale_factor() as f32;
        let scale = if scale > 0.01 { scale } else { 1.0 };
        let inner = search_inner(self.layout.search, !self.search.buffer.is_empty());
        self.search.set_scale(scale);
        self.search.set_width(inner.w.max(1.0));
        linefield::ensure_caret_visible(&mut self.search);
    }

    fn clear_query(&mut self) {
        if self.search.buffer.is_empty() {
            return;
        }
        self.search.clear();
        self.after_query_change();
    }

    /// 把输入法候选窗贴到搜索框上（不贴的话候选窗会飘在屏幕左上角）。
    /// 数搜索框上的连击：同一位置 400ms 内的连续按下算一次加击（最多三击）。
    fn next_search_click(&mut self) -> u8 {
        let now = std::time::Instant::now();
        let near = self.last_click.is_some_and(|(at, x, y)| {
            now.duration_since(at) < std::time::Duration::from_millis(400)
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

    /// 搜索框先吃键。返回 true ＝ 这个键已被它吃掉。
    ///
    /// 故意没有"先点一下输入框才能打字"这层状态：这个窗口的用途就是“翻找”，
    /// 打开就能直接敲字最省事，而 ↑↓/Enter/Delete 仍然管列表。
    fn search_key(&mut self, event: &KeyEvent) -> bool {
        match search_edit(
            &event.logical_key,
            event.text.as_deref(),
            self.modifiers.control_key(),
        ) {
            SearchEdit::Ignore => false,
            SearchEdit::Clear => {
                // 第一次 Esc 只清搜索，第二次才关窗：误触不该直接关掉。
                if self.search.buffer.is_empty() {
                    false
                } else {
                    self.clear_query();
                    true
                }
            }
            SearchEdit::Backspace => {
                if linefield::backspace(&mut self.search) {
                    self.after_query_change();
                }
                self.search_focus = true;
                true
            }
            SearchEdit::Append(text) => {
                linefield::insert(&mut self.search, &text);
                self.search_focus = true;
                self.after_query_change();
                true
            }
            SearchEdit::Paste => {
                // 粘进光标处（多行会被折成一行，单行框里塞 `\n` 会画到框外）。
                self.search_focus = true;
                if linefield::paste_clipboard(&mut self.search) {
                    self.after_query_change();
                }
                true
            }
        }
    }

    fn copy_selected(&mut self) -> Vec<Outcome> {
        let visible = self.visible();
        let Some(index) = selected_entry(&visible, self.selected) else {
            return Vec::new();
        };
        let Some(entry) = self.entries.get(index) else {
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
        // 先把“筛后列表”算好：拿 `pixmap` 的可变借用后再读 `self` 会借不了。
        let visible = self.visible();
        let footer = if self.entries.is_empty() {
            self.footer_empty.as_str()
        } else {
            self.footer_help.as_str()
        };
        // "空"分两种：历史本来就没东西，还是搜索没命中——两句话差别很大。
        let empty = if !self.entries.is_empty() && !self.search.buffer.is_empty() {
            self.no_match_text.as_str()
        } else {
            self.empty_text.as_str()
        };
        let Some(pixmap) = self.pixmap.as_mut() else {
            return;
        };
        draw(
            pixmap,
            &self.entries,
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
                footer,
                empty,
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
                self.cursor = p;
                if self.drag_search {
                    // 拖拽扩选：锚点留在按下处，光标跟着鼠标走。
                    let inner = search_inner(self.layout.search, !self.search.buffer.is_empty());
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
                // 点「×」先于选行：那个位置在列表上方，两者不重叠，但读起来清楚。
                if !self.search.buffer.is_empty()
                    && clear_button(&self.layout).contains(self.cursor)
                {
                    self.clear_query();
                    return Vec::new();
                }
                // 点搜索框：定位光标；拖 = 选；双击 = 选词；三击 = 全选。
                if self.layout.search.contains(self.cursor) {
                    let has_text = !self.search.buffer.is_empty();
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
                // 点哪条就选哪条并直接复制：鼠标用户不需要"先选中再回车"两下。
                if let Some(index) = self.hover {
                    self.selected = index;
                    self.clamp_selection();
                    return self.copy_selected();
                }
                Vec::new()
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / ROW_H,
                };
                // 长搜索词横向滚：鼠标在框上时滚轮不翻列表，跟输入框一致。
                if self.layout.search.contains(self.cursor)
                    && linefield::text_overflow(&self.search)
                {
                    linefield::scroll(&mut self.search, -lines * 48.0);
                    self.window.request_redraw();
                    return Vec::new();
                }
                // 以前这里啥也没做：一屏放不下时只能靠 ↑↓ 翻，鼠标用户直接卡住。
                let total = self.visible().len();
                let rows = visible_rows(total);
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / ROW_H,
                };
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
                // 中文/日文输入法提交的整串字符走这里（走不了按键字符）。
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
                // 搜索框先吃键（打字/退格/Ctrl+V），再轮到列表。
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
                        // 注意报出去的是 **`entries` 下标**：删数据的是宿主，
                        // 给它一个筛后行号会删错条目。
                        match selected_entry(&self.visible(), self.selected) {
                            Some(index) => vec![Outcome::Delete(index)],
                            None => Vec::new(),
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
    /// 搜索框（内容非空＝正在筛选）。
    pub search: &'a Search,
    /// 光标在搜索框里（空框也要把光标画出来，否则用户不知道在打字）。
    pub search_focus: bool,
    /// 搜索框为空时显示的提示语。
    pub placeholder: &'a str,
    pub footer: &'a str,
    pub empty: &'a str,
}

/// `visible` 是筛后的 `entries` 下标序列：画的是它，选中/滚动也是对它的下标。
pub fn draw(
    pixmap: &mut Pixmap,
    entries: &[Entry],
    visible: &[usize],
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

    draw_search(&mut p, layout.search, &view, &pal);

    // 列表
    p.set_clip(layout.list);
    if visible.is_empty() {
        p.text_in(view.empty, layout.list, 12.0, pal.dim, Weight::Regular);
    }
    for (row, rect) in layout.rows.iter().enumerate() {
        // 行号（含滚动）与条目下标是两个名字空间：搜索后“第 0 行”往往是 `entries[3]`。
        let row_index = row + view.scroll;
        let Some(index) = visible.get(row_index).copied() else {
            break;
        };
        let Some(entry) = entries.get(index) else {
            break;
        };
        let selected = row_index == view.selected;
        let hovered = view.hover == Some(row_index);
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

/// 搜索框：左侧放大镜、提示语/输入内容、右侧「×」。
fn draw_search(p: &mut Painter<'_>, rect: Rect, view: &View<'_>, pal: &Palette) {
    let active = !view.search.buffer.is_empty();
    p.fill(rect, 7.0, pal.raised);
    p.stroke(
        rect,
        7.0,
        1.0,
        if active {
            pal.accent.with_alpha(0.75)
        } else {
            pal.dim.with_alpha(0.35)
        },
    );

    // 放大镜：两个圆 + 一根斜柄，比一个文字符号在任何字体下都稳。
    let cy = rect.center().y;
    p.circle(
        Point::new(rect.x + 15.0, cy - 1.0),
        4.2,
        pal.dim.with_alpha(0.75),
    );
    p.circle(Point::new(rect.x + 15.0, cy - 1.0), 2.4, pal.raised);
    p.line(
        rect.x + 18.0,
        cy + 2.0,
        rect.x + 21.0,
        cy + 5.0,
        pal.dim.with_alpha(0.75),
        1.4,
    );

    let area = search_inner(rect, active);
    if active || view.search_focus {
        // 内容、选区底纹、光标、横向滚动条都由 `linefield` 一处画：
        // 点在哪、选到哪、光标画在哪必须用同一份几何，各算一次必然对不上。
        linefield::draw_contents(
            p,
            area,
            view.search,
            pal.text,
            pal.dim.with_alpha(0.28),
            pal.accent,
        );
    } else {
        p.text_in(view.placeholder, area, SEARCH_PX, pal.dim, Weight::Regular);
    }
    if active {
        let clear = clear_button_in(rect);
        p.text_center("×", clear, 14.0, pal.dim, Weight::Regular);
    }
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
    fn the_wheel_scrolls_the_viewport_and_stops_at_the_ends() {
        // 一屏 10 行、共 30 条：可滚区间是 0..=20
        assert_eq!(
            scroll_from_wheel(0, 30, 10, -1.0),
            3,
            "向下滚一格＝下移三行"
        );
        assert_eq!(scroll_from_wheel(3, 30, 10, 1.0), 0, "向上滚回顶部");
        assert_eq!(scroll_from_wheel(0, 30, 10, 1.0), 0, "到顶不再动");
        assert_eq!(scroll_from_wheel(20, 30, 10, -1.0), 20, "到底不再动");
        assert_eq!(
            scroll_from_wheel(19, 30, 10, -5.0),
            20,
            "一次滚很远也只到边界"
        );
    }

    #[test]
    fn the_wheel_does_nothing_when_everything_fits() {
        for scroll in [0, 5] {
            assert_eq!(scroll_from_wheel(scroll, 4, 4, -1.0), 0);
        }
    }

    #[test]
    fn tiny_trackpad_slides_still_move_one_row() {
        // 触摸板一次只推 0.1 行，不能因为取整变成"滚了没反应"
        assert_eq!(scroll_from_wheel(5, 30, 10, -0.05), 6);
        assert_eq!(scroll_from_wheel(5, 30, 10, 0.05), 4);
        assert_eq!(scroll_from_wheel(5, 30, 10, 0.0), 5);
    }

    #[test]
    fn pixel_deltas_are_converted_to_rows() {
        // 触摸板的像素级滚动按行高折算（44px 一行）后走同一套夹取逻辑
        let lines = -ROW_H * 2.0 / ROW_H;
        assert_eq!(scroll_from_wheel(0, 30, 10, lines), 6);
    }

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
            assert!((row.y - (list_top() + i as f32 * ROW_H)).abs() < 0.01);
            assert!((row.h - ROW_H).abs() < 0.01);
        }
        let last = l.rows.last().unwrap();
        assert!((last.bottom() - l.list.bottom()).abs() < 0.01);
    }

    #[test]
    fn the_search_box_sits_above_the_list() {
        let l = layout(5);
        assert!(
            l.search.bottom() <= l.list.y + 0.01,
            "search must not overlap the rows"
        );
        assert!(l.search.y >= l.header.bottom() - 0.01);
        assert!(l.search.h < SEARCH_H, "box is shorter than its slot");
        // 零条命中时也要留出搜索框的高度（空结果也得能继续改关键词）。
        assert!(l.size.1 >= list_top() + FOOTER_H + ROW_H);
    }

    #[test]
    fn the_clear_button_lives_inside_the_search_box() {
        let l = layout(3);
        let clear = clear_button(&l);
        assert!(l.search.contains(clear.center()));
        assert!((clear.right() - l.search.right()).abs() < 8.0);
    }

    fn sample_entries() -> Vec<Entry> {
        [
            "cargo test --workspace",
            "git commit -m search",
            "有一个中文字符串",
            "C:\\work\\tools\\run.exe",
        ]
        .iter()
        .enumerate()
        .map(|(i, text)| Entry {
            text: (*text).to_string(),
            copied_at: 1000 + i as u64,
        })
        .collect()
    }

    #[test]
    fn matching_is_case_insensitive_and_substring() {
        let terms = query_terms("CARGO");
        assert!(matches_query("cargo test", &terms));
        assert!(matches_query("run CARGO now", &terms));
        assert!(!matches_query("carg", &terms));
        // 空查询＝全匹配（连纯空白也算空）
        assert!(matches_query("anything", &query_terms("   ")));
    }

    #[test]
    fn every_term_must_match() {
        let terms = query_terms("cargo test");
        assert!(matches_query("cargo test --workspace", &terms));
        assert!(!matches_query("cargo build", &terms), "缺一个词就不算命中");
        assert!(!matches_query("test only", &terms));
    }

    #[test]
    fn filtering_keeps_the_newest_first_order() {
        let entries = sample_entries();
        assert_eq!(filter(&entries, &query_terms("")), vec![0, 1, 2, 3]);
        assert_eq!(filter(&entries, &query_terms("cargo")), vec![0]);
        assert_eq!(filter(&entries, &query_terms("搜索")), Vec::<usize>::new());
        assert_eq!(filter(&entries, &query_terms("中文")), vec![2]);
        assert_eq!(filter(&entries, &query_terms("run")), vec![3]);
        assert_eq!(filter(&entries, &query_terms("nope")), Vec::<usize>::new());
    }

    #[test]
    fn a_search_hit_maps_back_to_the_entry_index() {
        // 这是最容易错的地方：筛选后第 0 行不该被当成 entries[0]。
        let entries = sample_entries();
        let visible = filter(&entries, &query_terms("run.exe"));
        assert_eq!(visible, vec![3]);
        assert_eq!(entries[visible[0]].text, "C:\\work\\tools\\run.exe");
    }

    #[test]
    fn extreme_queries_do_not_panic() {
        let entries = sample_entries();
        // 全角空格、大小写混合、只有空格——都不能把列表搞崩。
        for query in ["　", "CARGO  TEST", "   ", "cargo\tcargo"] {
            let visible = filter(&entries, &query_terms(query));
            assert!(visible.len() <= entries.len());
        }
    }

    #[test]
    fn search_keys_are_classified_before_the_list_gets_them() {
        use winit::keyboard::SmolStr;
        let named = |k: NamedKey| Key::Named(k);
        // 打字 → 追加
        assert_eq!(
            search_edit(&Key::Character(SmolStr::new("a")), Some("a"), false),
            SearchEdit::Append("a".into())
        );
        // 中文输入法提交的整串
        assert_eq!(
            search_edit(&Key::Character(SmolStr::new("文")), Some("中文"), false),
            SearchEdit::Append("中文".into())
        );
        // 退格 / Esc / Ctrl+V
        assert_eq!(
            search_edit(&named(NamedKey::Backspace), None, false),
            SearchEdit::Backspace
        );
        assert_eq!(
            search_edit(&named(NamedKey::Escape), None, false),
            SearchEdit::Clear
        );
        assert_eq!(
            search_edit(&Key::Character(SmolStr::new("v")), None, true),
            SearchEdit::Paste
        );
        assert_eq!(
            search_edit(&Key::Character(SmolStr::new("V")), None, true),
            SearchEdit::Paste,
            "大写 V 也是粘贴"
        );
        // 方向键、Enter、Delete 不归搜索框
        for key in [
            named(NamedKey::ArrowDown),
            named(NamedKey::Enter),
            named(NamedKey::Delete),
            named(NamedKey::Home),
        ] {
            assert_eq!(search_edit(&key, None, false), SearchEdit::Ignore);
        }
        // Ctrl+A 不要变成字母 a
        assert_eq!(
            search_edit(&Key::Character(SmolStr::new("a")), Some("a"), true),
            SearchEdit::Ignore
        );
        // 控制字符不进入查询
        assert_eq!(
            search_edit(&Key::Character(SmolStr::new("\t")), Some("\t"), false),
            SearchEdit::Ignore
        );
    }

    #[test]
    fn the_selected_row_follows_the_filter() {
        let entries = sample_entries();
        let all = filter(&entries, &query_terms(""));
        // 没搜索时行号就是条目下标
        assert_eq!(selected_entry(&all, 2), Some(2));
        // 搜索后第 0 行是原来的第 3 条
        let only_path = filter(&entries, &query_terms("run.exe"));
        assert_eq!(selected_entry(&only_path, 0), Some(3));
        // 越界不再返回“最后一条”而是 None——否则空列表也会“选中第 0 条”
        assert_eq!(selected_entry(&only_path, 1), None);
        assert_eq!(selected_entry(&[], 0), None);
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

    // ── 搜索框的鼠标行为 ──────────────────────

    /// 按真实度量算"第 `index` 个字节处的横坐标"（内框左沿为 0）。
    fn caret_x_of(text: &str, index: usize) -> f32 {
        linefield::width_between(text, 0, index, 1.0, SEARCH_PX)
    }

    fn search_with(text: &str) -> Search {
        let mut search = Search::default();
        search.set(text);
        let inner = search_inner(layout(0).search, !text.is_empty());
        search.set_scale(1.0);
        search.set_width(inner.w);
        search
    }

    fn search_inner_rect(text: &str) -> Rect {
        search_inner(layout(0).search, !text.is_empty())
    }

    #[test]
    fn clicking_the_search_box_puts_the_caret_under_the_pointer() {
        let mut search = search_with("cargo test");
        let inner = search_inner_rect("cargo test");
        // 点在 "test" 前面（第 6 个字节处）
        let x = inner.x + caret_x_of("cargo test", 6);
        linefield::set_caret_from_x(&mut search, inner, x, false);
        assert_eq!(search.cursor, 6, "点哪落哪");
        assert_eq!(search.anchor, 6, "单击不产生选区");
        // 点到文字右边很远处：夹到末尾
        linefield::set_caret_from_x(&mut search, inner, inner.right() + 200.0, false);
        assert_eq!(search.cursor, 10);
        // 点到左边很远处：夹到开头
        linefield::set_caret_from_x(&mut search, inner, inner.x - 200.0, false);
        assert_eq!(search.cursor, 0);
    }

    #[test]
    fn clicking_a_chinese_query_lands_on_a_character_boundary() {
        let text = "中文字符";
        let mut search = search_with(text);
        let inner = search_inner_rect(text);
        // 点在第 2 个字的后半（过了中点才落到下一个边界）
        let x = inner.x + caret_x_of(text, 3) + 10.0;
        linefield::set_caret_from_x(&mut search, inner, x, false);
        assert!(text.is_char_boundary(search.cursor), "不能切在多字节中间");
        assert_eq!(search.cursor, 6);
    }

    #[test]
    fn dragging_inside_the_search_box_selects_a_range() {
        let text = "cargo test";
        let mut search = search_with(text);
        let inner = search_inner_rect(text);
        // 按下（锚点）在第 6 字节，拖到第 10 字节
        linefield::set_caret_from_x(&mut search, inner, inner.x + caret_x_of(text, 6), false);
        linefield::set_caret_from_x(&mut search, inner, inner.x + caret_x_of(text, 10), true);
        assert_eq!(
            linefield::selection(&search),
            Some((6, 10)),
            "拖选保留锚点，得到一段选区"
        );
    }

    #[test]
    fn double_clicking_selects_one_word_in_the_search() {
        let text = "C:\\work\\tools\\run.exe";
        let mut search = search_with(text);
        let inner = search_inner_rect(text);
        // 双击 tools 中间
        let mid = caret_x_of(text, 9) + 2.0;
        linefield::select_word(&mut search, inner, inner.x + mid);
        let picked = &text[search.anchor..search.cursor];
        assert_eq!(picked, "tools", "双击按路径分隔符切词");
        // 双击中文：整串汉字算一个词
        let zh = "中文字符";
        let mut search = search_with(zh);
        let inner = search_inner_rect(zh);
        linefield::select_word(&mut search, inner, inner.x + 4.0);
        assert_eq!(&zh[search.anchor..search.cursor], zh);
    }

    #[test]
    fn typing_and_backspace_replace_the_selection() {
        let text = "cargo test";
        let mut search = search_with(text);
        let inner = search_inner_rect(text);
        // 双击选中 test，然后打字顶掉
        linefield::select_word(&mut search, inner, inner.x + caret_x_of(text, 8));
        assert_eq!(&text[search.anchor..search.cursor], "test");
        linefield::insert(&mut search, "build");
        assert_eq!(search.buffer, "cargo build");
        assert_eq!(search.cursor, search.buffer.len());
        // 再全选后按退格：一次清空
        linefield::select_all(&mut search);
        assert!(linefield::backspace(&mut search));
        assert!(search.buffer.is_empty());
    }

    #[test]
    fn a_long_query_scrolls_so_the_caret_stays_visible() {
        let text = "一个很长很长的搜索词用来测试水平滚动到底会不会把光标挤出框外";
        let mut search = Search::default();
        search.set(text);
        // 窄框：只有 60px
        search.set_scale(1.0);
        search.set_width(60.0);
        linefield::ensure_caret_visible(&mut search);
        assert!(search.offset > 0, "光标在末尾，必须往左滚");
        assert!(text.is_char_boundary(search.offset));
        // 滚过去之后，光标到可视起点的宽度必须放得下
        let shown = linefield::width_between(text, search.offset, search.cursor, 1.0, SEARCH_PX);
        assert!(shown <= 60.0 + 0.01, "滚动之后光标要在框里：{shown}");
        // Ctrl+A 之后光标回到末尾，但可视起点不变（全选不需要滚）
        linefield::select_all(&mut search);
        assert_eq!(search.cursor, text.len());
    }

    #[test]
    fn a_wide_query_shows_a_scrollbar() {
        let text = "一个很长很长的搜索词用来测试水平滚动到底会不会把光标挤出框外";
        let mut search = search_with("短");
        let inner = search_inner_rect("短");
        assert!(!linefield::text_overflow(&search), "短词不该有滚动条");
        assert!(linefield::thumb(&search, inner).is_none());
        search.set(text);
        search.set_width(60.0);
        assert!(linefield::text_overflow(&search));
        let thumb = linefield::thumb(&search, inner).expect("溢出就有 thumb");
        assert!(thumb.w >= 24.0, "thumb 不能窄到点不到：{}", thumb.w);
        // 拖到最右边：可视起点应该到末尾附近
        linefield::set_offset_from_x(&mut search, inner, inner.right());
        assert!(search.offset > 0);
        assert!(text.is_char_boundary(search.offset));
    }

    #[test]
    fn the_search_box_text_area_leaves_room_for_the_icon_and_the_clear_button() {
        let box_ = layout(0).search;
        let empty = search_inner(box_, false);
        let typed = search_inner(box_, true);
        assert!(empty.x > box_.x + 20.0, "左边给放大镜让位");
        assert!(typed.right() < box_.right() - 20.0, "有字时右边给 × 让位");
        assert!(typed.w > 40.0, "别把自己挤没了");
    }
}
