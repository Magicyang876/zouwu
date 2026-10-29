//! 设置面板「气泡球」页的离屏渲染检查（临时工具，验完即删）。
//!
//! ```bash
//! cargo run -p zouwu-ui --example settings_preview
//! ```
//!
//! 产物写到 `target/zouwu-offscreen/`：
//! * `settings-bubbles*.png` —— 「气泡球」页的各种状态（常态 / 最小宽 / 滚到底 / 录热键 / 编辑 / 空态）
//! * `ring-1..6.png`        —— 环上 1~6 颗球的均分效果（"配置几个就几个"的肉眼回归）

use std::path::PathBuf;

use tiny_skia::Pixmap;
use zouwu_core::config::{Config, RingCfg, RingItem};
use zouwu_core::geom::{Point, Rect};
use zouwu_core::i18n::I18n;
use zouwu_core::ring::solve_rotation;
use zouwu_ui::paint::{BubblePaint, Scene};
use zouwu_ui::settings::{self, Model, Paths, Tab};

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("zouwu-offscreen");
    std::fs::create_dir_all(&dir).expect("create output dir");
    dir
}

fn shoot(name: &str, w: f32, h: f32, model: &mut Model, scale: f32) {
    let layout = settings::layout(model, w, h);
    let mut pixmap = Pixmap::new((w * scale) as u32, (h * scale) as u32).expect("pixmap");
    settings::draw(&mut pixmap, &layout, model);
    let path = out_dir().join(name);
    pixmap.save_png(&path).expect("save png");
    println!("{}", path.display());
}

fn main() {
    let mut config = Config::default();
    config.appearance.theme = "lined-paper".to_string();
    config.general.language = "zh-CN".to_string();
    // 一颗关掉的、一颗自定义的、一颗没命令的 —— 三种行样式都要能看到。
    config.ring.items = vec![
        RingItem::builtin(zouwu_core::action::QRCODE_ID),
        RingItem::builtin(zouwu_core::action::CLIPBOARD_ID),
        RingItem {
            id: "custom.1".into(),
            enabled: false,
            hotkey: "Ctrl+Alt+9".into(),
            label: "记事本".into(),
            command: "notepad.exe".into(),
            icon: String::new(),
        },
        RingItem {
            id: "custom.2".into(),
            enabled: true,
            hotkey: String::new(),
            label: String::new(),
            command: String::new(),
            icon: String::new(),
        },
    ];

    let paths = Paths {
        config: "C:\\Users\\admin\\AppData\\Roaming\\zouwu\\config.toml".into(),
        log: "C:\\Users\\admin\\AppData\\Roaming\\zouwu\\logs\\zouwu.log".into(),
    };

    let mut model = Model::new(config.clone(), Vec::new(), Vec::new(), paths);
    model.set_tab(Tab::Bubbles);
    shoot("settings-bubbles.png", 760.0, 600.0, &mut model, 1.0);
    // 最小尺寸 + 125% DPI：检查控件是否被挤扁、文字是否错位。
    shoot("settings-bubbles-min.png", 560.0, 620.0, &mut model, 1.25);

    // 滚到底：移除按钮 + 两个新增按钮。
    model.scroll = 100000.0;
    shoot("settings-bubbles-bottom.png", 760.0, 600.0, &mut model, 1.0);
    model.scroll = 0.0;

    // 正在录热键：框要变成强调色并提示按键。
    let mut cap = Model::new(
        config.clone(),
        Vec::new(),
        Vec::new(),
        Paths {
            config: String::new(),
            log: String::new(),
        },
    );
    cap.set_tab(Tab::Bubbles);
    settings::begin_capture(&mut cap, settings::CaptureTarget::Item(1));
    shoot("settings-bubbles-capture.png", 760.0, 600.0, &mut cap, 1.0);

    // 交互状态：第 1 颗球正在录热键、第 2 颗在编辑名字、鼠标悬停在被删掉的球上。
    settings::begin_capture(&mut model, settings::CaptureTarget::Item(0));
    settings::begin_edit(&mut model, settings::EditTarget::ItemLabel(2));
    settings::edit_text(&mut model, "计算器");
    let mut model2 = model;
    shoot(
        "settings-bubbles-editing.png",
        760.0,
        600.0,
        &mut model2,
        1.0,
    );

    // 一颗球都没有时的空态 + 漏出的描边不该出现。
    let mut empty = Config::default();
    empty.ring.items.clear();
    let mut m3 = Model::new(
        empty,
        Vec::new(),
        Vec::new(),
        Paths {
            config: String::new(),
            log: String::new(),
        },
    );
    m3.set_tab(Tab::Bubbles);
    shoot("settings-bubbles-empty.png", 760.0, 600.0, &mut m3, 1.0);

    // 环上到底几颗球：配置几个启用的就几颗，且均匀分布。
    let mut ring_items = Vec::new();
    for n in 0..6 {
        let id = if n == 0 {
            zouwu_core::action::QRCODE_ID.to_string()
        } else {
            format!("custom.{n}")
        };
        ring_items.push(RingItem {
            id,
            enabled: true,
            hotkey: String::new(),
            label: format!("{n}"),
            command: String::new(),
            icon: String::new(),
        });
    }
    let theme = zouwu_core::theme::Theme::builtin("zouwu-warm").expect("theme");
    for n in 1..=6 {
        let cfg = RingCfg {
            items: ring_items[..n].to_vec(),
            ..RingCfg::default()
        };
        let items = zouwu_core::action::resolve(&cfg.items, &I18n::new("zh-CN"));
        let ring = cfg.to_ring_config();
        let size = 360.0f32;
        let center = Point::new(size * 0.5, size * 0.5);
        let view = Rect::new(0.0, 0.0, size, size);
        let rotation = solve_rotation(&ring, center, view, items.len());
        let paints: Vec<BubblePaint> = ring
            .layout(items.len(), center, rotation)
            .into_iter()
            .zip(&items)
            .map(|(b, it)| BubblePaint {
                center: b.center,
                radius: b.radius,
                scale: 1.0,
                alpha: 1.0,
                glyph: it.glyph,
                icon: None,
                selected: false,
            })
            .collect();
        let mut pm = Pixmap::new(size as u32, size as u32).expect("pixmap");
        let theme = &theme;
        let scene = Scene {
            width: size,
            height: size,
            scale: 1.0,
            buddy_center: center,
            buddy_radius: 28.0,
            buddy_scale: 1.0,
            buddy_alpha: 1.0,
            blink: 0.0,
            breath: 0.0,
            theme,
            bubbles: &paints,
            skin: None,
        };
        zouwu_ui::paint::render(&scene, &mut pm);
        let path = out_dir().join(format!("ring-{n}.png"));
        pm.save_png(&path).expect("png");
        println!("{}", path.display());
    }
}
