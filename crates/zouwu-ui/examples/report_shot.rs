//! 把结果小窗离屏渲染成 PNG，用来做视觉验收。
//!
//! ```text
//! cargo run -p zouwu-ui --example report_shot
//! # 想按真机缩放看：ZOUWU_DPI=1.5 cargo run ...
//! ```
//!
//! 截图在本机不可用（整个桌面只有两种颜色），所以窗口类的东西一律靠这个出图。

use std::path::PathBuf;

use tiny_skia::Pixmap;

use zouwu_core::theme::Theme;
use zouwu_ui::report::{draw, layout, Entry, Report};

fn main() {
    let out_dir = PathBuf::from("target/zouwu-report");
    std::fs::create_dir_all(&out_dir).expect("create output dir");

    let dpi: f32 = std::env::var("ZOUWU_DPI")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(2.0)
        .clamp(0.75, 4.0);

    let found = Report {
        headline: "识别到 2 个二维码".to_string(),
        detail: Some("用时 68 毫秒".to_string()),
        entries: vec![
            Entry {
                text: "https://example.com/zouwu".to_string(),
                screen: "显示器 1".to_string(),
            },
            Entry {
                text: "WIFI:S:Zouwu-Guest;T:WPA;P:rongyu2026;;".to_string(),
                screen: "显示器 2".to_string(),
            },
        ],
        footer: "点一行复制内容 · Esc 关闭".to_string(),
    };

    let empty = Report {
        headline: "没有找到二维码".to_string(),
        detail: Some("已扫描 5760×1080 的整个桌面 · 用时 41 毫秒".to_string()),
        entries: Vec::new(),
        footer: "点一行复制内容 · Esc 关闭".to_string(),
    };

    let many = Report {
        headline: "识别到 12 个二维码".to_string(),
        detail: Some("用时 152 毫秒 · 有 1 个码读不出来".to_string()),
        entries: (0..12)
            .map(|i| Entry {
                text: format!("https://example.com/item/{i}?from=poster&ref=zouwu"),
                screen: format!("显示器 {}", i % 2 + 1),
            })
            .collect(),
        footer: "点一行复制内容 · Esc 关闭".to_string(),
    };

    for (name, report) in [("found", &found), ("empty", &empty), ("many", &many)] {
        let l = layout(report);
        let theme = Theme::fallback();
        let w = (l.size.0 * dpi).round() as u32;
        let h = (l.size.1 * dpi).round() as u32;
        let mut pixmap = Pixmap::new(w, h).expect("pixmap");
        draw(&mut pixmap, report, &l, &theme);
        let path = out_dir.join(format!("{name}.png"));
        pixmap.save_png(&path).expect("save png");
        println!(
            "{}: {:.0}x{:.0} logical, {} rows, {} hidden",
            path.display(),
            l.size.0,
            l.size.1,
            l.rows.len(),
            l.hidden
        );
    }
}
