//! 给 README 出「七套形象」的对照图：一套一张、透明底、同一尺寸。
//!
//! 走的是应用真正的那条渲染路径（`SkinPackage` → `Skin::decode` → `paint::render`），
//! 不是把 `body.png` 缩一缩——README 里的样子必须就是屏幕上 1:1 会看到的样子。
//!
//! ```text
//! cargo run -p zouwu-ui --example skins_gallery
//! ```

use std::path::{Path, PathBuf};

use tiny_skia::{Color, Pixmap};
use zouwu_core::geom::Point;
use zouwu_core::skin::PROCEDURAL_SKIN_ID;
use zouwu_core::theme::Theme;
use zouwu_platform::assets;
use zouwu_ui::paint::{render, Scene};
use zouwu_ui::skin::Skin;

/// 画布边长（逻辑像素）。主体半径 88，四角留出眨眼与浮动的余量。
const SIDE: f32 = 240.0;
/// 出图倍率：README 里按 120 px 显示，2 倍刚好够 Retina。
const SCALE: f32 = 2.0;
const RADIUS: f32 = 88.0;

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/screenshots/skins")
}

fn main() {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).expect("create out dir");
    let theme = Theme::by_name("zouwu-warm");

    let i18n = zouwu_core::i18n::I18n::new("zh-CN");
    let mut entries: Vec<(String, String)> = vec![(
        PROCEDURAL_SKIN_ID.to_string(),
        i18n.t("settings.skin.procedural"),
    )];
    for id in assets::builtin_skin_ids() {
        let name = assets::load_skin(id)
            .ok()
            .and_then(|p| p.manifest.name)
            .unwrap_or_else(|| id.to_string());
        entries.push((id.to_string(), name));
    }

    for (index, (id, name)) in entries.iter().enumerate() {
        let skin = if id == PROCEDURAL_SKIN_ID {
            Skin::procedural()
        } else {
            let package = assets::load_skin(id).expect("skin loads");
            Skin::decode(&package).expect("skin decodes")
        };

        let px = (SIDE * SCALE).round() as u32;
        let mut pixmap = Pixmap::new(px, px).expect("pixmap");
        // 透明底：GitHub 的浅色/深色模式各自适配，比上一版铺死的米色干净。
        pixmap.fill(Color::from_rgba8(0, 0, 0, 0));

        let scene = Scene {
            width: SIDE,
            height: SIDE,
            scale: SCALE,
            buddy_center: Point::new(SIDE * 0.5, SIDE * 0.5),
            buddy_radius: RADIUS,
            buddy_scale: 1.0,
            buddy_alpha: 1.0,
            blink: 0.0,
            breath: 0.0,
            theme: &theme,
            bubbles: &[],
            skin: Some(&skin),
        };
        render(&scene, &mut pixmap);

        let file = format!("{}-{}.png", index + 1, id);
        let path = dir.join(&file);
        pixmap.save_png(&path).expect("save png");
        println!("{file} -> {} ({name})", path.display());
    }
}
