//! 单行输入框的几何与编辑动作。
//!
//! 设置面板的「名称 / 命令」框和剪贴板历史的搜索框要的是同一套行为：点哪落哪、
//! 拖着选、双击选词、长文本横向滚、Ctrl+A/C/X/V。这些规则的坑都长在同一个地方
//! （多字节字符边界、光标与可视起点互相追、宽窗口窄窗口都别 panic），所以只写一份，
//! 两个界面各自实现一个 10 行的 [`LineField`] 就行。
//!
//! 实现者只提供"数据"，算法都在本模块：
//! - 光标 / 锚点 / 可视起点都是**字节下标**，且始终落在字符边界上（中文、emoji 不能被切一半）；
//! - `width` 是内框宽度（逻辑像素），`scale` 是 DPI 缩放，`font_px` 是字号；
//! - `width` 默认 `INFINITY`：没量过宽度时"保持光标可见"什么都不做，
//!   于是只改模型不画界面的单元测试不需要伪造视口。

use crate::widget::Rect;
use zouwu_core::geom::Point;
use zouwu_core::theme::Rgba;

/// 单行框的数据（字体、视口、光标、选区、可视起点）。
pub trait LineField {
    fn text(&self) -> &str;
    fn text_mut(&mut self) -> &mut String;
    fn caret(&self) -> usize;
    fn set_caret(&mut self, index: usize);
    fn anchor(&self) -> usize;
    fn set_anchor(&mut self, index: usize);
    fn offset(&self) -> usize;
    fn set_offset(&mut self, index: usize);
    /// 内框宽度（逻辑像素）；`INFINITY` 表示还没量过。
    fn width(&self) -> f32;
    fn set_width(&mut self, width: f32);
    fn scale(&self) -> f32;
    fn set_scale(&mut self, scale: f32);
    /// 字号（逻辑像素）。两个界面的字号不一样，所以量宽度得问它。
    fn font_px(&self) -> f32;
}

// ── 字符边界 ──────────────────────────────

/// 夹到一个合法的字符边界上（`str` 切片不能落在多字节字符中间）。
pub fn clamp_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// 前一个字符边界。
pub fn prev_boundary(text: &str, index: usize) -> Option<usize> {
    let index = clamp_boundary(text, index);
    if index == 0 {
        return None;
    }
    let mut prev = index - 1;
    while prev > 0 && !text.is_char_boundary(prev) {
        prev -= 1;
    }
    Some(prev)
}

/// 后一个字符边界。
pub fn next_boundary(text: &str, index: usize) -> Option<usize> {
    let index = clamp_boundary(text, index);
    if index >= text.len() {
        return None;
    }
    let mut next = index + 1;
    while next < text.len() && !text.is_char_boundary(next) {
        next += 1;
    }
    Some(next)
}

// ── 量宽度 ────────────────────────────────

/// 量一段文字的宽度（逻辑像素）。`scale` 是 DPI 缩放，`px` 是字号。
pub fn measure_text(text: &str, scale: f32, px: f32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    crate::text::TextEngine::with(|engine| engine.measure(text, px * scale, false)) / scale
}

/// 量一段字节区间（`from..to`）的宽度。
pub fn width_between(text: &str, from: usize, to: usize, scale: f32, px: f32) -> f32 {
    let from = clamp_boundary(text, from);
    let to = clamp_boundary(text, to).max(from);
    measure_text(&text[from..to], scale, px)
}

/// 整段文本的宽度（判断要不要出滚动条）。
pub fn text_width(text: &str, scale: f32, px: f32) -> f32 {
    width_between(text, 0, text.len(), scale, px)
}

// ── 光标 / 选区 ───────────────────────────

/// 当前选区（字节区间，左闭右开）。没选区返回 `None`。
pub fn selection<F: LineField + ?Sized>(field: &F) -> Option<(usize, usize)> {
    let text = field.text();
    let anchor = clamp_boundary(text, field.anchor());
    let cursor = clamp_boundary(text, field.caret());
    if anchor == cursor {
        None
    } else {
        Some((anchor.min(cursor), anchor.max(cursor)))
    }
}

/// 把光标拉回可视区：跑到左边就整个跟过去，跑到右边就往前吃字。
pub fn ensure_caret_visible<F: LineField + ?Sized>(field: &mut F) {
    let width = field.width();
    if !width.is_finite() || width <= 0.0 {
        return;
    }
    let text = field.text().to_owned();
    let (scale, px) = (field.scale(), field.font_px());
    let cursor = clamp_boundary(&text, field.caret());
    let start = clamp_boundary(&text, field.offset()).min(cursor);
    if cursor < start {
        field.set_offset(cursor);
        return;
    }
    if width_between(&text, start, cursor, scale, px) <= width {
        // 光标本来就在框里，但可能因为删字而滚得太靠右：往前收一收看能否贴回左端。
        field.set_offset(start);
        return;
    }
    let mut start = cursor;
    while let Some(prev) = prev_boundary(&text, start) {
        if width_between(&text, prev, cursor, scale, px) > width {
            break;
        }
        start = prev;
    }
    field.set_offset(start);
}

/// 视口的起点（夹到字符边界上）。
pub fn visible_start<F: LineField + ?Sized>(field: &F) -> usize {
    clamp_boundary(field.text(), field.offset())
}

/// 光标相对内框左沿的横坐标（可能落在框外——滚出去时由绘制层贴边）。
pub fn caret_x<F: LineField + ?Sized>(field: &F) -> f32 {
    let text = field.text();
    let start = visible_start(field);
    let cursor = clamp_boundary(text, field.caret());
    if cursor < start {
        return -width_between(text, cursor, start, field.scale(), field.font_px());
    }
    width_between(text, start, cursor, field.scale(), field.font_px())
}

/// 选区的两个横坐标（相对内框左沿），已裁到可视区。
pub fn selection_x<F: LineField + ?Sized>(field: &F, inner: Rect) -> Option<(f32, f32)> {
    let text = field.text();
    let (lo, hi) = selection(field)?;
    let start = visible_start(field);
    let left = |index: usize| -> f32 {
        let index = clamp_boundary(text, index);
        if index < start {
            0.0
        } else {
            width_between(text, start, index, field.scale(), field.font_px()).min(inner.w)
        }
    };
    let (a, b) = (left(lo), left(hi));
    if (b - a).abs() < 0.5 {
        return None;
    }
    Some((inner.x + a.min(b), inner.x + a.max(b)))
}

/// 点在框里的横坐标换算成字符下标（严格照字符边界走）。
fn index_from_x<F: LineField + ?Sized>(field: &F, inner: Rect, x: f32) -> usize {
    let text = field.text();
    let (scale, px) = (field.scale(), field.font_px());
    let start = visible_start(field);
    let local = (x - inner.x).max(0.0) + width_between(text, 0, start, scale, px);
    let mut index = 0;
    let mut acc = 0.0;
    while let Some(next) = next_boundary(text, index) {
        let w = measure_text(&text[index..next], scale, px);
        if acc + w * 0.5 > local {
            break;
        }
        acc += w;
        index = next;
    }
    index
}

/// 点击定位光标。`extend = true`（拖拽、Shift+点击）时保留锚点，于是结果是一段选区。
pub fn set_caret_from_x<F: LineField + ?Sized>(field: &mut F, inner: Rect, x: f32, extend: bool) {
    field.set_width(inner.w.max(1.0));
    let index = index_from_x(field, inner, x);
    if !extend {
        field.set_anchor(index);
    }
    field.set_caret(index);
    ensure_caret_visible(field);
}

/// 全选。
pub fn select_all<F: LineField + ?Sized>(field: &mut F) {
    field.set_anchor(0);
    let end = field.text().len();
    field.set_caret(end);
    ensure_caret_visible(field);
}

/// 词的分界：空白、以及路径/命令行里常见的分隔符。
///
/// 双击 `C:\work\a.txt` 选中 `work` 而不是整条：整条路径长到看不下，
/// 而用户双击通常是想替换其中一段。
fn is_break(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '/' | '\\'
                | ':'
                | ';'
                | ','
                | '|'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '"'
                | '\''
                | '='
                | '<'
                | '>'
                | '&'
        )
}

/// 双击选词：中文按"连续同类"成词，路径按分隔符切开。
pub fn select_word<F: LineField + ?Sized>(field: &mut F, inner: Rect, x: f32) {
    field.set_width(inner.w.max(1.0));
    let text = field.text().to_owned();
    let index = index_from_x(field, inner, x);
    let char_at =
        |index: usize| -> Option<char> { text.get(index..).and_then(|s| s.chars().next()) };
    // 点在字缝右边（下标已经越过那个字）：往左看一个字，用户点的是它。
    let mut hit = index;
    if hit == text.len() {
        hit = prev_boundary(&text, hit).unwrap_or(0);
    }
    let Some(ch) = char_at(hit) else {
        return;
    };
    let (lo, hi) = if ch.is_whitespace() {
        // 点空白：选中这段空白（跟系统输入框一样）。
        let mut lo = hit;
        while let Some(prev) = prev_boundary(&text, lo) {
            if !char_at(prev).is_some_and(char::is_whitespace) {
                break;
            }
            lo = prev;
        }
        let mut hi = next_boundary(&text, hit).unwrap_or(hit);
        while char_at(hi).is_some_and(char::is_whitespace) {
            hi = next_boundary(&text, hi).unwrap_or(hi);
        }
        (lo, hi)
    } else if is_break(ch) {
        (hit, next_boundary(&text, hit).unwrap_or(hit))
    } else {
        let mut lo = hit;
        while let Some(prev) = prev_boundary(&text, lo) {
            if char_at(prev).is_none_or(is_break) {
                break;
            }
            lo = prev;
        }
        let mut hi = next_boundary(&text, lo).unwrap_or(lo);
        while char_at(hi).is_some_and(|c| !is_break(c)) {
            hi = next_boundary(&text, hi).unwrap_or(hi);
        }
        (lo, hi)
    };
    field.set_anchor(lo);
    field.set_caret(hi);
    ensure_caret_visible(field);
}

/// 光标移动的四种基本动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Home,
    End,
}

/// 移动光标；`extend`（按住 Shift）时保留锚点，于是变成"扩展选区"。
pub fn move_caret<F: LineField + ?Sized>(field: &mut F, motion: Motion, extend: bool) {
    let text = field.text().to_owned();
    let cursor = clamp_boundary(&text, field.caret());
    let anchor = clamp_boundary(&text, field.anchor());
    let target = match motion {
        Motion::Left => prev_boundary(&text, cursor).unwrap_or(0),
        Motion::Right => next_boundary(&text, cursor).unwrap_or(text.len()),
        Motion::Home => 0,
        Motion::End => text.len(),
    };
    if !extend {
        // 有选区时按左/右先"收起"：往选区哪头收，看方向（跟系统输入框一致）。
        if cursor != anchor {
            let (lo, hi) = (cursor.min(anchor), cursor.max(anchor));
            let collapse = match motion {
                Motion::Left => lo,
                Motion::Right => hi,
                Motion::Home => 0,
                Motion::End => text.len(),
            };
            field.set_caret(collapse);
            field.set_anchor(collapse);
            ensure_caret_visible(field);
            return;
        }
        field.set_anchor(target);
    }
    field.set_caret(target);
    ensure_caret_visible(field);
}

// ── 编辑动作 ─────────────────────────────

/// 删掉选区；返回是否真的删了。
pub fn delete_selection<F: LineField + ?Sized>(field: &mut F) -> bool {
    let Some((start, end)) = selection(field) else {
        return false;
    };
    field.text_mut().replace_range(start..end, "");
    field.set_anchor(start);
    field.set_caret(start);
    true
}

/// 在光标处插入（有选区先替换掉）。
pub fn insert<F: LineField + ?Sized>(field: &mut F, text: &str) {
    if text.is_empty() {
        return;
    }
    delete_selection(field);
    let cursor = clamp_boundary(field.text(), field.caret());
    field.text_mut().insert_str(cursor, text);
    let caret = cursor + text.len();
    field.set_caret(caret);
    field.set_anchor(caret);
    ensure_caret_visible(field);
}

/// 退格。返回是否真的删了东西。
pub fn backspace<F: LineField + ?Sized>(field: &mut F) -> bool {
    if delete_selection(field) {
        ensure_caret_visible(field);
        return true;
    }
    let text = field.text().to_owned();
    let cursor = clamp_boundary(&text, field.caret());
    let Some(prev) = prev_boundary(&text, cursor) else {
        return false;
    };
    field.text_mut().replace_range(prev..cursor, "");
    field.set_caret(prev);
    field.set_anchor(prev);
    ensure_caret_visible(field);
    true
}

/// 删除键（往后删）。返回是否真的删了东西。
pub fn delete_forward<F: LineField + ?Sized>(field: &mut F) -> bool {
    if delete_selection(field) {
        ensure_caret_visible(field);
        return true;
    }
    let text = field.text().to_owned();
    let cursor = clamp_boundary(&text, field.caret());
    let Some(next) = next_boundary(&text, cursor) else {
        return false;
    };
    field.text_mut().replace_range(cursor..next, "");
    ensure_caret_visible(field);
    true
}

/// 把多行文本折成单行：回车丢掉、换行/制表符变空格、其它控制字符丢掉。
///
/// 单行框里插进 `\n` 会被画出两行，而内框只裁上下，字会溢出到面板别处。
pub fn flatten_single_line(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\r' => {}
            '\n' | '\t' => cleaned.push(' '),
            c if c.is_control() => {}
            c => cleaned.push(c),
        }
    }
    cleaned
}

/// 粘贴一段（可能多行的）文本。返回是否真的粘进去了。
pub fn paste<F: LineField + ?Sized>(field: &mut F, raw: &str) -> bool {
    let cleaned = flatten_single_line(raw);
    if cleaned.is_empty() {
        return false;
    }
    insert(field, &cleaned);
    true
}

/// 从系统剪贴板粘贴。
pub fn paste_clipboard<F: LineField + ?Sized>(field: &mut F) -> bool {
    match zouwu_platform::clipboard::get_text() {
        Some(text) => paste(field, &text),
        None => false,
    }
}

/// 复制选区（没选区就复制整段）。返回是否真拷到了。
pub fn copy<F: LineField + ?Sized>(field: &F) -> bool {
    let text = field.text();
    let picked = match selection(field) {
        Some((a, b)) => &text[a..b],
        None => text,
    };
    if picked.is_empty() {
        return false;
    }
    zouwu_platform::clipboard::set_text(picked)
}

/// 剪切选区（没选区就什么都不干：整段剪切等于"清空"，太危险）。
pub fn cut<F: LineField + ?Sized>(field: &mut F) -> bool {
    let Some((a, b)) = selection(field) else {
        return false;
    };
    let picked = field.text()[a..b].to_owned();
    if !zouwu_platform::clipboard::set_text(&picked) {
        return false;
    }
    delete_selection(field);
    ensure_caret_visible(field);
    true
}

// ── 水平滚动 ─────────────────────────────

/// 内容宽到放不下（放不下才画滚动条）。
pub fn text_overflow<F: LineField + ?Sized>(field: &F) -> bool {
    let width = field.width();
    width.is_finite()
        && width > 0.0
        && text_width(field.text(), field.scale(), field.font_px()) > width + 0.5
}

/// 单行框自己的水平滚动条（压在内框下沿，只有 5px 高，不抢正文的点击区）。
pub fn scroll_track(inner: Rect) -> Rect {
    Rect::new(inner.x, inner.bottom() - 5.0, inner.w, 5.0)
}

/// 滚动条的 thumb（绘制与命中必须用同一个公式，所以只算一次）。
pub fn thumb<F: LineField + ?Sized>(field: &F, inner: Rect) -> Option<Rect> {
    if !text_overflow(field) {
        return None;
    }
    let (scale, px, width) = (field.scale(), field.font_px(), field.width());
    let text = field.text();
    let total = text_width(text, scale, px);
    if total <= 0.0 {
        return None;
    }
    let ratio = (width / total).clamp(0.0, 1.0);
    // 不能写 `.clamp(24.0, inner.w)`：内框比 24px 窄时 min > max 会直接 panic。
    let thumb_w = (inner.w * ratio).clamp(inner.w.min(24.0), inner.w.max(0.0));
    let scrolled = width_between(text, 0, visible_start(field), scale, px);
    let max_scroll = (total - width).max(1.0);
    let x = inner.x + (inner.w - thumb_w) * (scrolled / max_scroll).clamp(0.0, 1.0);
    Some(Rect::new(x, inner.bottom() - 5.0, thumb_w, 5.0))
}

/// 拖动滚动条：把可视左沿放到鼠标那里。
///
/// **不动光标与选区**：拖滚动条只是"看看别处"，把光标挪过去会当场抹掉用户选好的选区。
pub fn set_offset_from_x<F: LineField + ?Sized>(field: &mut F, inner: Rect, x: f32) {
    if !text_overflow(field) {
        return;
    }
    let (scale, px) = (field.scale(), field.font_px());
    let text = field.text().to_owned();
    let total = text_width(&text, scale, px);
    let max_scroll = (total - field.width()).max(1.0);
    let ratio = ((x - inner.x) / inner.w.max(1.0)).clamp(0.0, 1.0);
    let want = (ratio * max_scroll).clamp(0.0, max_scroll);
    // 只能滚到字缝上：滚到字中间会让字被切一半。
    let mut index = 0;
    let mut acc = 0.0;
    while let Some(next) = next_boundary(&text, index) {
        let w = measure_text(&text[index..next], scale, px);
        if acc + w > want {
            break;
        }
        acc += w;
        index = next;
    }
    field.set_offset(index);
}

/// 鼠标滚轮在输入框上时横着滚（`dx > 0` 往后看）。
pub fn scroll<F: LineField + ?Sized>(field: &mut F, dx: f32) {
    if !text_overflow(field) || dx.abs() < 0.01 {
        return;
    }
    let (scale, px) = (field.scale(), field.font_px());
    let text = field.text().to_owned();
    let mut index = visible_start(field);
    if dx > 0.0 {
        let mut left = dx;
        while left > 0.0 {
            let Some(next) = next_boundary(&text, index) else {
                break;
            };
            left -= measure_text(&text[index..next], scale, px);
            index = next;
        }
    } else {
        let mut left = -dx;
        while left > 0.0 {
            let Some(prev) = prev_boundary(&text, index) else {
                break;
            };
            left -= measure_text(&text[prev..index], scale, px);
            index = prev;
        }
    }
    field.set_offset(index);
}

/// 这个点是不是落在滚动条 thumb 上（窗口层判命中用）。
pub fn scrollbar_at<F: LineField + ?Sized>(field: &F, inner: Rect, p: Point) -> bool {
    thumb(field, inner).is_some_and(|t| Rect::new(t.x, inner.bottom() - 7.0, t.w, 9.0).contains(p))
}

// ── 绘制 ─────────────────────────────────

/// 画单行框的内容：选区底纹 + 文字 + 光标 + 溢出时的滚动条。
///
/// 只画"内容"（内框里），边框/背景由调用方按自己的样式画——设置面板和搜索框
/// 的圆角、底色都不同。
pub fn draw_contents<F: LineField + ?Sized>(
    p: &mut crate::widget::Painter,
    inner: Rect,
    field: &F,
    color: Rgba,
    track: Rgba,
    accent: Rgba,
) {
    let text = field.text();
    let start = visible_start(field);
    if let Some((a, b)) = selection_x(field, inner) {
        p.fill(
            Rect::new(a, inner.y + 3.0, b - a, inner.h - 6.0),
            3.0,
            accent.with_alpha(0.32),
        );
    }
    if start < text.len() {
        let px = field.font_px();
        let shown = p.truncate(&text[start..], px, inner.w - 2.0);
        p.text_in(&shown, inner, px, color, crate::widget::Weight::Regular);
    }
    let caret = caret_x(field);
    let x = (inner.x + caret).clamp(inner.x + 0.5, (inner.right() - 2.0).max(inner.x + 0.5));
    p.line(x, inner.y + 8.0, x, inner.bottom() - 8.0, accent, 1.6);
    if text_overflow(field) {
        p.fill(scroll_track(inner), 2.5, track);
        if let Some(t) = thumb(field, inner) {
            p.fill(t, 2.5, accent.with_alpha(0.75));
        }
    }
}
