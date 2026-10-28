//! 生成一张二维码 PNG（给手工验收用：把它摆在屏幕上，再跑 `scan_qr`）。
//!
//! `cargo run -p zouwu-ui --example qr_png -- "https://example.com/zouwu"`

fn main() {
    let text = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com/zouwu".to_string());
    let code = qrcode::QrCode::new(text.as_bytes()).expect("encode");
    let modules = code.width();
    let scale = 8u32;
    let quiet = 4u32;
    let side = (modules as u32 + quiet * 2) * scale;
    let colors = code.to_colors();

    let mut pixmap = tiny_skia::Pixmap::new(side, side).expect("pixmap");
    pixmap.fill(tiny_skia::Color::WHITE);
    let mut paint = tiny_skia::Paint::default();
    paint.set_color(tiny_skia::Color::BLACK);
    paint.anti_alias = false;
    for y in 0..modules {
        for x in 0..modules {
            if colors[y * modules + x] != qrcode::Color::Dark {
                continue;
            }
            let rect = tiny_skia::Rect::from_xywh(
                ((x as u32 + quiet) * scale) as f32,
                ((y as u32 + quiet) * scale) as f32,
                scale as f32,
                scale as f32,
            )
            .unwrap();
            pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
        }
    }

    let out = std::path::Path::new("target/qr-demo.png");
    pixmap.save_png(out).expect("write png");
    println!("wrote {} ({side}x{side}) for {:?}", out.display(), text);
}
