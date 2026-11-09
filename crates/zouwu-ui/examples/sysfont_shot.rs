//! 把「系统字体」小窗离屏渲染成 PNG。
//!
//! 与 `settings_shot.rs` 同理：本机（远程会话/无合成器）拿不到实时截图，
//! 而这一窗全是文字和列表，**必须看见**才能验收行高、截断、滚动条对不对。
//!
//! 顺带验证两件事：字体列表是真的从系统枚举出来的（不是假数据），
//! 选中行、滚动条、按钮在两种主题下都画得出来。

use std::path::PathBuf;

use tiny_skia::Pixmap;
use zouwu_core::i18n::I18n;
use zouwu_core::theme::Theme;
use zouwu_ui::sysfont_window::{self, Hit, State};

fn main() -> Result<(), String> {
    zouwu_platform::logging::init();

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("zouwu-settings");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;

    let language = std::env::var("ZOUWU_LANG").unwrap_or_else(|_| "zh-CN".to_string());
    let i18n = I18n::new(&language);

    // 真状态：真实枚举出来的字体 + 真实读到的当前系统字体。
    let mut state = State::new();
    eprintln!(
        "{} faces, current {:?}, admin {}, substituted {:?}",
        state.faces.len(),
        state
            .snapshot
            .as_ref()
            .and_then(|s| s.get(zouwu_platform::sysfont::Target::Menu))
            .map(|e| (e.face.clone(), e.height)),
        state.admin,
        state.substituted
    );
    if state.faces.is_empty() {
        return Err("no faces enumerated — nothing to draw".to_string());
    }

    // 演示态：选一个真的装了的字体 + 12 pt 加粗，比默认 9 pt 更能看出字号是不是真的生效。
    let demo = std::env::var("ZOUWU_FACE").unwrap_or_default();
    if !demo.is_empty() {
        if let Some(i) = state
            .faces
            .iter()
            .position(|f| f.eq_ignore_ascii_case(&demo))
        {
            state.selected = i;
        }
    }
    state.pt = 12.0;
    state.bold = true;
    // 指定了 ZOUWU_FACE 就只验收那一个字体；否则演示"滚动条 + 选中行"。
    if demo.is_empty() {
        state.scroll = 3.min(state.faces.len().saturating_sub(1));
        state.selected = (state.scroll + 2).min(state.faces.len() - 1);
    }
    state.status = Some(i18n.t_args(
        "sysfont.applied",
        &[
            ("face", state.face().unwrap_or_default()),
            ("path", "backup-2026-11-09T101500.000.toml"),
        ],
    ));

    // `ZOUWU_SYSFONT_ROUNDTRIP=1`：把**当前值原样写回去**，验证 SPI 写 + 广播这条路
    // 真的通（不改变任何可见效果，所以可以随便跑）。真正的"换个字体"由用户点按钮完成。
    // `ZOUWU_SYSFONT_RESTORE=<备份文件>`：从指定快照还原。窗口里的「还原上次」用的是
    // 最新那份；这个开关是给"摸规则时留下的中间状态"收尾用的。
    // 只读：把 `NONCLIENTMETRICS` 里我们关心的字段打印一行。排"哪个字段在变"用。
    if std::env::var_os("ZOUWU_SYSFONT_DUMP").is_some() {
        eprintln!(
            "{}",
            zouwu_platform::sysfont::metrics_dump().unwrap_or_else(|| "cannot read".into())
        );
        return Ok(());
    }

    // `ZOUWU_SYSFONT_FIX_CHROME=1`：把边框/滚动条/标题栏宽度恢复成标准尺寸。
    // 这是给“被旧版本整块乘系数放大过”的机器收尾用的（ADR-066）。
    if std::env::var_os("ZOUWU_SYSFONT_FIX_CHROME").is_some() {
        let before = zouwu_platform::sysfont::metrics_dump().unwrap_or_else(|| "?".into());
        let chrome =
            zouwu_platform::sysfont::restore_chrome().map_err(|e| format!("fix failed: {e}"))?;
        eprintln!("before: {before}");
        eprintln!("chrome -> {chrome:?}");
        eprintln!(
            "{}",
            zouwu_platform::sysfont::metrics_dump().unwrap_or_else(|| "cannot read back".into())
        );
        return Ok(());
    }

    if let Ok(path) = std::env::var("ZOUWU_SYSFONT_RESTORE") {
        let snapshot = zouwu_platform::sysfont::read_snapshot(std::path::Path::new(path.trim()))
            .ok_or_else(|| format!("cannot read {}", path.trim()))?;
        zouwu_platform::sysfont::restore(&snapshot).map_err(|e| format!("restore failed: {e}"))?;
        let after = zouwu_platform::sysfont::current().ok_or("cannot read back")?;
        for target in zouwu_platform::sysfont::Target::ALL {
            eprintln!(
                "{}: {:?} -> {:?}",
                target.id(),
                snapshot.get(target).map(|e| e.height),
                after.get(target).map(|e| e.height)
            );
        }
        return Ok(());
    }

    if std::env::var("ZOUWU_SYSFONT_ROUNDTRIP").is_ok() {
        let Some(before) = zouwu_platform::sysfont::current() else {
            return Err("cannot read current system font".to_string());
        };
        // 标题栏那一对**不在**我们要改的五项里，所以它是"整块缩放有没有补对"的
        // 唯一证人：SPI 的 SET 会把整块长度按 DPI 除一次，补不回原值的话它会
        // 每写一次缩 20%，最后整条标题栏缩成几个像素（真出过这事）。
        let caption_before = zouwu_platform::sysfont::caption_metrics();
        let entries: Vec<_> = zouwu_platform::sysfont::Target::ALL
            .into_iter()
            .filter_map(|t| before.get(t).map(|e| (t, e.clone())))
            .collect();
        let path = zouwu_platform::sysfont::save_snapshot(&before);
        zouwu_platform::sysfont::apply(&entries).map_err(|e| format!("apply failed: {e}"))?;
        let after = zouwu_platform::sysfont::current().ok_or("cannot read back")?;
        let caption_after = zouwu_platform::sysfont::caption_metrics();
        if caption_before != caption_after {
            return Err(format!(
                "caption metrics drifted: {caption_before:?} -> {caption_after:?}"
            ));
        }
        eprintln!("caption (font, height) stayed {caption_after:?}");
        for target in zouwu_platform::sysfont::Target::ALL {
            let a = before.get(target);
            let b = after.get(target);
            if a != b {
                return Err(format!(
                    "{} did not round-trip: {a:?} -> {b:?}",
                    target.id()
                ));
            }
        }
        eprintln!(
            "roundtrip ok ({} targets, snapshot {:?})",
            entries.len(),
            path.map(|p| p.display().to_string())
        );
    }

    for (theme_id, file, hover) in [
        ("zouwu-warm", "sysfont-light.png", None),
        ("zouwu-dark", "sysfont-dark.png", Some(Hit::Substitute)),
    ] {
        let theme = Theme::by_name(theme_id);
        let scale = 2.0f32;
        let mut pixmap = Pixmap::new(
            (sysfont_window::WIDTH * scale) as u32,
            (sysfont_window::HEIGHT * scale) as u32,
        )
        .ok_or("cannot allocate pixmap")?;
        sysfont_window::draw(&mut pixmap, &state, &i18n, &theme, hover);
        let path = out.join(file);
        pixmap.save_png(&path).map_err(|e| e.to_string())?;
        println!("{}", path.display());
    }
    println!(
        "window is {:.0}×{:.0} logical px",
        sysfont_window::WIDTH,
        sysfont_window::HEIGHT
    );
    Ok(())
}
