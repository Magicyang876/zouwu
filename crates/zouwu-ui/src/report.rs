//! 结果小窗：把一次功能的结果（二维码内容、失败原因）摆出来。
//!
//! 与设置面板同款做法：**正常的 winit 窗口 + softbuffer**。主窗口是分层挂件，
//! 拿不到焦点、按不了键，摆列表天生需要一个真窗口。
//!
//! 内容与绘制分离：`Report` 是纯数据，`layout` 只算矩形，`draw` 只画。
//! 这样"点到的"与"看到的"共用同一份矩形（`row_at` 与 `draw` 都从 `Layout` 取），
//! 并且可以离屏渲染成 PNG 做验收。

use std::sync::Arc;

use tiny_skia::Pixmap;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};
use zouwu_core::geom::Point;
use zouwu_core::theme::{Rgba, Theme};

use crate::presenter::{create_soft, Presenter};
use crate::widget::{Painter, Rect, Weight};

/// 一条结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 二维码里的内容。
    pub text: String,
    /// 在哪块屏幕上找到的（多屏时用户才知道去关哪个屏幕上的码）。
    pub screen: String,
}

/// 一次要展示的结果。
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// 大标题（"识别到 2 个二维码"）。
    pub headline: String,
    /// 副标题：补一句必要的说明（"T 秒内完成" / "有 1 个码读不出来"）。
    pub detail: Option<String>,
    pub entries: Vec<Entry>,
    /// 底部的操作提示。
    pub footer: String,
}

const PAD: f32 = 16.0;
const HEADER_H: f32 = 68.0;
const ROW_H: f32 = 40.0;
const FOOTER_H: f32 = 36.0;
const WIDTH: f32 = 560.0;
/// 最多摆这么多行，其余折成 "+N 个未显示"（窗口高度要跟着内容走，不能无限长）。
const MAX_ROWS: usize = 8;

pub struct Layout {
    pub size: (f32, f32),
    pub header: Rect,
    pub rows: Vec<Rect>,
    pub footer: Rect,
    /// 被折叠掉的行数（`display_overflow` 用）。
    pub hidden: usize,
}

/// 算出整窗的几何。内容越多窗越高 —— 用户不必为了看第 5 条去滚动。
pub fn layout(report: &Report) -> Layout {
    let shown = report.entries.len().min(MAX_ROWS);
    let hidden = report.entries.len().saturating_sub(shown);
    let mut height = HEADER_H + ROW_H * shown as f32 + FOOTER_H + PAD * 0.5;
    if hidden > 0 {
        height += 20.0; // 给"还有 N 条"那行留位置
    }
    let mut rows = Vec::with_capacity(shown);
    for i in 0..shown {
        rows.push(Rect::new(
            PAD,
            HEADER_H + ROW_H * i as f32,
            WIDTH - PAD * 2.0,
            ROW_H,
        ));
    }
    Layout {
        size: (WIDTH, height.max(160.0)),
        header: Rect::new(PAD, 0.0, WIDTH - PAD * 2.0, HEADER_H),
        rows,
        footer: Rect::new(PAD, height - FOOTER_H, WIDTH - PAD * 2.0, FOOTER_H - 4.0),
        hidden,
    }
}

/// 第 i 行的命中区域（点它 = 复制）。
pub fn row_at(layout: &Layout, p: Point) -> Option<usize> {
    layout.rows.iter().position(|r| r.contains(p))
}

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

pub fn draw(pixmap: &mut Pixmap, report: &Report, layout: &Layout, theme: &Theme) {
    let scale = pixmap.width() as f32 / layout.size.0;
    let pal = palette(theme);
    let mut p = Painter::new(pixmap, scale);

    let full = Rect::new(0.0, 0.0, layout.size.0, layout.size.1);
    p.fill(full, 0.0, pal.bg);

    // 标题
    let title = p.truncate(&report.headline, 16.0, layout.header.w);
    p.text_in(
        &title,
        Rect::new(
            layout.header.x,
            layout.header.y + 14.0,
            layout.header.w,
            24.0,
        ),
        16.0,
        pal.text,
        Weight::Bold,
    );
    if let Some(detail) = &report.detail {
        let line = p.truncate(detail, 11.0, layout.header.w);
        p.text_in(
            &line,
            Rect::new(
                layout.header.x,
                layout.header.y + 38.0,
                layout.header.w,
                18.0,
            ),
            11.0,
            pal.muted,
            Weight::Regular,
        );
    }

    // 结果行：一条一个盒子，点哪条复制哪条
    for (i, row) in layout.rows.iter().enumerate() {
        let Some(entry) = report.entries.get(i) else {
            break;
        };
        p.fill(row.inset(0.0, 3.0), 6.0, pal.raised);
        p.stroke(row.inset(0.0, 3.0), 6.0, 1.0, pal.line);

        // 左边一条强调色，暗示"这行可以点"
        p.fill(
            Rect::new(row.x + 8.0, row.y + 10.0, 3.0, ROW_H - 20.0),
            1.5,
            pal.accent,
        );

        let screen_w = p.measure(&entry.screen, 10.0, Weight::Regular);
        let text_w = (row.w - 28.0 - screen_w - 12.0).max(40.0);
        let text = p.truncate(&entry.text, 12.0, text_w);
        p.text_in(
            &text,
            Rect::new(row.x + 20.0, row.y + 5.0, text_w, ROW_H - 10.0),
            12.0,
            pal.text,
            Weight::Regular,
        );
        p.text_right(
            &entry.screen,
            Rect::new(row.x, row.y + 5.0, row.w - 12.0, ROW_H - 10.0),
            10.0,
            pal.muted,
            Weight::Regular,
        );
    }

    if layout.hidden > 0 {
        let y = HEADER_H + ROW_H * layout.rows.len() as f32;
        let more = format!("+{} …", layout.hidden);
        p.text_in(
            &more,
            Rect::new(PAD, y, layout.size.0 - PAD * 2.0, 18.0),
            10.0,
            pal.muted,
            Weight::Regular,
        );
    }

    // 页脚：一条分隔线 + 一行提示
    p.hline(PAD, layout.size.0 - PAD, layout.footer.y, pal.line);
    let footer = p.truncate(&report.footer, 11.0, layout.footer.w);
    p.text_in(
        &footer,
        Rect::new(
            layout.footer.x,
            layout.footer.y + 4.0,
            layout.footer.w,
            layout.footer.h,
        ),
        11.0,
        pal.muted,
        Weight::Regular,
    );
}

/// 窗口壳：事件 → 重绘 / 复制 / 关闭。
pub struct ReportWindow {
    window: Arc<Window>,
    presenter: Box<dyn Presenter>,
    pixmap: Option<Pixmap>,
    pub report: Report,
    theme: Theme,
    layout: Option<Layout>,
    hover: Option<usize>,
    /// 复制成功后把底部的提示换掉，让"复制了"这件事有反馈。
    copied: bool,
    /// 第一帧已经补刷过暗色标题栏。
    frame_dark: bool,
    pub footer_copied: String,
}

/// 窗口对宿主说的话。
pub enum Outcome {
    Close,
}

impl ReportWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        report: Report,
        theme: Theme,
        title: &str,
        footer_copied: String,
    ) -> Result<Self, String> {
        let layout = layout(&report);
        let attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(layout.size.0, layout.size.1))
            .with_min_inner_size(LogicalSize::new(360.0, 160.0))
            .with_resizable(false)
            .with_visible(true);
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("create report window: {e}"))?,
        );
        let physical = window.inner_size();
        let presenter = create_soft(window.clone(), physical.width, physical.height)?;
        crate::frame::apply(&window, &theme);
        crate::frame::life("report", "created", &window);
        window.focus_window();
        let this = Self {
            window,
            presenter,
            pixmap: None,
            report,
            theme,
            layout: Some(layout),
            hover: None,
            copied: false,
            frame_dark: false,
            footer_copied,
        };
        this.request_redraw();
        Ok(this)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub fn close(&self) {
        crate::frame::life("report", "hidden", &self.window);
        self.window.set_visible(false);
    }

    fn rebuild_layout(&mut self) {
        let mut l = layout(&self.report);
        // 窗口大小可能在 Resized 里变过（用户拖不了，但 DPI 变化会改），
        // 布局按窗口实际宽度重算一下，别让内容停在旧尺寸上。
        let size = self.window.inner_size();
        let scale = self.window.scale_factor() as f32;
        if scale > 0.01 {
            let logical_w = size.width as f32 / scale;
            if (logical_w - l.size.0).abs() > 1.0 && logical_w > 200.0 {
                l = layout_for_width(&self.report, logical_w);
            }
        }
        self.layout = Some(l);
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
        self.rebuild_layout();
        let (Some(layout), Some(pixmap)) = (&self.layout, self.pixmap.as_mut()) else {
            return;
        };
        // 复制之后换成"已复制"的提示，一眼可见的反馈胜过日志。
        let mut report = self.report.clone();
        if self.copied {
            report.footer = self.footer_copied.clone();
        }
        draw(pixmap, &report, layout, &self.theme);
        if let Err(err) = self.presenter.present(pixmap) {
            log::error!("report: present failed: {err}");
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
                self.request_redraw();
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
                let hover = self.layout.as_ref().and_then(|l| row_at(l, p));
                if hover != self.hover {
                    self.hover = hover;
                    self.window.set_cursor(if hover.is_some() {
                        CursorIcon::Pointer
                    } else {
                        CursorIcon::Default
                    });
                    self.request_redraw();
                }
                Vec::new()
            }
            WindowEvent::CursorLeft { .. } => {
                if self.hover.take().is_some() {
                    self.request_redraw();
                }
                Vec::new()
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // 点的是哪条，复制哪条；这里不关闭窗口，用户可能想连点两条。
                if let Some(index) = self.hover {
                    log::debug!("report: click on row {index}");
                    self.copy(index);
                }
                Vec::new()
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return Vec::new();
                }
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => vec![Outcome::Close],
                    Key::Named(NamedKey::Enter) => {
                        log::debug!("report: Enter pressed");
                        if !self.report.entries.is_empty() {
                            self.copy(0);
                        }
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn copy(&mut self, index: usize) {
        let Some(entry) = self.report.entries.get(index) else {
            return;
        };
        let text = entry.text.clone();
        if zouwu_platform::clipboard::set_text(&text) {
            self.copied = true;
            log::info!("copied to clipboard: {text}");
        } else {
            // 剪贴板被别的程序占着。别静默失败——用户按了 Ctrl+V 粘贴不出来才知道就晚了。
            log::warn!("clipboard busy; could not copy {text}");
        }
        self.request_redraw();
    }
}

impl Drop for ReportWindow {
    fn drop(&mut self) {
        crate::frame::life("report", "destroyed", &self.window);
    }
}

/// 宽度可变的 `layout`：窗口被 DPI 缩放改了宽度时用。
fn layout_for_width(report: &Report, width: f32) -> Layout {
    let mut l = layout(report);
    let width = width.max(320.0);
    let dw = width - l.size.0;
    l.size.0 = width;
    l.header.w += dw;
    l.footer.w += dw;
    l.footer.y = l.size.1 - FOOTER_H;
    for row in &mut l.rows {
        row.w += dw;
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> Report {
        Report {
            headline: format!("识别到 {n} 个二维码"),
            detail: Some("用时 68 毫秒".to_string()),
            entries: (0..n)
                .map(|i| Entry {
                    text: format!("https://example.com/{i}"),
                    screen: "显示器 1".to_string(),
                })
                .collect(),
            footer: "Enter 复制 · Esc 关闭".to_string(),
        }
    }

    #[test]
    fn rows_never_overlap_the_footer_or_the_header() {
        for n in [1usize, 3, 8] {
            let report = sample(n);
            let l = layout(&report);
            assert!(
                l.rows[0].y >= l.header.bottom() - 0.1,
                "rows start below header"
            );
            assert!(
                l.rows.last().unwrap().bottom() <= l.footer.y + 0.1,
                "rows must end above the footer"
            );
            assert_eq!(l.hidden, 0);
        }
    }

    #[test]
    fn too_many_entries_are_folded_not_clipped() {
        let l = layout(&sample(20));
        assert_eq!(l.rows.len(), MAX_ROWS);
        assert_eq!(l.hidden, 20 - MAX_ROWS);
        // 折叠提示也要在页脚上方
        let more_y = HEADER_H + ROW_H * MAX_ROWS as f32;
        assert!(more_y + 18.0 <= l.footer.y + 0.1);
    }

    #[test]
    fn row_at_matches_the_rows_we_drew() {
        let l = layout(&sample(3));
        for (i, row) in l.rows.iter().enumerate() {
            assert_eq!(row_at(&l, row.center()), Some(i));
        }
        // 页脚与标题区不该命中任何行
        assert_eq!(row_at(&l, Point::new(20.0, 10.0)), None);
        assert_eq!(row_at(&l, Point::new(20.0, l.footer.y + 10.0)), None);
    }

    #[test]
    fn height_grows_with_the_number_of_results() {
        let small = layout(&sample(1)).size.1;
        let big = layout(&sample(4)).size.1;
        assert!(big > small);
    }
}
