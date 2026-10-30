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

/// 把“当前 DPI + 输入框宽度”告诉模型（真程序里由 `rebuild_layout` 做）。
/// 返回内框矩形，方便接着模拟“拖滚动条”。
fn edit_viewport(
    m: &mut Model,
    w: f32,
    h: f32,
    target: settings::EditTarget,
) -> Option<zouwu_ui::widget::Rect> {
    let layout = settings::layout(m, w, h);
    let inner = layout
        .rows
        .iter()
        .find_map(|(row, rl)| match &row.control {
            settings::Control::Text { target: t, .. } if *t == target => {
                Some(settings::text_inner(rl.hit))
            }
            _ => None,
        })?;
    settings::set_edit_viewport(m, inner, 1.0);
    Some(inner)
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

    // 鼠标拖选：名字整段被选中（选区底色 + 只有一个光标）。
    let mut sel = Config::default();
    let mut extra = zouwu_core::config::RingItem::custom("custom.1");
    extra.label = "打开记事本".to_string();
    extra.command = "notepad.exe".to_string();
    sel.ring.items.push(extra);
    let mut m4 = Model::new(
        sel,
        Vec::new(),
        Vec::new(),
        Paths {
            config: String::new(),
            log: String::new(),
        },
    );
    m4.set_tab(Tab::Bubbles);
    settings::begin_edit(&mut m4, settings::EditTarget::ItemLabel(2));
    if let Some(editing) = m4.editing.as_mut() {
        // 模拟“从行首拖到行尾”：锚点在 0、光标在末尾。
        editing.anchor = 0;
        editing.cursor = editing.buffer.len();
    }
    shoot("settings-bubbles-selected.png", 760.0, 600.0, &mut m4, 1.0);
    // 同一状态在 125% 缩放下再拍一张：选区/光标靠真实文字度量算 x，
    // 忘了乘 DPI 就会出现“在 100% 下对、在 125% 下偏半格”。
    shoot(
        "settings-bubbles-selected-dpi.png",
        760.0,
        600.0,
        &mut m4,
        1.25,
    );

    // 长命令编辑中 + 已经横滚到中间：水平滚动条（轨道 + thumb）要看得见，
    // 而且可见文字必须从“滚到的那一个字”开始（不是从头截）。
    let mut long = Config::default();
    let mut long_item = zouwu_core::config::RingItem::custom("custom.1");
    long_item.label = "打开工具".to_string();
    long_item.command =
        "C:\\Program Files\\Some Vendor\\Some Tool\\bin\\tool.exe --flag value".to_string();
    long.ring.items.push(long_item);
    let mut m5 = Model::new(
        long,
        Vec::new(),
        Vec::new(),
        Paths {
            config: String::new(),
            log: String::new(),
        },
    );
    m5.set_tab(Tab::Bubbles);
    settings::begin_edit(&mut m5, settings::EditTarget::ItemCommand(2));
    if let Some(inner) = edit_viewport(&mut m5, 760.0, 600.0, settings::EditTarget::ItemCommand(2))
    {
        // 拖到 45% 处：文字与 thumb 都得跟着走。
        settings::set_offset_from_x(&mut m5, inner, inner.x + inner.w * 0.45);
    }
    shoot("settings-bubbles-scrolled.png", 760.0, 600.0, &mut m5, 1.0);

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
