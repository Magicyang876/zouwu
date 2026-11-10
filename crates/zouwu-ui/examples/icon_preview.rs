//! 气泡球图标预览：把几张 PNG 放进气泡球，按**真实渲染路径**（`paint::render`）出图。
//!
//! ```bash
//! cargo run -p zouwu-ui --example icon_preview -- a.png b.png c.png
//! ```
//!
//! 不给参数时读 `%APPDATA%\zouwu\icons` 下的所有 PNG。
//! 产物：`target/zouwu-offscreen/ring-icons-{light,dark}.png`（1.0 与 1.25 两档 DPI 并排）。
//!
//! 为什么要有它：图标在球里只有 ~30 px，好不好认必须在**实际尺寸**看，
//! 拿 256 px 的原图判断等于没判断。

use std::path::PathBuf;

use tiny_skia::{Color, Pixmap};
use zouwu_core::action::Glyph;
use zouwu_core::geom::Point;
use zouwu_core::ring::RingConfig;
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

fn icon_paths() -> Vec<PathBuf> {
    let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if !args.is_empty() {
        return args;
    }
    let dir = zouwu_platform::assets::icons_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn shoot(name: &str, icons: &[(String, Pixmap)], logical: f32, scale: f32, theme: &Theme) {
    let count = icons.len().max(1);
    let ring = RingConfig::default();
    let center = Point::new(logical * 0.5, logical * 0.5);
    let layout = ring.layout(count, center, 0.0);
    let glyph = Glyph::Bolt;
    let bubbles: Vec<BubblePaint<'_>> = layout
        .iter()
        .enumerate()
        .map(|(index, bubble)| BubblePaint {
            center: bubble.center,
            radius: bubble.radius,
            scale: 1.0,
            alpha: 1.0,
            glyph,
            icon: icons.get(index).map(|(_, pixmap)| pixmap),
            selected: false,
        })
        .collect();

    let px = (logical * scale).round() as u32;
    let mut pixmap = Pixmap::new(px, px).expect("pixmap");
    // 铺一层桌面色：球是浅色的，透明底上看不出边缘。
    pixmap.fill(Color::from_rgba8(58, 50, 45, 255));
    let scene = Scene {
        width: logical,
        height: logical,
        scale,
        buddy_center: center,
        buddy_radius: 34.0,
        buddy_scale: 1.0,
        buddy_alpha: 1.0,
        blink: 0.0,
        breath: 0.0,
        theme,
        bubbles: &bubbles,
        skin: None,
    };
    render(&scene, &mut pixmap);
    let path = out_dir().join(format!("{name}.png"));
    pixmap.save_png(&path).expect("save png");
    println!(
        "{:<22} {px}x{px}  {} icons  ->  {}",
        name,
        icons.len(),
        path.display()
    );
}

fn main() {
    let paths = icon_paths();
    if paths.is_empty() {
        eprintln!("没有可预览的 PNG：传路径，或往 %APPDATA%\\zouwu\\icons 里放几张");
        return;
    }
    let mut icons = Vec::new();
    for path in &paths {
        match std::fs::read(path)
            .ok()
            .and_then(|bytes| Pixmap::decode_png(&bytes).ok())
        {
            Some(pixmap) => {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                println!("load {name}  {}x{}", pixmap.width(), pixmap.height());
                icons.push((name, pixmap));
            }
            None => eprintln!("skip {}: 不是能解的 PNG", path.display()),
        }
    }
    if icons.is_empty() {
        return;
    }

    let warm = Theme::by_name("zouwu-warm");
    let dark = Theme::by_name("zouwu-dark");
    // 球里只有 ~30 px，两档 DPI 各看一眼：1.0 模拟 100%，1.25 模拟 125% 缩放。
    shoot("ring-icons", &icons, 340.0, 1.0, &warm);
    shoot("ring-icons-hi", &icons, 340.0, 1.25, &warm);
    shoot("ring-icons-dark", &icons, 340.0, 1.0, &dark);
}
