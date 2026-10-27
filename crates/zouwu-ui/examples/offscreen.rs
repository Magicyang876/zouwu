//! 离屏渲染检查：不依赖窗口、不依赖任何桌面环境，直接把一帧场景光栅成 PNG。
//!
//! ```bash
//! cargo run -p zouwu-ui --example offscreen
//! ```
//!
//! 产物写到 `target/zouwu-offscreen/`：
//! * `collapsed.png`   —— 常态（只有驺吾本体）
//! * `expanded.png`    —— 悬停展开（一圈气泡）
//! * `hover-hi.png`    —— 125% DPI 下的展开态（验证 DPI 缩放）
//!
//! 用途：在没有图形界面的机器上也能检查画面是否正确，同时是皮肤改动时的肉眼回归工具。

use std::path::PathBuf;

use tiny_skia::{Color, Pixmap};
use zouwu_core::action::{default_ring, ActionItem};
use zouwu_core::geom::{Point, Rect};
use zouwu_core::ring::{solve_rotation, RingConfig};
use zouwu_core::theme::Theme;
use zouwu_ui::paint::{render, BubblePaint, Scene};

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("zouwu-offscreen");
    std::fs::create_dir_all(&dir).expect("create output dir");
    dir
}

/// 复刻 `app.rs` 的布局逻辑：先算整体旋转，再算每个气泡的位置。
fn bubble_paints(
    items: &[ActionItem],
    ring: &RingConfig,
    center: Point,
    view: Rect,
    openness: f32,
) -> Vec<BubblePaint> {
    let rotation = solve_rotation(ring, center, view, items.len());
    ring.layout(items.len(), center, rotation)
        .into_iter()
        .zip(items)
        .map(|(bubble, item)| BubblePaint {
            center: bubble.center,
            radius: bubble.radius,
            // 弹簧的过冲用抛物线近似：0 → 1，中途略过 1 再回落
            scale: openness,
            alpha: openness.clamp(0.0, 1.0),
            glyph: item.glyph,
            selected: false,
        })
        .collect()
}

fn shoot(name: &str, logical: f32, scale: f32, openness: f32, theme: &Theme) {
    let items = default_ring();
    let ring = RingConfig::default();
    let px = (logical * scale).round() as u32;
    let mut pixmap = Pixmap::new(px, px).expect("pixmap");
    pixmap.fill(Color::TRANSPARENT);

    let center = Point::new(logical * 0.5, logical * 0.5);
    let view = Rect::new(0.0, 0.0, logical, logical);
    let bubbles = bubble_paints(&items, &ring, center, view, openness);

    let scene = Scene {
        width: logical,
        height: logical,
        scale,
        buddy_center: center,
        buddy_radius: 56.0,
        buddy_scale: 1.0 + 0.06 * openness,
        buddy_alpha: 1.0,
        blink: 0.0,
        theme,
        bubbles: &bubbles,
    };
    render(&scene, &mut pixmap);

    let path = out_dir().join(format!("{name}.png"));
    pixmap.save_png(&path).expect("save png");
    let data = pixmap.data();
    let visible = data.chunks_exact(4).filter(|p| p[3] > 8).count();
    let max_alpha = data.chunks_exact(4).map(|p| p[3]).max().unwrap_or(0);
    println!(
        "{:<14} {}x{}  visible_px={:<7} max_alpha={}  ->  {}",
        name,
        px,
        px,
        visible,
        max_alpha,
        path.display()
    );
}

fn main() {
    let warm = Theme::by_name("zouwu-warm");
    let dark = Theme::by_name("zouwu-dark");

    shoot("collapsed", 340.0, 1.0, 0.0, &warm);
    shoot("expanded", 340.0, 1.0, 1.0, &warm);
    shoot("expanded-dark", 340.0, 1.0, 1.0, &dark);
    shoot("expanded-os", 340.0, 1.0, -0.08, &warm); // 负值 = 收起过冲的一瞬间
    shoot("hover-hi", 340.0, 1.25, 1.0, &warm);
    println!("\n用任意看图工具打开上面的 PNG 即可检查皮肤与布局。");
}
