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

    // 真实主题/皮肤列表：形象页的缩略图必须走真实加载路径，
    // 否则这个示例验收的是"假图"，而假图永远不会和桌面上的小兽不一样。
    let themes = vec![Theme::by_name("zouwu-warm"), Theme::by_name("zouwu-dark")];
    let _ = zouwu_platform::assets::ensure_builtin_skins();
    // 字体包：先注册磁盘上已有的包，再把配置里选中的那套装上。
    // 面板全是文字，字体这一层看不见就等于没验收。
    zouwu_platform::fontpack::register_all();
    let mut config = Config::default();
    config.general.language = "zh-CN".into();
    if let Ok(pack_id) = std::env::var("ZOUWU_FONT") {
        if let Some(pack) = zouwu_platform::fontpack::get(pack_id.trim()) {
            zouwu_ui::text::set_family(Some(pack.family.clone()));
            config.appearance.font_pack = pack.id.clone();
            eprintln!("font pack: {} ({})", pack.display_name(), pack.family);
        } else {
            eprintln!("no such font pack: {pack_id}");
        }
    }
    let skins = zouwu_platform::assets::skin_choices(&zouwu_core::i18n::I18n::new(
        &config.general.language,
    ));

    for (tab, file) in [
        (Tab::General, "general.png"),
        (Tab::Interaction, "interaction.png"),
        (Tab::Appearance, "appearance.png"),
        (Tab::Skins, "skins.png"),
        (Tab::About, "about.png"),
    ] {
        let mut model = Model::new(
            config.clone(),
            themes.clone(),
            skins.clone(),
            Paths {
                config: r"C:\Users\you\AppData\Roaming\zouwu\config.toml".into(),
                log: r"C:\Users\you\AppData\Roaming\zouwu\logs\zouwu.log".into(),
            },
        );
        model.refresh_fonts();
        model.set_tab(tab);
        // 键盘焦点 + 悬停各演示一行，方便看选中/悬停态。
        model.keyboard_used = true;
        if tab == Tab::Skins {
            model.ensure_thumbs();
            model.hover_skin = Some(1);
            // 勾上「覆盖已安装」：这一页的重点控件，图里必须能看见它的两种状态之一。
            model.overwrite_skin = true;
            // 展开长说明：这条说明是"点开才看到"的，图里必须能验收它的排版。
            model.overwrite_help_open = std::env::var("ZOUWU_NOTE").is_ok();
        }

        let (lw, lh) = settings::DEFAULT_SIZE;
        // `ZOUWU_DPI` 用来复现真机缩放：文字排版对「逻辑像素 × 整数倍」很敏感，
        // 2.0 下正常的布局在 1.25/1.5 下可能有细微差（ADR-029 就是这么漏掉的）。
        let scale = std::env::var("ZOUWU_DPI")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| (0.75..=4.0).contains(v))
            .unwrap_or(2.0);
        let layout = settings::layout(&model, lw, lh);
        model.focus = layout.rows.iter().position(|(row, _)| row.field.is_some());
        let layout = settings::layout(&model, lw, lh);

        if std::env::var("ZOUWU_DUMP").is_ok() {
            println!("-- rows. h={:.0} content={:?}", lh, layout.content);
            for (row, rl) in &layout.rows {
                let text = match &row.control {
                    settings::Control::Section { text } => format!("SECTION {text}"),
                    _ => row.label.clone(),
                };
                println!(
                    "  y={:6.1} h={:5.1} bottom={:6.1}  {}",
                    rl.rect.y,
                    rl.rect.h,
                    rl.rect.bottom(),
                    text
                );
            }
        }

        let mut pixmap =
            Pixmap::new((lw * scale) as u32, (lh * scale) as u32).ok_or("pixmap alloc failed")?;
        settings::draw(&mut pixmap, &layout, &model);

        let file = if (scale - 2.0).abs() < f32::EPSILON {
            file.to_string()
        } else {
            file.replace(".png", &format!("@{scale}x.png"))
        };
        let path = out.join(file);
        pixmap.save_png(&path).map_err(|e| e.to_string())?;
        println!("{}", path.display());
    }
    Ok(())
}
