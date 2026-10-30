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
use zouwu_ui::history::{draw, filter, layout, query_terms, search_inner, Search, View};
use zouwu_ui::linefield::LineField;

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
    let footer = "↑↓ 选择 · Enter 复制并关闭 · 输入即搜索 · Delete 删除 · Esc 关闭";
    let placeholder = "搜索（空格分隔多个关键词）";

    // 名字 / 条目 / 查询 / 选中 / 滚动 / 光标或选区（字节区间，None＝不在框里）
    let cases: [(
        &str,
        &Vec<Entry>,
        &str,
        usize,
        usize,
        Option<(usize, usize)>,
    ); 7] = [
        ("many", &many, "", 0, 0, None),
        ("scrolled", &many, "", 12, 5, None),
        ("empty", &empty, "", 0, 0, None),
        ("search", &many, "cargo", 0, 0, None),
        // 光标停在框里（空选区）：验证"点得进去"
        ("search-focused", &many, "cargo", 0, 0, Some((5, 5))),
        // 双击选中 test：验证选区底纹压在字上
        ("search-selected", &many, "cargo test", 0, 0, Some((6, 10))),
        ("no-match", &many, "根本不存在的词", 0, 0, None),
    ];

    for (name, entries, query, selected, scroll, span) in cases {
        let visible = filter(entries, &query_terms(query));
        let l = layout(visible.len());
        let theme = Theme::fallback();
        let w = (l.size.0 * dpi).round() as u32;
        let h = (l.size.1 * dpi).round() as u32;
        let mut pixmap = Pixmap::new(w, h).expect("pixmap");
        let empty_text = if query.is_empty() {
            "还没有记录"
        } else {
            "没有匹配的条目"
        };
        // 搜索框的真实视口：宽度与 DPI 都得给，否则光标位置与横向滚动算不出来。
        let mut search = Search::default();
        search.set(query);
        search.set_scale(dpi);
        search.set_width(search_inner(l.search, !query.is_empty()).w);
        if let Some((anchor, caret)) = span {
            search.set_anchor(anchor);
            search.set_caret(caret);
        }
        draw(
            &mut pixmap,
            entries,
            &visible,
            &l,
            &theme,
            View {
                selected,
                scroll,
                hover: if visible.is_empty() { None } else { Some(0) },
                search: &search,
                search_focus: span.is_some(),
                placeholder,
                footer,
                empty: empty_text,
            },
        );
        let path = out.join(format!("{name}.png"));
        pixmap.save_png(&path).expect("save png");
        println!(
            "{}: {} hits, rows {} @ {:.0}x{:.0}",
            path.display(),
            visible.len(),
            l.rows.len(),
            l.size.0,
            l.size.1
        );
    }
}
