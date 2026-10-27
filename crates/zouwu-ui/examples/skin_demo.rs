//! 皮肤加载器的端到端自检：**从 PNG 字节到像素**，全程不碰磁盘、不碰窗口。
//!
//! ```bash
//! cargo run --release -p zouwu-ui --example skin_demo
//! ```
//!
//! 它做三件事：
//!
//! 1. 现场用 tiny-skia 画一张两层皮肤的 PNG（身体 + 闭眼），
//!    拼成 `SkinPackage` 走一遍真正的解码路径（`Skin::decode`）；
//! 2. 用这张皮肤渲染 常态 / 展开 / 眨眼 三张图，写到
//!    `target/zouwu-skin-demo/`，肉眼可查；
//! 3. 量一下"位图皮肤 vs 代码绘制"的单帧成本——skin.rs 里那张表的数字出处。
//!
//! 为什么不用现成的皮肤文件：**加载器最怕的是"从字节开始就对"**这件事没被测过。
//! 这里造的就是最朴素的两层 PNG，没有磁盘、没有配置、没有 DPI 的干扰项。

use std::path::PathBuf;
use std::time::Instant;

use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};
use zouwu_core::skin::{LayerRole, SkinManifest};
use zouwu_core::theme::Theme;
use zouwu_platform::assets::SkinPackage;
use zouwu_ui::paint::{render, Scene};
use zouwu_ui::skin::Skin;

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("zouwu-skin-demo");
    std::fs::create_dir_all(&dir).expect("create output dir");
    dir
}

/// 画一张"剪纸风"的驺吾：大圆身体 + 两只尖耳 + 尾巴 + 两点眼睛。
///
/// 故意做成**不依赖主题色**的纯色，这样一眼就能看出"这张图是从文件来的"
/// 而不是代码绘制那只（后者会跟着主题变色）。
fn paint_body(size: u32) -> Vec<u8> {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let s = size as f32;
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };

    // 尾巴
    paint.set_color(Color::from_rgba8(196, 132, 74, 255));
    let mut pb = PathBuilder::new();
    pb.move_to(s * 0.22, s * 0.62);
    pb.quad_to(s * 0.04, s * 0.50, s * 0.12, s * 0.30);
    if let Some(path) = pb.finish() {
        pm.stroke_path(
            &path,
            &paint,
            &Stroke {
                width: s * 0.09,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }

    // 耳朵
    paint.set_color(Color::from_rgba8(222, 160, 96, 255));
    for k in [-1.0f32, 1.0] {
        let mut pb = PathBuilder::new();
        pb.move_to(s * (0.50 + 0.16 * k), s * 0.34);
        pb.line_to(s * (0.50 + 0.26 * k), s * 0.12);
        pb.line_to(s * (0.50 + 0.04 * k), s * 0.26);
        pb.close();
        if let Some(path) = pb.finish() {
            pm.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    // 身体
    paint.set_color(Color::from_rgba8(232, 152, 84, 255));
    if let Some(path) = PathBuilder::from_circle(s * 0.5, s * 0.55, s * 0.30) {
        pm.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    // 眼睛（睁眼：两个实心圆）
    paint.set_color(Color::from_rgba8(48, 34, 28, 255));
    for k in [-1.0f32, 1.0] {
        if let Some(path) = PathBuilder::from_circle(s * (0.5 + 0.10 * k), s * 0.53, s * 0.035) {
            pm.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    pm.encode_png().expect("encode body")
}

/// 睁眼层之上的"闭眼"叠加图：两条弯弧 + 透明背景。
fn paint_blink(size: u32) -> Vec<u8> {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let s = size as f32;
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(Color::from_rgba8(48, 34, 28, 255));

    for k in [-1.0f32, 1.0] {
        let mut pb = PathBuilder::new();
        let cx = s * (0.5 + 0.10 * k);
        pb.move_to(cx - s * 0.05, s * 0.53);
        pb.quad_to(cx, s * 0.565, cx + s * 0.05, s * 0.53);
        if let Some(path) = pb.finish() {
            pm.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width: s * 0.018,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }

    pm.encode_png().expect("encode blink")
}

fn demo_package(size_px: u32) -> SkinPackage {
    let size = size_px;
    let manifest = SkinManifest::parse(
        r#"
id = "demo-papercut"
name = "剪纸驺吾"

[geometry]
radius = 56.0
breath_amount = 0.035

[[layers]]
image = "body.png"

[[layers]]
image = "blink.png"
role = "blink"
"#,
    )
    .expect("built-in demo manifest");

    SkinPackage {
        manifest,
        dir: PathBuf::from("(generated)"),
        images: vec![
            ("body.png".to_string(), paint_body(size)),
            ("blink.png".to_string(), paint_blink(size)),
        ],
    }
}

/// 离屏渲染的 DPI 倍率。
///
/// 默认为 1.0，但**必须能改**：ADR-029 那个「贴图忘乘 DPI scale」的 bug 在
/// 1.0 下永远正确，只有真机上才偏。`ZOUWU_DPI=1.5` 就是把真机搬进离屏渲染。
fn dpi() -> f32 {
    std::env::var("ZOUWU_DPI")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| (0.5..=4.0).contains(v))
        .unwrap_or(1.0)
}

fn shoot(name: &str, skin: &Skin, openness: f32, blink: f32, breath: f32) {
    let dpi = dpi();
    // 逻辑尺寸固定 340，物理画布随 DPI 放大——和真窗口的 inner_size 关系一致。
    let size = (340.0 * dpi).round() as u32;
    let mut pixmap = Pixmap::new(size, size).expect("pixmap");
    // （可选）用 `ZOUWU_THEME=zouwu-dark` 换配色，方便一眼看出"皮肤和主题是两件事"。
    let theme = std::env::var("ZOUWU_THEME")
        .ok()
        .and_then(|id| Theme::builtin(&id))
        .unwrap_or_else(Theme::fallback);
    let scene = Scene {
        width: 340.0,
        height: 340.0,
        scale: dpi,
        buddy_center: zouwu_core::geom::Point::new(170.0, 170.0),
        buddy_radius: 56.0,
        buddy_scale: 1.0,
        buddy_alpha: 1.0,
        blink,
        breath,
        theme: &theme,
        bubbles: &demo_bubbles(openness),
        skin: Some(skin),
    };
    render(&scene, &mut pixmap);
    let path = if (dpi - 1.0).abs() < f32::EPSILON {
        out_dir().join(format!("{name}.png"))
    } else {
        out_dir().join(format!("{name}@{dpi}x.png"))
    };
    pixmap.save_png(&path).expect("save png");
    let visible = pixmap.data().chunks_exact(4).filter(|px| px[3] > 8).count();
    println!("{name}: {} visible px -> {}", visible, path.display());
}

fn demo_bubbles(openness: f32) -> Vec<zouwu_ui::paint::BubblePaint> {
    use zouwu_core::action::default_ring;
    use zouwu_core::geom::{Point, Rect};
    use zouwu_core::ring::{solve_rotation, RingConfig};
    use zouwu_ui::paint::BubblePaint;

    let items = default_ring();
    let ring = RingConfig::default();
    let center = Point::new(170.0, 170.0);
    let view = Rect::new(0.0, 0.0, 340.0, 340.0);
    let rotation = solve_rotation(&ring, center, view, items.len());
    ring.layout(items.len(), center, rotation)
        .into_iter()
        .zip(&items)
        .map(|(bubble, item)| BubblePaint {
            center: bubble.center,
            radius: bubble.radius,
            scale: openness,
            alpha: openness.clamp(0.0, 1.0),
            glyph: item.glyph,
            selected: false,
        })
        .collect()
}

/// 单帧成本：位图皮肤（缩放 / 1:1）vs 代码绘制。
///
/// **为什么要交替测**：这台机器上后台负载会让同一段代码在两次运行间差 1.5×。
/// 顺序地量 A 再来 B，得到的比值基本是噪声。所以这里做的是
/// 「轮流各跑 120 帧 × 6 轮，取每轮最小值再平均」——最小值对调度抖动最不敏感。
fn bench(scaled: &Skin, native: &Skin) {
    const ROUNDS: usize = 6;
    const FRAMES: u32 = 120;

    let size = 340u32;
    let mut pixmap = Pixmap::new(size, size).expect("pixmap");
    let theme = Theme::fallback();
    let bubbles = demo_bubbles(1.0);

    let variants: [(&str, Option<&Skin>); 3] = [
        ("bitmap 256² → 112 px", Some(scaled)),
        ("bitmap 112² → 112 px", Some(native)),
        ("procedural", None),
    ];
    let mut best = [f64::MAX; 3];

    for _ in 0..ROUNDS {
        for (idx, (_label, skin)) in variants.iter().enumerate() {
            let scene = scene_for(&theme, &bubbles, *skin, size);
            // 每轮先热身 10 帧，避免把首次 page fault 算进去。
            for _ in 0..10 {
                render(&scene, &mut pixmap);
            }
            let t0 = Instant::now();
            for _ in 0..FRAMES {
                render(&scene, &mut pixmap);
            }
            let per = t0.elapsed().as_secs_f64() * 1e6 / FRAMES as f64;
            best[idx] = best[idx].min(per);
        }
    }

    println!(
        "
cost per frame (340² canvas, release, best of {ROUNDS}×{FRAMES}):"
    );
    for (idx, (label, _)) in variants.iter().enumerate() {
        println!("  {label:>22}: {:7.1} µs", best[idx]);
    }
    let procedural = best[2];
    for (idx, (label, _)) in variants.iter().enumerate().take(2) {
        println!(
            "  {label:>22}: {:.2}× faster than procedural",
            procedural / best[idx]
        );
    }
}

fn scene_for<'a>(
    theme: &'a Theme,
    bubbles: &'a [zouwu_ui::paint::BubblePaint],
    skin: Option<&'a Skin>,
    size: u32,
) -> Scene<'a> {
    Scene {
        width: size as f32,
        height: size as f32,
        scale: 1.0,
        buddy_center: zouwu_core::geom::Point::new(size as f32 * 0.5, size as f32 * 0.5),
        buddy_radius: 56.0,
        buddy_scale: 1.0,
        buddy_alpha: 1.0,
        blink: 0.0,
        breath: 0.0,
        theme,
        bubbles,
        skin,
    }
}

/// `--install`：把这份示例皮肤真的装进 `%APPDATA%/zouwu/skins/`，
/// 这样就能在**运行时**验证"从磁盘读 PNG → 托盘菜单里出现 → 切换生效"整条链路。
/// （离线示例证明的是解码正确，安装版证明的是路径/配置/菜单也接对了。）
fn install(package: &SkinPackage) {
    // 目录名必须等于 `skin.toml` 里的 id —— 加载器就是这么校验的
    // （否则一个 `foo` 目录里放 `bar` 的图，排查起来是场噩梦）。
    let dir = zouwu_platform::assets::skins_dir().join(&package.manifest.id);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {err}", dir.display());
        return;
    }
    for (name, bytes) in &package.images {
        let path = dir.join(name);
        if let Err(err) = std::fs::write(&path, bytes) {
            eprintln!("write {} failed: {err}", path.display());
            return;
        }
    }
    match std::fs::write(dir.join("skin.toml"), package.manifest.to_toml()) {
        Ok(()) => {
            println!("installed demo skin -> {}", dir.display());
            println!(
                "switch with: appearance.skin = \"{}\"  (or the tray menu)",
                package.manifest.id
            );
        }
        Err(err) => eprintln!("write skin.toml failed: {err}"),
    }
}

/// 直接预览一个**皮肤目录**（作者改完图就能看，不必先装进 `%APPDATA%`）。
///
/// 走的是和运行时完全相同的 `SkinPackage` → `Skin::decode` 路径，
/// 唯一的区别是目录由参数给定、图层文件从磁盘读。
fn load_dir(dir: &std::path::Path) -> Result<SkinPackage, String> {
    let text = std::fs::read_to_string(dir.join("skin.toml"))
        .map_err(|err| format!("{}: {err}", dir.join("skin.toml").display()))?;
    let manifest = SkinManifest::parse(&text)?;
    let mut images = Vec::new();
    for layer in &manifest.layers {
        let path = dir.join(&layer.image);
        let bytes =
            std::fs::read(&path).map_err(|err| format!("layer `{}`: {err}", path.display()))?;
        images.push((layer.image.clone(), bytes));
    }
    Ok(SkinPackage {
        manifest,
        dir: dir.to_path_buf(),
        images,
    })
}

fn main() {
    if let Some(dir) = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--preview")
        .map(|w| PathBuf::from(&w[1]))
    {
        let package = match load_dir(&dir) {
            Ok(pkg) => pkg,
            Err(err) => {
                eprintln!("cannot load {}: {err}", dir.display());
                std::process::exit(1);
            }
        };
        let skin = Skin::decode(&package).expect("decode preview skin");
        println!(
            "previewing `{}` from {}: {} layers",
            skin.manifest.display_name(),
            dir.display(),
            skin.layers.len()
        );
        shoot("preview-collapsed", &skin, 0.0, 0.0, 0.0);
        shoot("preview-expanded", &skin, 1.0, 0.0, 0.0);
        shoot("preview-blink", &skin, 1.0, 1.0, 0.0);
        return;
    }

    let package = demo_package(256);
    if std::env::args().any(|a| a == "--install") {
        install(&package);
    }
    let skin = Skin::decode(&package).expect("decode demo skin");
    println!(
        "decoded `{}`: {} layers",
        skin.manifest.display_name(),
        skin.layers.len()
    );
    assert_eq!(skin.layers.len(), 2);
    assert!(!skin.is_procedural());
    assert_eq!(skin.layers_of(LayerRole::Blink).count(), 1);

    shoot("skin-collapsed", &skin, 0.0, 0.0, 0.0);
    shoot("skin-expanded", &skin, 1.0, 0.0, 0.0);
    shoot("skin-blink", &skin, 1.0, 1.0, 0.0);
    shoot("skin-breath", &skin, 0.0, 0.0, 1.0);

    // 再解一份 1:1 大小的（112 px 长边 = 2 × radius），用来说明"缩放"的代价。
    let native = Skin::decode(&demo_package(112)).expect("decode 1:1 skin");
    bench(&skin, &native);
}
