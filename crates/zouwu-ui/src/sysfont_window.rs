//! 「系统字体」小窗：把 Windows 自己的界面字体换成你喜欢的（noMeiryoUI 那一套）。
//!
//! 为什么要一个独立窗口而不是设置面板里的第 6 页：
//!
//! - 它改的是 **OS**，不是本程序；设置面板里全是"本程序"的东西，混在一起
//!   很容易让人以为只影响挂件（改坏了要重启资源管理器才知道）。
//! - 列表里动辄几百个字体家族，需要一块能滚动的整屏高度。
//! - 它是**危险操作**：写之前自动拍快照、能一键还原、能导出 `.reg`。
//!   这些按钮摆在一个专门的地方，比塞进设置面板的第 6 页更像话。
//!
//! 与 `report.rs` / `history.rs` 同款做法：正常 winit 窗口 + softbuffer，
//! 内容（`State`）与绘制（`draw`）分离，所以可以离屏渲染成 PNG 做验收
//! （`cargo run --example sysfont_shot`）。

use std::sync::Arc;

use tiny_skia::Pixmap;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};
use zouwu_core::geom::Point;
use zouwu_core::i18n::I18n;
use zouwu_core::theme::{Rgba, Theme};
use zouwu_platform::sysfont::{self, Entry, Snapshot, Target};

use crate::presenter::{create_soft, Presenter};
use crate::widget::{Painter, Rect, Weight};

/// 窗口逻辑宽度（离屏验收的示例也要用）。
pub const WIDTH: f32 = 520.0;
const PAD: f32 = 16.0;
const HEADER_H: f32 = 58.0;
const ROW_H: f32 = 28.0;
/// 列表一次显示这么多行，其余靠滚轮。窗高固定 —— 列表长度是系统决定的（几百个）。
const VISIBLE_ROWS: usize = 10;
const LIST_H: f32 = ROW_H * VISIBLE_ROWS as f32;
const SIZE_H: f32 = 34.0;
const PREVIEW_H: f32 = 84.0;
const BUTTON_H: f32 = 34.0;
const FOOTER_H: f32 = 58.0;
/// 窗口逻辑高度。
pub const HEIGHT: f32 =
    HEADER_H + LIST_H + 8.0 + SIZE_H + PREVIEW_H + BUTTON_H + FOOTER_H + PAD * 1.5;

/// 列表顶部的常客：这几个是中文 Windows 上真正会被拿来当界面字体的。
/// 放在最前面省得用户在一个几百项的字母序列表里找。
const PINNED: [&str; 8] = [
    "Microsoft YaHei UI",
    "Microsoft YaHei",
    "Sarasa Gothic SC",
    "Sarasa UI SC",
    "Source Han Sans SC",
    "Noto Sans CJK SC",
    "Segoe UI",
    "SimSun",
];

/// 字号可选区间：再小看不清，再大菜单会挤爆。
const MIN_PT: f32 = 8.0;
const MAX_PT: f32 = 16.0;

/// 窗口的全部状态。`draw()` 只读它，所以离屏验收不需要 winit。
#[derive(Clone, Debug)]
pub struct State {
    pub faces: Vec<String>,
    pub selected: usize,
    pub scroll: usize,
    /// 字号（pt），`apply()` 时才换算成 `lfHeight`。
    pub pt: f32,
    pub bold: bool,
    /// 系统现在的样子（进窗时读一次）。
    pub snapshot: Option<Snapshot>,
    /// `Segoe UI` 被全局替换成了哪个字体。
    pub substituted: Option<String>,
    pub admin: bool,
    /// 上一次操作的反馈，画在底部。
    pub status: Option<String>,
}

impl State {
    pub fn new() -> Self {
        let faces = sorted_faces(sysfont::installed_faces());
        let snapshot = sysfont::current();
        // 默认停在系统现在用的那个字体上，字号也照抄 —— 打开就能看到"现状"。
        let current_face = snapshot
            .as_ref()
            .and_then(|s| s.get(Target::Menu))
            .map(|e| e.face.clone())
            .unwrap_or_default();
        let selected = faces
            .iter()
            .position(|f| f.eq_ignore_ascii_case(&current_face))
            .unwrap_or(0);
        let pt = snapshot
            .as_ref()
            .and_then(|s| s.get(Target::Menu))
            .map(|e| sysfont::pt_for_height(e.height))
            .filter(|pt| *pt >= MIN_PT && *pt <= MAX_PT)
            .unwrap_or(9.0);
        Self {
            faces,
            selected,
            scroll: 0,
            pt: (pt * 2.0).round() / 2.0,
            bold: false,
            snapshot,
            substituted: sysfont::substitute_face(),
            admin: sysfont::is_admin(),
            status: None,
        }
    }

    pub fn face(&self) -> Option<&str> {
        self.faces.get(self.selected).map(|f| f.as_str())
    }

    /// 要写进系统的五项。
    pub fn entries(&self) -> Vec<(Target, Entry)> {
        let Some(face) = self.face() else {
            return Vec::new();
        };
        let entry = Entry::new(face, sysfont::height_for_pt(self.pt)).bold(self.bold);
        Target::ALL
            .into_iter()
            .map(|t| (t, entry.clone()))
            .collect()
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// 常客在前、其余按名字。`@` 开头的竖排别名 `installed_faces()` 已经滤掉了。
pub fn sorted_faces(mut faces: Vec<String>) -> Vec<String> {
    faces.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    faces.dedup();
    let mut ordered: Vec<String> = Vec::with_capacity(faces.len());
    for pin in PINNED {
        if let Some(i) = faces.iter().position(|f| f.eq_ignore_ascii_case(pin)) {
            ordered.push(faces.remove(i));
        }
    }
    ordered.extend(faces);
    ordered
}

// ---------------------------------------------------------------- 几何

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Face(usize),
    SizeDown,
    SizeUp,
    Bold,
    /// 全局替换：把 `Segoe UI` 换成当前选的字体 / 撤销。
    Substitute,
    Apply,
    Restore,
    Export,
    Close,
}

pub struct Layout {
    pub header: Rect,
    pub list: Rect,
    pub rows: Vec<Rect>,
    pub size_row: Rect,
    pub size_down: Rect,
    pub size_up: Rect,
    pub bold: Rect,
    pub preview: Rect,
    pub substitute: Rect,
    pub buttons: Vec<(Rect, Hit)>,
    pub footer: Rect,
}

/// 只算几何，不看状态 —— 这样"点到的"和"看到的"必然一致。
pub fn layout(scroll: usize, count: usize) -> Layout {
    let width = WIDTH - PAD * 2.0;
    let list = Rect::new(PAD, HEADER_H, width, LIST_H);
    let mut rows = Vec::with_capacity(VISIBLE_ROWS);
    for i in 0..VISIBLE_ROWS {
        let index = scroll + i;
        if index >= count {
            break;
        }
        rows.push(Rect::new(PAD, list.y + ROW_H * i as f32, width, ROW_H));
    }

    let mut y = list.bottom() + 8.0;
    let size_row = Rect::new(PAD, y, width, SIZE_H);
    let size_down = Rect::new(PAD, y + 3.0, 26.0, SIZE_H - 6.0);
    let size_up = Rect::new(PAD + 96.0, y + 3.0, 26.0, SIZE_H - 6.0);
    let bold = Rect::new(PAD + 140.0, y + 3.0, width - 140.0, SIZE_H - 6.0);

    y = size_row.bottom();
    let preview = Rect::new(PAD, y, width, PREVIEW_H);
    y = preview.bottom();
    let substitute = Rect::new(PAD, y, width, BUTTON_H);
    y = substitute.bottom() + 6.0;

    // 四个按钮：应用占一半宽度（它是主操作），其余三等分。
    let gap = 8.0;
    let wide = (width - gap) * 0.5;
    let narrow = (width - gap - wide - gap * 3.0) / 3.0;
    let apply = Rect::new(PAD, y, wide, BUTTON_H);
    let restore = Rect::new(apply.right() + gap, y, narrow, BUTTON_H);
    let export = Rect::new(restore.right() + gap, y, narrow, BUTTON_H);
    let close = Rect::new(export.right() + gap, y, narrow, BUTTON_H);
    y += BUTTON_H;

    Layout {
        header: Rect::new(PAD, 0.0, width, HEADER_H),
        list,
        rows,
        size_row,
        size_down,
        size_up,
        bold,
        preview,
        substitute,
        buttons: vec![
            (apply, Hit::Apply),
            (restore, Hit::Restore),
            (export, Hit::Export),
            (close, Hit::Close),
        ],
        footer: Rect::new(PAD, y, width, HEIGHT - y),
    }
}

pub fn hit(layout: &Layout, scroll: usize, p: Point) -> Option<Hit> {
    if let Some(i) = layout.rows.iter().position(|r| r.contains(p)) {
        return Some(Hit::Face(scroll + i));
    }
    if layout.size_down.contains(p) {
        return Some(Hit::SizeDown);
    }
    if layout.size_up.contains(p) {
        return Some(Hit::SizeUp);
    }
    if layout.bold.contains(p) {
        return Some(Hit::Bold);
    }
    if layout.substitute.contains(p) {
        return Some(Hit::Substitute);
    }
    for (rect, action) in &layout.buttons {
        if rect.contains(p) {
            return Some(*action);
        }
    }
    None
}

/// 一次左键按下应该落在哪个控件上。
///
/// 两个来源，按可信度排：指针位置（`CursorMoved` 存的最后位置，最准）→ `hover`
/// （上一次移动时算的，可能已经过期，也可能还没有——窗口刚显示、鼠标没动过就点的话
/// 它就是 `None`）。只认 `hover` 会让“移进来就点”整个落空。
pub fn click_target(
    pointer: Option<Point>,
    hover: Option<Hit>,
    layout: &Layout,
    scroll: usize,
) -> Option<Hit> {
    pointer.and_then(|p| hit(layout, scroll, p)).or(hover)
}

// ---------------------------------------------------------------- 绘制

#[derive(Clone, Copy)]
struct Palette {
    bg: Rgba,
    raised: Rgba,
    text: Rgba,
    muted: Rgba,
    line: Rgba,
    accent: Rgba,
}

fn palette(theme: &Theme) -> Palette {
    Palette {
        bg: theme.bubble_bg,
        raised: theme.bubble_bg_active,
        text: theme.glyph,
        muted: theme.glyph.with_alpha(0.55),
        line: theme.bubble_edge.with_alpha(0.5),
        accent: theme.accent,
    }
}

/// 画一整窗。`hover` 只影响鼠标下的那一行/按钮，测试时传 `None`。
pub fn draw(pixmap: &mut Pixmap, state: &State, i18n: &I18n, theme: &Theme, hover: Option<Hit>) {
    let layout = layout(state.scroll, state.faces.len());
    let scale = pixmap.width() as f32 / WIDTH;
    let pal = palette(theme);
    let mut p = Painter::new(pixmap, scale);

    let full = Rect::new(0.0, 0.0, WIDTH, HEIGHT);
    p.fill(full, 0.0, pal.bg);

    // 标题 + 现状。两行分开摆：`text_in` 是垂直居中的，共用一个矩形会叠在一起。
    p.text_in(
        &i18n.t("sysfont.title"),
        Rect::new(
            layout.header.x,
            layout.header.y + 6.0,
            layout.header.w,
            22.0,
        ),
        15.0,
        pal.text,
        Weight::Bold,
    );
    let now = match state.snapshot.as_ref().and_then(|s| s.get(Target::Menu)) {
        Some(entry) => format!(
            "{}  ·  {:.1} pt",
            entry.face,
            sysfont::pt_for_height(entry.height)
        ),
        None => i18n.t("sysfont.current.unknown"),
    };
    p.text_in(
        &i18n.t_args("sysfont.current", &[("now", &now)]),
        Rect::new(
            layout.header.x,
            layout.header.y + 30.0,
            layout.header.w,
            18.0,
        ),
        11.0,
        pal.muted,
        Weight::Regular,
    );

    // 字体列表
    p.fill(layout.list, 6.0, pal.raised.with_alpha(0.35));
    let current_face = state
        .snapshot
        .as_ref()
        .and_then(|s| s.get(Target::Menu))
        .map(|e| e.face.to_lowercase())
        .unwrap_or_default();
    for (i, rect) in layout.rows.iter().enumerate() {
        let index = state.scroll + i;
        let Some(face) = state.faces.get(index) else {
            break;
        };
        let selected = index == state.selected;
        if selected {
            p.fill(*rect, 4.0, pal.accent.with_alpha(0.22));
        } else if hover == Some(Hit::Face(index)) {
            p.fill(*rect, 4.0, pal.raised.with_alpha(0.6));
        }
        // 系统现在用的那个家族前面标一个点，一眼看出"我原来用的是它"。
        let mark = if face.to_lowercase() == current_face {
            "● "
        } else {
            "  "
        };
        let text = format!("{mark}{face}");
        let text = p.truncate(&text, 12.0, rect.w - 16.0);
        p.text_in(
            &text,
            Rect::new(rect.x + 8.0, rect.y, rect.w - 16.0, rect.h),
            12.0,
            if selected {
                pal.text
            } else {
                pal.text.with_alpha(0.85)
            },
            if selected {
                Weight::Bold
            } else {
                Weight::Regular
            },
        );
    }
    // 滚动条：列表长的时候必须能看出"还有多少"
    if state.faces.len() > VISIBLE_ROWS {
        let total = state.faces.len() as f32;
        let track_h = LIST_H - 8.0;
        let thumb_h = (track_h * VISIBLE_ROWS as f32 / total).max(24.0);
        let max_scroll = (state.faces.len() - VISIBLE_ROWS) as f32;
        let t = if max_scroll > 0.0 {
            state.scroll as f32 / max_scroll
        } else {
            0.0
        };
        let x = layout.list.right() - 6.0;
        p.fill(
            Rect::new(x, layout.list.y + 4.0, 3.0, track_h),
            1.5,
            pal.line.with_alpha(0.5),
        );
        p.fill(
            Rect::new(
                x,
                layout.list.y + 4.0 + (track_h - thumb_h) * t,
                3.0,
                thumb_h,
            ),
            1.5,
            pal.muted.with_alpha(0.7),
        );
    }

    // 字号：[-] 12 pt [+]，右边一个"加粗"
    let size_rect = layout.size_row;
    p.text_in(
        &i18n.t("sysfont.size"),
        Rect::new(size_rect.x, size_rect.y, 92.0, size_rect.h),
        11.0,
        pal.muted,
        Weight::Regular,
    );
    for (rect, label) in [(layout.size_down, "−"), (layout.size_up, "+")] {
        let hot = hover.is_some()
            && matches!(
                (hover, rect),
                (Some(Hit::SizeDown), r) if r == layout.size_down
            );
        p.fill(
            rect,
            4.0,
            if hot {
                pal.accent.with_alpha(0.35)
            } else {
                pal.raised
            },
        );
        p.text_center(label, rect, 13.0, pal.text, Weight::Bold);
    }
    p.text_center(
        &format!("{:.1} pt", state.pt),
        Rect::new(layout.size_down.right() + 6.0, size_rect.y, 60.0, SIZE_H),
        12.0,
        pal.text,
        Weight::Bold,
    );
    let box_rect = Rect::new(layout.bold.x, layout.bold.y + 4.0, 16.0, 16.0);
    p.fill(
        box_rect,
        3.0,
        if state.bold { pal.accent } else { pal.raised },
    );
    if state.bold {
        p.text_center("✓", box_rect, 12.0, pal.bg, Weight::Bold);
    }
    p.text_in(
        &i18n.t("sysfont.bold"),
        Rect::new(
            layout.bold.x + 22.0,
            layout.bold.y,
            layout.bold.w - 22.0,
            layout.bold.h,
        ),
        12.0,
        pal.text,
        Weight::Regular,
    );

    // 预览：**用选中的字体画**。这是这个窗口唯一真正"预览"的地方，
    // 所以临时切一下家族再切回来（见 `preview_family`）。
    p.fill(layout.preview, 6.0, pal.raised.with_alpha(0.35));
    let sample = i18n.t("sysfont.sample");
    let px = state.pt * 96.0 / 72.0;
    let inside = layout.preview.inset(10.0, 8.0);
    p.set_clip(inside);
    preview_family(state.face(), || {
        let lines: Vec<String> = vec![sample.clone()];
        let mut baseline = inside.y + px;
        for line in lines {
            p.text(
                &line,
                inside.x,
                baseline,
                state.pt,
                pal.text,
                if state.bold {
                    Weight::Bold
                } else {
                    Weight::Regular
                },
            );
            baseline += px * 1.5;
        }
    });
    p.clear_clip();
    if state
        .face()
        .is_some_and(|f| !crate::text::family_covers_cjk(f))
    {
        p.text_in(
            &i18n.t("sysfont.no.cjk"),
            Rect::new(inside.x, inside.bottom() - 16.0, inside.w, 16.0),
            10.0,
            pal.muted,
            Weight::Regular,
        );
    }

    // 全局替换开关
    let sub_text = match &state.substituted {
        Some(face) => i18n.t_args("sysfont.substitute.on", &[("face", face)]),
        None => i18n.t("sysfont.substitute"),
    };
    p.fill(
        layout.substitute,
        4.0,
        if hover == Some(Hit::Substitute) {
            pal.accent.with_alpha(0.28)
        } else {
            pal.raised.with_alpha(0.55)
        },
    );
    p.text_in(
        &sub_text,
        layout.substitute.inset(8.0, 0.0),
        11.0,
        pal.text,
        Weight::Regular,
    );

    // 按钮
    for (rect, action) in &layout.buttons {
        let primary = matches!(action, Hit::Apply);
        let bg = if primary {
            pal.accent
        } else if hover == Some(*action) {
            pal.accent.with_alpha(0.3)
        } else {
            pal.raised
        };
        p.fill(*rect, 5.0, bg);
        let label = match action {
            Hit::Apply => i18n.t("sysfont.apply"),
            Hit::Restore => i18n.t("sysfont.restore"),
            Hit::Export => i18n.t("sysfont.export"),
            Hit::Close => i18n.t("sysfont.close"),
            _ => String::new(),
        };
        let label = p.truncate(&label, 11.0, rect.w - 10.0);
        p.text_center(
            &label,
            *rect,
            11.0,
            if primary { pal.bg } else { pal.text },
            Weight::Bold,
        );
    }

    // 页脚：操作反馈（没有反馈时给一句"图标文字不跟着改"之类的说明）
    p.hline(PAD, WIDTH - PAD, layout.footer.y, pal.line);
    let footer = state
        .status
        .clone()
        .unwrap_or_else(|| i18n.t("sysfont.footer"));
    for (i, line) in wrap(&footer, layout.footer.w, &mut p, 11.0)
        .into_iter()
        .enumerate()
    {
        p.text_in(
            &line,
            Rect::new(
                layout.footer.x,
                layout.footer.y + 6.0 + 15.0 * i as f32,
                layout.footer.w,
                15.0,
            ),
            11.0,
            pal.muted,
            Weight::Regular,
        );
    }
}

/// 页脚只放文件名：完整路径动辄 60 个字符，会把两行的位置吃光（完整路径在日志里）。
fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// 简单按字符数折行：页脚最多两行，够用就不引排版引擎。
fn wrap(text: &str, max_width: f32, p: &mut Painter, size: f32) -> Vec<String> {
    if p.measure(text, size, Weight::Regular) <= max_width {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        let mut probe = current.clone();
        probe.push(ch);
        if p.measure(&probe, size, Weight::Regular) > max_width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            if lines.len() == 2 {
                break;
            }
        }
        current.push(ch);
    }
    if lines.len() < 2 && !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// 临时代入候选字体画一段，画完立刻切回原家族。
///
/// `Painter` 没有"按调用指定家族"的能力（字体家族是进程级的全局状态，见
/// `crates/zouwu-ui/src/text.rs`），所以预览只能这样借一下。代价是每次重绘
/// 会切换两次家族；这个窗口只在事件到来时重绘，实测没有可感的开销。
fn preview_family<R>(face: Option<&str>, f: impl FnOnce() -> R) -> R {
    let previous = crate::text::family();
    if let Some(face) = face {
        if previous.as_deref() != Some(face) {
            crate::text::set_family(Some(face.to_string()));
        }
    }
    let out = f();
    if crate::text::family() != previous {
        crate::text::set_family(previous);
    }
    out
}

// ---------------------------------------------------------------- 窗口

/// 窗口对宿主说的话。
pub enum Outcome {
    Close,
}

pub struct SysFontWindow {
    window: Arc<Window>,
    presenter: Box<dyn Presenter>,
    pixmap: Option<Pixmap>,
    theme: Theme,
    i18n: I18n,
    pub state: State,
    hover: Option<Hit>,
    /// 最后一次看到的指针位置（文档坐标）。点击时用它重新命中一次——只靠 `hover` 的话，
    /// "移进来就点"在 CursorMoved 之前发生的点击会被整个吃掉（Close 按钮没反应就是这么来的）。
    pointer: Option<Point>,
    dragging_scroll: bool,
    frame_dark: bool,
}

impl SysFontWindow {
    /// `language` 是配置里的语言设置（`"auto"` / `"zh-CN"` / `"en-US"`）。
    /// `I18n` 不实现 `Clone`（里面是 `FluentBundle`），所以窗口自己建一份，
    /// 与设置面板同一个做法。
    pub fn new(
        event_loop: &ActiveEventLoop,
        theme: Theme,
        language: &str,
        title: &str,
    ) -> Result<Self, String> {
        let attrs = Window::default_attributes()
            .with_window_icon(crate::icon::app_icon())
            .with_title(title)
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
            .with_min_inner_size(LogicalSize::new(WIDTH, HEIGHT))
            .with_resizable(false)
            .with_visible(false);
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("create sysfont window: {e}"))?,
        );
        let physical = crate::frame::settle(&window, LogicalSize::new(WIDTH, HEIGHT), None);
        let presenter = create_soft(window.clone(), physical.width, physical.height)?;
        crate::frame::apply(&window, &theme);
        crate::frame::life("sysfont", "created", &window);
        let mut this = Self {
            window,
            presenter,
            pixmap: None,
            theme,
            i18n: I18n::new(language),
            state: State::new(),
            hover: None,
            pointer: None,
            dragging_scroll: false,
            frame_dark: false,
        };
        log::info!(
            "sysfont: window opened ({} faces, admin {}, substituted {:?})",
            this.state.faces.len(),
            this.state.admin,
            this.state.substituted
        );
        crate::frame::show(&this.window);
        this.redraw();
        this.request_redraw();
        Ok(this)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible().unwrap_or(true)
    }

    pub fn close(&self) {
        crate::frame::life("sysfont", "hidden", &self.window);
        self.window.set_visible(false);
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        crate::frame::apply(&self.window, &self.theme);
        self.request_redraw();
    }

    pub fn show(&mut self) {
        // 每次打开都重新读一遍系统状态：用户可能刚在别处改过。
        self.state = State::new();
        self.window.set_visible(true);
        self.window.focus_window();
        self.request_redraw();
    }

    fn max_scroll(&self) -> usize {
        self.state.faces.len().saturating_sub(VISIBLE_ROWS)
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
        draw(pixmap, &self.state, &self.i18n, &self.theme, self.hover);
        if let Err(err) = self.presenter.present(pixmap) {
            log::error!("sysfont: present failed: {err}");
        }
    }

    /// 点一下某个东西之后做的事。抽出来是因为测试要能直接调它。
    ///
    /// 返回要交给主循环的 `Outcome`：只有"关闭"需要往外传，其余都是就地处理。
    fn activate(&mut self, hit: Hit) -> Vec<Outcome> {
        match hit {
            Hit::Face(index) => {
                self.state.selected = index.min(self.state.faces.len().saturating_sub(1));
                if let Some(face) = self.state.face() {
                    // 选中就问一次"覆盖不覆盖中文"，缺字的字体要提前警告。
                    if !crate::text::family_covers_cjk(face) {
                        log::warn!("sysfont: {face} has no CJK glyphs");
                    }
                }
            }
            Hit::SizeDown => self.state.pt = (self.state.pt - 0.5).max(MIN_PT),
            Hit::SizeUp => self.state.pt = (self.state.pt + 0.5).min(MAX_PT),
            Hit::Bold => self.state.bold = !self.state.bold,
            Hit::Apply => self.apply(),
            Hit::Restore => self.restore(),
            Hit::Export => self.export(),
            Hit::Substitute => self.toggle_substitute(),
            Hit::Close => {
                // 隐藏窗口 + 告诉主循环（它负责复位拖动/轮询状态）。
                self.close();
                return vec![Outcome::Close];
            }
        }
        Vec::new()
    }

    fn apply(&mut self) {
        // 边框/滚动条被旧版本放大过的话先顺手修：SPI 对这几项不作为，只能写注册表，
        // 而且要重新登录才生效，所以下面会把提示语换掉。
        let needs_repair = sysfont::chrome_needs_repair();
        if needs_repair {
            match sysfont::restore_chrome() {
                Ok(chrome) => log::info!("sysfont: chrome restored to {chrome:?}"),
                Err(err) => log::warn!("sysfont: cannot restore chrome: {err}"),
            }
        }
        let entries = self.state.entries();
        if entries.is_empty() {
            self.state.status = Some(self.i18n.t("sysfont.no.selection"));
            return;
        }
        // 先拍照再动手：快照失败就直接不写系统了。
        let Some(before) = sysfont::current() else {
            self.state.status = Some(self.i18n.t("sysfont.cannot.read"));
            return;
        };
        let Some(path) = sysfont::save_snapshot(&before) else {
            self.state.status = Some(self.i18n.t("sysfont.cannot.backup"));
            log::error!("sysfont: refusing to apply without a snapshot");
            return;
        };
        match sysfont::apply(&entries) {
            Ok(()) => {
                let face = self.state.face().unwrap_or_default().to_string();
                log::info!(
                    "sysfont: applied {face} {:.1}pt bold={} (snapshot {})",
                    self.state.pt,
                    self.state.bold,
                    path.display()
                );
                self.state.snapshot = sysfont::current();
                // 尺寸修复要重新登录才生效，这时候把“已应用”换成一句带前提的话。
                let repaired = needs_repair;
                self.state.status = Some(if repaired {
                    self.i18n.t("sysfont.chrome.repaired")
                } else {
                    self.i18n.t_args(
                        "sysfont.applied",
                        &[("face", &face), ("path", &file_name(&path))],
                    )
                });
            }
            Err(err) => {
                log::error!("sysfont: apply failed: {err}");
                self.state.status =
                    Some(self.i18n.t_args("sysfont.apply.failed", &[("why", &err)]));
            }
        }
    }

    fn restore(&mut self) {
        let Some((path, snapshot)) = sysfont::latest_snapshot() else {
            self.state.status = Some(self.i18n.t("sysfont.no.backup"));
            return;
        };
        match sysfont::restore(&snapshot) {
            Ok(()) => {
                log::info!("sysfont: restored {}", path.display());
                self.state.snapshot = sysfont::current();
                self.state.status = Some(
                    self.i18n
                        .t_args("sysfont.restored", &[("path", &file_name(&path))]),
                );
            }
            Err(err) => {
                log::error!("sysfont: restore failed: {err}");
                self.state.status =
                    Some(self.i18n.t_args("sysfont.restore.failed", &[("why", &err)]));
            }
        }
    }

    fn export(&mut self) {
        let entries = self.state.entries();
        let reg = sysfont::to_reg(&entries);
        let dir = sysfont::snapshot_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            self.state.status = Some(self.i18n.t("sysfont.cannot.backup"));
            return;
        }
        let path = dir.join("zouwu-sysfont.reg");
        match std::fs::write(&path, reg) {
            Ok(()) => {
                log::info!("sysfont: reg -> {}", path.display());
                self.state.status = Some(
                    self.i18n
                        .t_args("sysfont.exported", &[("path", &file_name(&path))]),
                );
            }
            Err(err) => {
                log::error!("sysfont: cannot write reg: {err}");
                self.state.status = Some(
                    self.i18n
                        .t_args("sysfont.export.failed", &[("why", &err.to_string())]),
                );
            }
        }
    }

    fn toggle_substitute(&mut self) {
        let next = match self.state.substituted.clone() {
            Some(_) => None,
            None => self.state.face().map(str::to_string),
        };
        match sysfont::set_substitute(next.as_deref()) {
            Ok(()) => {
                log::info!("sysfont: Segoe UI -> {next:?}");
                self.state.substituted = sysfont::substitute_face();
                self.state.status = Some(match &next {
                    Some(face) => self
                        .i18n
                        .t_args("sysfont.substitute.set", &[("face", face)]),
                    None => self.i18n.t("sysfont.substitute.cleared"),
                });
            }
            Err(err) => {
                log::error!("sysfont: substitute failed: {err}");
                let key = if err.contains("administrator") {
                    "sysfont.needs.admin.long"
                } else {
                    "sysfont.substitute.failed"
                };
                self.state.status = Some(if key == "sysfont.needs.admin.long" {
                    self.i18n.t(key)
                } else {
                    self.i18n.t_args(key, &[("why", &err)])
                });
            }
        }
    }

    pub fn window_event(&mut self, event: &WindowEvent) -> Vec<Outcome> {
        match event {
            WindowEvent::RedrawRequested => {
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
                self.request_redraw();
                Vec::new()
            }
            WindowEvent::CloseRequested => {
                self.close();
                vec![Outcome::Close]
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.window.scale_factor() as f32;
                let p = Point::new(
                    position.x as f32 / scale.max(0.01),
                    position.y as f32 / scale.max(0.01),
                );
                self.pointer = Some(p);
                let layout = layout(self.state.scroll, self.state.faces.len());
                let next = hit(&layout, self.state.scroll, p);
                let cursor = if next.is_some() {
                    CursorIcon::Pointer
                } else {
                    CursorIcon::Default
                };
                self.window.set_cursor(cursor);
                if next != self.hover {
                    self.hover = next;
                    self.request_redraw();
                }
                Vec::new()
            }
            WindowEvent::CursorLeft { .. } => {
                if self.hover.is_some() {
                    self.hover = None;
                    self.request_redraw();
                }
                Vec::new()
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if *button != MouseButton::Left || *state != ElementState::Pressed {
                    self.dragging_scroll = false;
                    return Vec::new();
                }
                // 先用指针的实际位置重算一次（`hover` 可能是上一次移动时的旧值，
                // 也可能还没有——窗口刚显示、鼠标没动过就点的话它是 `None`）。
                let layout = layout(self.state.scroll, self.state.faces.len());
                let Some(hit) = click_target(self.pointer, self.hover, &layout, self.state.scroll)
                else {
                    return Vec::new();
                };
                let outcomes = self.activate(hit);
                self.request_redraw();
                outcomes
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    // 触控板给的是像素，40 像素当一行。
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                // 滚轮向上 = 内容向上走 = 起点变小（与剪贴板历史一致）。
                let step = steps.round() as i32;
                let next = self.state.scroll as i32 - step;
                self.state.scroll = next.clamp(0, self.max_scroll() as i32) as usize;
                self.request_redraw();
                Vec::new()
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return Vec::new();
                }
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        self.close();
                        vec![Outcome::Close]
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        let max = self.state.faces.len().saturating_sub(1);
                        self.state.selected = (self.state.selected + 1).min(max);
                        self.ensure_visible();
                        self.request_redraw();
                        Vec::new()
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        self.state.selected = self.state.selected.saturating_sub(1);
                        self.ensure_visible();
                        self.request_redraw();
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    /// 键盘移动选中行之后，把它滚进视野。
    fn ensure_visible(&mut self) {
        if self.state.selected < self.state.scroll {
            self.state.scroll = self.state.selected;
        } else if self.state.selected >= self.state.scroll + VISIBLE_ROWS {
            self.state.scroll = self.state.selected + 1 - VISIBLE_ROWS;
        }
        self.state.scroll = self.state.scroll.min(self.max_scroll());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(faces: &[&str]) -> State {
        State {
            faces: faces.iter().map(|f| f.to_string()).collect(),
            selected: 0,
            scroll: 0,
            pt: 9.0,
            bold: false,
            snapshot: None,
            substituted: None,
            admin: false,
            status: None,
        }
    }

    #[test]
    fn pinned_faces_come_first_and_duplicates_go_away() {
        let faces = sorted_faces(vec![
            "zzz".into(),
            "Sarasa Gothic SC".into(),
            "Segoe UI".into(),
            "Segoe UI".into(),
            "Microsoft YaHei UI".into(),
            "aaa".into(),
        ]);
        assert_eq!(faces[0], "Microsoft YaHei UI");
        assert_eq!(faces[1], "Sarasa Gothic SC");
        assert_eq!(faces[2], "Segoe UI");
        // 其余按名字排在后面，重复项已经去掉
        assert_eq!(&faces[3..], &["aaa".to_string(), "zzz".to_string()]);
    }

    #[test]
    fn entries_cover_every_target_with_the_same_face() {
        let state = state_with(&["Sarasa Gothic SC"]);
        let entries = state.entries();
        assert_eq!(entries.len(), Target::ALL.len());
        for (target, entry) in entries {
            assert!(Target::ALL.contains(&target));
            assert_eq!(entry.face, "Sarasa Gothic SC");
            assert!(entry.height < 0);
        }
    }

    #[test]
    fn bold_reaches_the_entry() {
        let mut state = state_with(&["Segoe UI"]);
        state.bold = true;
        assert_eq!(state.entries()[0].1.weight, 700);
    }

    #[test]
    fn click_on_a_row_selects_that_row_even_when_scrolled() {
        let faces: Vec<String> = (0..40).map(|i| format!("font-{i:02}")).collect();
        let state = State {
            faces,
            selected: 0,
            scroll: 25,
            ..state_with(&[])
        };
        let layout = layout(state.scroll, state.faces.len());
        // 列表第 3 行 = 25 + 3 = 28
        let rect = layout.rows[3];
        let hit = hit(
            &layout,
            state.scroll,
            Point::new(rect.x + 10.0, rect.y + rect.h * 0.5),
        );
        assert_eq!(hit, Some(Hit::Face(28)));
    }

    #[test]
    fn hit_testing_agrees_with_the_buttons_we_drew() {
        let layout = layout(0, 10);
        for (rect, action) in &layout.buttons {
            let p = Point::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
            assert_eq!(hit(&layout, 0, p), Some(*action));
        }
        // 每行的中心点必须命中它自己
        for (i, rect) in layout.rows.iter().enumerate() {
            let p = Point::new(rect.x + 4.0, rect.y + rect.h * 0.5);
            assert_eq!(hit(&layout, 0, p), Some(Hit::Face(i)));
        }
    }

    #[test]
    fn layout_fits_inside_the_window() {
        let layout = layout(0, 10);
        for rect in &layout.rows {
            assert!(rect.bottom() <= HEIGHT);
        }
        for (rect, _) in &layout.buttons {
            assert!(rect.right() <= WIDTH, "{:?} overflows", rect);
        }
        assert!(layout.footer.bottom() <= HEIGHT);
    }

    #[test]
    fn rows_stop_at_the_end_of_the_list() {
        // 只剩 3 个字体时不该画出 10 个可点区域
        let layout = layout(0, 3);
        assert_eq!(layout.rows.len(), 3);
    }

    #[test]
    fn scrolling_never_selects_a_row_outside_the_list() {
        let layout = layout(0, 3);
        let below = Point::new(layout.list.x + 4.0, layout.list.y + ROW_H * 5.0);
        assert_eq!(hit(&layout, 0, below), None);
    }

    #[test]
    fn the_close_button_is_hittable() {
        // 回归：`Close` 曾经在 `activate` 里是空实现，而且点击只认 `hover`——
        // 窗口刚出来、鼠标没动过（`hover` 为 `None`）时点关闭按钮什么都不发生。
        let layout = layout(0, 10);
        let close = layout
            .buttons
            .iter()
            .find(|(_, action)| *action == Hit::Close)
            .map(|(rect, _)| rect.center())
            .expect("关闭按钮要在按钮表里");
        assert_eq!(hit(&layout, 0, close), Some(Hit::Close));
        // 指针位置在就是它说了算，`hover` 没有也能点中
        assert_eq!(
            click_target(Some(close), None, &layout, 0),
            Some(Hit::Close)
        );
        // 指针位置与 `hover` 冲突时以指针为准（`hover` 可能是上一帧的旧值）
        assert_eq!(
            click_target(Some(close), Some(Hit::Apply), &layout, 0),
            Some(Hit::Close)
        );
        // 指针落在空白处时回退到 `hover`
        let blank = layout.header.center();
        assert_eq!(hit(&layout, 0, blank), None);
        assert_eq!(
            click_target(Some(blank), Some(Hit::Apply), &layout, 0),
            Some(Hit::Apply)
        );
    }
}
