//! 把设置面板的每一页离屏渲染成 PNG。
//!
//! 存在理由和 `offscreen.rs` 一样：截图在本机（远程会话/无合成器）拿不到，
//! 而布局与字体必须**看见**才能验收。也顺带验证了 DPI 缩放路径（2× 渲染）。

use std::path::PathBuf;

use tiny_skia::Pixmap;
use zouwu_core::config::Config;
use zouwu_core::theme::Theme;
use zouwu_ui::settings::{self, Model, Paths, Tab};

fn main() -> Result<(), String> {
    // 日志会告诉我们字体是否加载成功（面板全是文本，字体没了就白屏）。
    zouwu_platform::logging::init();

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("zouwu-settings");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;

    let themes = vec![Theme::by_name("zouwu-warm"), Theme::by_name("zouwu-dark")];
    let skins = vec![("procedural".to_string(), "代码绘制（内置）".to_string())];

    for (tab, file) in [
        (Tab::General, "general.png"),
        (Tab::Interaction, "interaction.png"),
        (Tab::Appearance, "appearance.png"),
        (Tab::About, "about.png"),
    ] {
        let mut model = Model::new(
            Config::default(),
            themes.clone(),
            skins.clone(),
            Paths {
                config: r"C:\Users\you\AppData\Roaming\zouwu\config.toml".into(),
                log: r"C:\Users\you\AppData\Roaming\zouwu\logs\zouwu.log".into(),
            },
        );
        model.set_tab(tab);
        // 键盘焦点 + 悬停各演示一行，方便看选中/悬停态。
        model.keyboard_used = true;

        let (lw, lh) = settings::DEFAULT_SIZE;
        let scale = 2.0;
        let layout = settings::layout(&model, lw, lh);
        model.focus = layout.rows.iter().position(|(row, _)| row.field.is_some());
        let layout = settings::layout(&model, lw, lh);

        let mut pixmap =
            Pixmap::new((lw * scale) as u32, (lh * scale) as u32).ok_or("pixmap alloc failed")?;
        settings::draw(&mut pixmap, &layout, &model);

        let path = out.join(file);
        pixmap.save_png(&path).map_err(|e| e.to_string())?;
        println!("{}", path.display());
    }
    Ok(())
}
