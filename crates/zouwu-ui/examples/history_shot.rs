//! 把剪贴板历史窗离屏渲染成 PNG（本机截不了屏，画面验收全靠这个）。
//!
//! ```text
//! cargo run -p zouwu-ui --example history_shot
//! ZOUWU_DPI=1.5 cargo run -p zouwu-ui --example history_shot
//! ```

use std::path::PathBuf;

use tiny_skia::Pixmap;

use zouwu_core::history::Entry;
use zouwu_core::theme::Theme;
use zouwu_ui::history::{draw, layout, View};

fn main() {
    let out = PathBuf::from("target/zouwu-history");
    std::fs::create_dir_all(&out).expect("create output dir");
    let dpi: f32 = std::env::var("ZOUWU_DPI")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(2.0)
        .clamp(0.75, 4.0);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let many: Vec<Entry> = (0..14)
        .map(|i| {
            let text = match i % 4 {
                0 => "https://example.com/zouwu?from=clipboard&ref=history".to_string(),
                1 => "cargo test --workspace 2>&1 | tail -30".to_string(),
                2 => "第一行\n第二行\n第三行（多行内容在列表里只露第一行）".to_string(),
                _ => format!("第 {i} 条：一段普通的中文文本，用来看看截断是否自然"),
            };
            Entry::new(text, now - (i as u64) * 37)
        })
        .collect();
    let empty: Vec<Entry> = Vec::new();

    let cases: [(&str, &Vec<Entry>, usize, usize); 3] = [
        ("many", &many, 0, 0),
        ("scrolled", &many, 12, 5),
        ("empty", &empty, 0, 0),
    ];

    for (name, entries, selected, scroll) in cases {
        let l = layout(entries.len());
        let theme = Theme::fallback();
        let w = (l.size.0 * dpi).round() as u32;
        let h = (l.size.1 * dpi).round() as u32;
        let mut pixmap = Pixmap::new(w, h).expect("pixmap");
        draw(
            &mut pixmap,
            entries,
            &l,
            &theme,
            View {
                selected,
                scroll,
                hover: if entries.is_empty() { None } else { Some(2) },
                footer: "↑↓ 选择 · Enter 复制并关闭 · Delete 删除 · Esc 关闭",
                empty: "还没有记录",
            },
        );
        let path = out.join(format!("{name}.png"));
        pixmap.save_png(&path).expect("save png");
        println!(
            "{}: rows {} @ {:.0}x{:.0}",
            path.display(),
            l.rows.len(),
            l.size.0,
            l.size.1
        );
    }
}
