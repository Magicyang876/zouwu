//! 右键呼出的形象列表：点一下换个形象。
//!
//! 为什么是一个**真窗口**而不是画在挂件里：挂件是 340 见方的分层窗口，菜单要能
//! 伸出它、要能被点、要在多屏上贴边，这些在一个分层窗口里全要自己重写一遍
//! （几何、命中、滚轮…）。而"列表窗口"这套设置面板/历史窗已经有了，直接复用。
//!
//! 为什么不用 Win32 的 `TrackPopupMenu`：它跑的是自己的**模态消息循环**，会把
//! winit 的事件循环卡在它里面——ADR-037 的教训是主线程上一卡，整只小兽就像死了一样。
//!
//! 内容与绘制分离（同 `report.rs`）：`layout()` 只算矩形，`draw()` 只画，
//! `row_at()` 与 `draw()` 共用同一份 `Layout`，点到的位置与看到的位置不可能错开。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};
use zouwu_core::geom::Point;
use zouwu_core::theme::Theme;

use crate::presenter::{create_soft, Presenter};
use crate::widget::{Painter, Rect, Weight};

pub const WIDTH: f32 = 216.0;
pub const HEADER_H: f32 = 30.0;
pub const ROW_H: f32 = 38.0;
const PAD: f32 = 8.0;
const MAX_ROWS: usize = 12;
/// 建窗后忽略失焦事件的时间窗（见 opened_at）。
const FOCUS_GRACE: Duration = Duration::from_millis(400);

/// 一份几何：整窗、标题、每一行。
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub size: (f32, f32),
    pub header: Rect,
    pub rows: Vec<Rect>,
}

pub fn layout(count: usize) -> Layout {
    let rows = count.clamp(1, MAX_ROWS);
    let size = (WIDTH, HEADER_H + rows as f32 * ROW_H + PAD * 2.0 + 4.0);
    let header = Rect::new(PAD, PAD, WIDTH - PAD * 2.0, HEADER_H - PAD);
    let mut row_rects = Vec::with_capacity(count);
    for i in 0..count {
        row_rects.push(Rect::new(
            PAD,
            HEADER_H + PAD + i as f32 * ROW_H,
            WIDTH - PAD * 2.0,
            ROW_H,
        ));
    }
    Layout {
        size,
        header,
        rows: row_rects,
    }
}

/// 指针落在哪一行。
pub fn row_at(layout: &Layout, p: Point) -> Option<usize> {
    layout.rows.iter().position(|r| r.contains(p))
}

/// 菜单对宿主说的话。
pub enum Outcome {
    Close,
    /// 选了某个形象（皮肤 id）。
    Pick(String),
}

pub struct SkinMenuWindow {
    window: Arc<Window>,
    presenter: Box<dyn Presenter>,
    pixmap: Option<Pixmap>,
    theme: Theme,
    layout: Layout,
    /// `(皮肤 id, 显示名)`，顺序就是加载顺序（内置在前）。
    choices: Vec<(String, String)>,
    current: String,
    hover: Option<usize>,
    title: String,
    cursor: Point,
    frame_dark: bool,
    /// 建窗时刻。刚建好的那一小会儿会收到一个 Focused(false)：窗口还没被
    /// 激活，只是创建即失焦的系统事件。不设宽限期的话，菜单会一闪即隐。
    opened_at: Instant,
}

impl SkinMenuWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        choices: Vec<(String, String)>,
        current: String,
        theme: Theme,
        title: String,
        at: (i32, i32),
        pad: f32,
    ) -> Result<Self, String> {
        let layout = layout(choices.len());
        let scale = event_loop
            .primary_monitor()
            .map(|m| m.scale_factor())
            .unwrap_or(1.0);
        let (x, y) = place(event_loop, layout.size, at, scale, pad);
        let attrs = Window::default_attributes()
            .with_title(&title)
            .with_window_icon(crate::icon::app_icon())
            .with_inner_size(LogicalSize::new(layout.size.0, layout.size.1))
            .with_position(PhysicalPosition::new(x, y))
            .with_decorations(false)
            .with_resizable(false)
            // 小兽是分层**置顶**窗口：菜单不置顶就会被压在它下面（实测菜单左上角
            // 被小兽的耳朵盖住）。菜单比小兽晚建，同层级里就在上面。
            .with_window_level(WindowLevel::AlwaysOnTop)
            // **不要**在这里 visible：建窗属性里的尺寸/位置是延后应用的，
            // 那一刻窗口会以系统默认尺寸（实测 1440x847）先闪一帧出来。
            // 见 `frame::settle` / `frame::show`。
            .with_visible(false);
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("create skin menu: {e}"))?,
        );
        // 窗口此刻还不可见：把位置与尺寸补到位，`inner_size()` 已经是最终值。
        let physical = crate::frame::settle(
            &window,
            LogicalSize::new(layout.size.0, layout.size.1),
            Some(PhysicalPosition::new(x, y)),
        );
        let presenter = create_soft(window.clone(), physical.width, physical.height)?;
        crate::frame::apply(&window, &theme);
        crate::frame::life("skin-menu", "created", &window);
        let mut this = Self {
            window,
            presenter,
            pixmap: None,
            theme,
            layout,
            choices,
            current,
            hover: None,
            title,
            cursor: Point::new(0.0, 0.0),
            frame_dark: false,
            opened_at: Instant::now(),
        };
        // 先上屏再画：窗口隐藏时 `BitBlt` 的像素会被丢掉（历史窗实测开出来是
        // 一片空底），而且 show 之后系统还会排一次 `WM_PAINT`。这个窗口又收不到
        // `RedrawRequested`，所以上屏后必须**立刻同步画**，不能挂 `request_redraw`。
        crate::frame::show(&this.window);
        this.repaint();
        Ok(this)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// 立刻画一帧，不走 `RedrawRequested`。
    ///
    /// 实测这个窗口**收不到** `RedrawRequested`（同样的写法在设置/历史/结果窗上都正常）：
    /// 表现是菜单开出来之后是一整块纯色底，一个字都没有。菜单只有几行字、画一次不到 1ms，
    /// 索性所有状态变化都同步画，不依赖事件循环的时序。
    fn repaint(&mut self) {
        if let Err(err) = self.redraw() {
            log::warn!("skin menu paint failed: {err}");
        }
    }

    pub fn close(&self) {
        crate::frame::life("skin-menu", "hidden", &self.window);
        self.window.set_visible(false);
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.frame_dark = false;
        self.pixmap = None;
        self.repaint();
    }

    /// 换一份列表（导入/删除皮肤之后）。
    pub fn reset(&mut self, choices: Vec<(String, String)>, current: String) {
        self.layout = layout(choices.len());
        self.choices = choices;
        self.current = current;
        self.hover = None;
        self.pixmap = None;
        let _ = self
            .window
            .request_inner_size(LogicalSize::new(self.layout.size.0, self.layout.size.1));
        self.repaint();
    }

    pub fn window_event(&mut self, event: &WindowEvent) -> Vec<Outcome> {
        log::trace!("skin menu event: {event:?}");
        let scale = self.window.scale_factor() as f32;
        match event {
            WindowEvent::CloseRequested => return vec![Outcome::Close],
            // 窗口尺寸变了（首次显示、缩放、DPI 变化）必须把 softbuffer 的
            // surface 一起 resize：否则 present() 的参数与窗口对不上，
            // 表现是**整块窗口一片纯色**——菜单里一个字都看不到。
            WindowEvent::Resized(size) => {
                crate::frame::apply(&self.window, &self.theme);
                self.presenter.resize(size.width.max(1), size.height.max(1));
                self.pixmap = None;
                self.repaint();
                return Vec::new();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Point::new(position.x as f32 / scale, position.y as f32 / scale);
                let hover = row_at(&self.layout, self.cursor);
                if hover != self.hover {
                    self.hover = hover;
                    self.repaint();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if self.hover.take().is_some() {
                    self.repaint();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                match row_at(&self.layout, self.cursor) {
                    Some(row) => {
                        if let Some((id, _)) = self.choices.get(row) {
                            log::info!("skin menu: pick `{id}`");
                            return vec![Outcome::Pick(id.clone()), Outcome::Close];
                        }
                    }
                    // 点在菜单的空白处：当成"算了"。
                    None => return vec![Outcome::Close],
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && matches!(event.logical_key, Key::Named(NamedKey::Escape))
                {
                    return vec![Outcome::Close];
                }
            }
            // 点到别的窗口就收起：这是菜单，不是常驻面板。
            // 但建窗后的头 400ms 会来一个创建即失焦，那一下不算。
            WindowEvent::Focused(false) => {
                if self.opened_at.elapsed() > FOCUS_GRACE {
                    return vec![Outcome::Close];
                }
            }
            // 理论上用不到（内容变化都已经同步画过），留着兜底。
            WindowEvent::RedrawRequested => self.repaint(),
            _ => {}
        }
        Vec::new()
    }

    fn redraw(&mut self) -> Result<(), String> {
        let scale = self.window.scale_factor() as f32;
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let mut pixmap = match self.pixmap.take() {
            Some(p) if p.width() == size.width && p.height() == size.height => p,
            _ => {
                // 尺寸变了：surface 也要跟上，别让 present() 拿旧尺寸凑合。
                self.presenter.resize(size.width, size.height);
                Pixmap::new(size.width, size.height).ok_or("pixmap")?
            }
        };
        draw(
            &mut pixmap,
            &self.layout,
            &self.theme,
            &View {
                choices: &self.choices,
                current: &self.current,
                hover: self.hover,
                title: &self.title,
            },
            scale,
        );
        if let Err(err) = self.presenter.present(&pixmap) {
            log::warn!("skin menu present failed: {err}");
        }
        if !self.frame_dark {
            crate::frame::apply(&self.window, &self.theme);
            self.frame_dark = true;
        }
        self.pixmap = Some(pixmap);
        Ok(())
    }
}

/// 只读的一份视图，供 `draw` 使用（避免在画的时候借整个结构）。
pub struct View<'a> {
    pub choices: &'a [(String, String)],
    pub current: &'a str,
    pub hover: Option<usize>,
    pub title: &'a str,
}

/// 画菜单。`scale` 是物理/逻辑比。
pub fn draw(pixmap: &mut Pixmap, layout: &Layout, theme: &Theme, view: &View<'_>, scale: f32) {
    let mut p = Painter::new(pixmap, scale);
    let full = Rect::new(0.0, 0.0, layout.size.0, layout.size.1);
    // 面板：与气泡同色系，圆角比气泡小一点（菜单是"板"不是"球"）。
    p.fill(full.inset(1.0, 1.0), 9.0, theme.bubble_bg);
    p.stroke(full.inset(1.0, 1.0), 9.0, 1.0, theme.bubble_edge);

    p.text_in(view.title, layout.header, 11.5, theme.glyph, Weight::Bold);

    for (i, row) in layout.rows.iter().enumerate() {
        let Some((id, name)) = view.choices.get(i) else {
            continue;
        };
        let current = id == view.current;
        // 悬停与"当前正在用"必须一眼分开：悬停只是底色变亮，当前那一项额外加
        // 一圈强调色描边 + 左侧圆点 + 亮字。两者都用底色的话，鼠标一划过就分不清
        // 哪个是"已经选了"、哪个是"点了会发生什么"。
        if view.hover == Some(i) {
            // 两层：先铺 `bubble_bg_active`，再压一层很淡的强调色。
            // 只用主题里的 active 底色时，和面板底只差十几个灰阶，扫一眼看不出来。
            p.fill(row.inset(0.0, 0.0), 6.0, theme.bubble_bg_active);
            p.fill(row.inset(0.0, 0.0), 6.0, theme.accent.with_alpha(0.10));
        }
        if current {
            p.stroke(row.inset(0.5, 0.5), 6.0, 1.2, theme.accent);
        }
        let color = if current {
            theme.glyph_active
        } else {
            theme.glyph
        };
        let label = p.truncate_left(name, 12.5, row.w - 34.0);
        p.text_in(
            &label,
            Rect::new(row.x + 24.0, row.y, row.w - 34.0, row.h),
            12.5,
            color,
            if current {
                Weight::Bold
            } else {
                Weight::Regular
            },
        );
        // 当前那一项前面点一个实心圆：比打勾更像"选中"，也不用备一套勾的图。
        if current {
            p.circle(
                Point::new(row.x + 12.0, row.y + row.h * 0.5),
                3.2,
                theme.accent,
            );
        }
    }
}

/// 把菜单摆到光标旁边，并夹进屏幕里。
///
/// 优先放到光标右下（鼠标惯用方向），右边/下边放不下就翻到左上。
fn place(
    event_loop: &ActiveEventLoop,
    size: (f32, f32),
    at: (i32, i32),
    scale: f64,
    pad: f32,
) -> (i32, i32) {
    let (w, h) = (
        (size.0 * scale as f32) as i32,
        (size.1 * scale as f32) as i32,
    );
    // `pad` 是小兽的视觉半径：菜单不能从兽脸上长出来。
    let gap = (8.0 * scale) as i32 + pad.max(0.0) as i32;
    let mut candidate: Option<(i32, i32, i32, i32)> = None;
    let mut monitors: Vec<_> = event_loop.available_monitors().collect();
    // 光标所在的那块屏优先。
    monitors.sort_by_key(|m| {
        let pos = m.position();
        let size = m.size();
        let inside = at.0 >= pos.x
            && at.0 < pos.x + size.width as i32
            && at.1 >= pos.y
            && at.1 < pos.y + size.height as i32;
        if inside {
            0
        } else {
            1
        }
    });
    for m in &monitors {
        let pos = m.position();
        let size = m.size();
        candidate = Some((pos.x, pos.y, size.width as i32, size.height as i32));
        break;
    }
    let Some((mx, my, mw, mh)) = candidate else {
        return (at.0 + gap, at.1 + gap);
    };

    let mut x = at.0 + gap;
    let mut y = at.1 + gap;
    if x + w > mx + mw {
        x = at.0 - gap - w;
    }
    if y + h > my + mh {
        y = at.1 - gap - h;
    }
    (
        x.clamp(mx, (mx + mw - w).max(mx)),
        y.clamp(my, (my + mh - h).max(my)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_stack_under_the_header_without_gaps() {
        let l = layout(3);
        assert_eq!(l.rows.len(), 3);
        for pair in l.rows.windows(2) {
            assert_eq!(pair[0].bottom(), pair[1].y, "行与行之间不该有空隙");
        }
        assert!(l.rows[0].y >= l.header.bottom(), "第一行要在标题下面");
        assert!(l.rows[2].bottom() <= l.size.1, "最后一行要在窗口里");
    }

    #[test]
    fn the_window_grows_with_the_number_of_skins() {
        assert!(layout(4).size.1 > layout(2).size.1);
        assert_eq!(layout(1).size.0, layout(9).size.0, "宽度不随数量变");
    }

    #[test]
    fn hit_testing_matches_the_drawn_rows() {
        let l = layout(3);
        for (i, row) in l.rows.iter().enumerate() {
            assert_eq!(row_at(&l, row.center()), Some(i));
        }
        // 标题行与面板边缘都不是行
        assert_eq!(row_at(&l, l.header.center()), None);
        assert_eq!(row_at(&l, Point::new(2.0, 2.0)), None);
        assert_eq!(row_at(&l, Point::new(WIDTH - 2.0, l.size.1 - 2.0)), None);
    }

    #[test]
    fn an_empty_menu_still_has_a_sane_size() {
        let l = layout(0);
        assert!(l.size.1 > HEADER_H);
        assert!(l.rows.is_empty());
    }

    #[test]
    fn the_menu_paints_every_row() {
        let l = layout(3);
        let theme = Theme::builtin("zouwu-warm").expect("builtin theme");
        let choices = vec![
            ("procedural".to_string(), "驺吾 · 代码绘制".to_string()),
            ("zouwu-ink".to_string(), "水墨驺吾".to_string()),
            ("zouwu-paper".to_string(), "折纸驺吾".to_string()),
        ];
        let scale = 1.0;
        let mut pm =
            Pixmap::new((l.size.0 * scale) as u32, (l.size.1 * scale) as u32).expect("pixmap");
        draw(
            &mut pm,
            &l,
            &theme,
            &View {
                choices: &choices,
                current: "zouwu-ink",
                hover: Some(1),
                title: "形象",
            },
            scale,
        );
        // 三行都该有字：行内非背景色的像素数要够（空图/漏画会直接掉到 0）。
        for (i, row) in l.rows.iter().enumerate() {
            let mut ink = 0;
            for y in row.y as u32..row.bottom() as u32 {
                for x in row.x as u32..row.right() as u32 {
                    let idx = ((y * pm.width() + x) as usize) * 4;
                    let px = &pm.data()[idx..idx + 4];
                    // 背景是深色的，字是浅色的：亮度过阈值就算"有字"。
                    if px[3] == 255 && px[0] as u32 + px[1] as u32 + px[2] as u32 > 420 {
                        ink += 1;
                    }
                }
            }
            assert!(ink > 40, "第 {i} 行几乎没画出字（{ink} px）");
        }
    }
}
