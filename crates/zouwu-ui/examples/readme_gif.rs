//! README 顶部的预览动图：把"悬停 → 一圈球依次弹出来"渲成逐帧 PNG，再拼成 GIF。
//!
//! 为什么不录屏：录屏会带上用户的壁纸、DPI、任务栏，还得手工裁；离屏渲染是**同一套
//! 绘制代码**（`zouwu_ui::paint::render`），帧率、时机、主题都完全可控，改配色重跑一遍就行。
//!
//! 帧数与节奏：24 帧 × 60ms ≈ 1.4s。再长就是"等它演完"，再短看不清"球是一个一个出来的"。

use std::path::PathBuf;

use tiny_skia::{Color, Pixmap};
use zouwu_core::action::ActionItem;
use zouwu_core::geom::{Point, Rect};
use zouwu_core::ring::{solve_rotation, RingConfig};
use zouwu_core::theme::Theme;
use zouwu_ui::paint::{render, BubblePaint, Scene};

/// 逻辑尺寸：和真实挂件的默认窗口一致（ADR-048 之后的 340）。
const LOGICAL: f32 = 340.0;
/// 出图倍率：2 倍再缩一半，边缘才不像锯齿。
const SCALE: f32 = 2.0;
const FRAMES: usize = 24;
const FRAME_MS: f32 = 60.0;
/// 每颗球的起跑延迟（与 `RingCfg::expand_stagger_ms` 默认值一致）。
const STAGGER_MS: f32 = 24.0;

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("zouwu-offscreen")
        .join("preview-frames");
    std::fs::create_dir_all(&dir).expect("create output dir");
    dir
}

/// 欠阻尼弹簧：0 → 1，中途冲过 1 再回落。
///
/// 和 `zouwu_core::anim::Spring` 的曲线**刻意不同**：那个是给实时循环用的数值积分，
/// 这里要的是一条能按帧号直接算出来的解析曲线，方便逐帧重渲。
fn spring(t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    let t = t.min(1.0);
    let value = 1.0 - (-6.0 * t).exp() * (9.0 * t).cos();
    // 收尾时把残余的负值抹平，免得最后一帧还在往回缩。
    if t >= 1.0 {
        1.0
    } else {
        value.max(0.0)
    }
}

fn bubble_paints(
    items: &[ActionItem],
    ring: &RingConfig,
    center: Point,
    view: Rect,
    elapsed_ms: f32,
) -> Vec<BubblePaint<'static>> {
    let rotation = solve_rotation(ring, center, view, items.len());
    ring.layout(items.len(), center, rotation)
        .into_iter()
        .zip(items)
        .enumerate()
        .map(|(i, (bubble, item))| {
            let start = i as f32 * STAGGER_MS;
            let t = (elapsed_ms - start) / 260.0;
            let scale = spring(t);
            BubblePaint {
                center: bubble.center,
                radius: bubble.radius,
                scale,
                alpha: scale.clamp(0.0, 1.0),
                glyph: item.glyph,
                icon: None,
                selected: false,
            }
        })
        .collect()
}

fn main() {
    let dir = out_dir();
    // 暖色主题（默认），浅色壁纸上的观感；README 里和深色那两张静态图配套。
    let theme = Theme::builtin("zouwu-warm").expect("theme");
    let items = zouwu_core::action::resolve(
        &zouwu_core::config::RingCfg::default().items,
        &zouwu_core::i18n::I18n::new("zh-CN"),
    );
    let ring = RingConfig::default();
    let px = (LOGICAL * SCALE).round() as u32;
    let center = Point::new(LOGICAL * 0.5, LOGICAL * 0.5);
    let view = Rect::new(0.0, 0.0, LOGICAL, LOGICAL);

    for frame in 0..FRAMES {
        let elapsed = frame as f32 * FRAME_MS;
        let mut pixmap = Pixmap::new(px, px).expect("pixmap");
        pixmap.fill(Color::TRANSPARENT);

        let bubbles = bubble_paints(&items, &ring, center, view, elapsed);
        // 第 10~13 帧眨一次眼：动图里"有生命"的信号主要靠这个。
        let blink = if (10..14).contains(&frame) {
            let phase = (frame - 10) as f32 / 3.0;
            if phase < 0.5 {
                phase * 2.0
            } else {
                (1.0 - phase) * 2.0
            }
        } else {
            0.0
        };
        // 呼吸：一圈 1.4s，浅一点就够（真实值是 0.03 的半径起伏）。
        let breath = (elapsed / 1400.0 * std::f32::consts::TAU).sin();

        let scene = Scene {
            width: LOGICAL,
            height: LOGICAL,
            scale: SCALE,
            buddy_center: center,
            buddy_radius: 56.0,
            buddy_scale: 1.0 + 0.03 * breath,
            buddy_alpha: 1.0,
            blink,
            breath,
            theme: &theme,
            bubbles: &bubbles,
            skin: None,
        };
        render(&scene, &mut pixmap);

        let path = dir.join(format!("f{frame:02}.png"));
        pixmap.save_png(&path).expect("save png");
        println!("frame {frame:02} -> {}", path.display());
    }
    println!("{FRAMES} frames in {}", dir.display());
}
