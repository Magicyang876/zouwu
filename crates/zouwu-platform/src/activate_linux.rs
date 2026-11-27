//! 把刚打开的面板窗口抬到最前（仅 KDE/KWin）。
//!
//! Wayland 没有"把自己的窗口顶上来"的接口：`xdg_toplevel` 里没有 raise，
//! `set_visible(true)` 只是重新 map，Z 序由合成器说了算。实测 KWin 会把重新
//! map 的面板压在别的窗口后面——用户看到的现象是"打开了、却看不见，也点不到，
//! 于是关不掉"。
//!
//! `xdg_activation_v1` 是"正确"的路，但它要一个来⾃**本客户端**最近一次输入事件
//! 的 serial；从托盘菜单（点在 plasmashell 上）或 D-Bus 触发时我们手里没有，
//! KWin 会直接拒绝。
//!
//! 所以这里退一步，用 KWin 自己的脚本接口：启动时装一次脚本，之后每次开面板
//! 调一次 `run()`，让 KWin 在每个输出上"激活"属于我们的窗口——这条是实际验证
//! 有效的（`workspace.activeWindow = w` 会让窗口抬到最前并获得键盘焦点，于是
//! Esc 和标题栏的关闭按钮都够得着）。
//!
//! 不是 KDE 就静默什么都不做：其它合成器各有各的规矩，宁可不开面板也不乱动。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use zbus::blocking::Connection;

const SERVICE: &str = "org.kde.KWin";
const SCRIPTING_PATH: &str = "/Scripting";
const SCRIPTING_IFACE: &str = "org.kde.kwin.Scripting";
const SCRIPT_IFACE: &str = "org.kde.kwin.Script";

/// 装好的脚本 id（`/Scripting/Script<id>`）。`None` = 没装/装不上。
static SCRIPT: Mutex<Option<i32>> = Mutex::new(None);

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from)
}

fn script_path() -> PathBuf {
    runtime_dir().join("zouwu-activate.js")
}

/// 脚本正文。
///
/// 只碰"我们自己的、有标题的普通窗口"：小兽本体（layer 表面）标题是空的，
/// 绝不能去激活它——那会把键盘从用户正在用的窗口抢走。
///
/// 窗口是**异步**注册到合成器的，`run()` 之后新窗口未必已经在 `windowList()`
/// 里，所以带一个短轮询：最多等 800ms，等到就抬。
fn script_body() -> String {
    r#"function zouwuRaise(left) {
    var wins = workspace.windowList().filter(function (w) {
        return w.resourceClass === "zouwu" && w.normalWindow && w.caption !== "";
    });
    if (wins.length === 0) {
        if (left > 0) {
            setTimeout(function () { zouwuRaise(left - 1); }, 100);
        }
        return;
    }
    for (var i = 0; i < wins.length; i++) {
        workspace.activeWindow = wins[i];
    }
}
zouwuRaise(8);
"#
    .to_string()
}

/// 启动时装一次脚本。返回是否装上了。
pub fn install() -> bool {
    if SCRIPT.lock().map(|s| s.is_some()).unwrap_or(false) {
        return true;
    }
    let Ok(conn) = Connection::session() else {
        return false;
    };
    // 不是 KDE 就直接算了。
    let owned: bool = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(SERVICE,),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<bool>().ok())
        .unwrap_or(false);
    if !owned {
        log::debug!("activate: KWin not running, panels will not be raised automatically");
        return false;
    }

    let path = script_path();
    if let Err(err) = std::fs::write(&path, script_body()) {
        log::debug!("activate: cannot write {}: {err}", path.display());
        return false;
    }
    let Some(path_str) = path.to_str().map(str::to_string) else {
        return false;
    };
    let id: Option<i32> = conn
        .call_method(
            Some(SERVICE),
            SCRIPTING_PATH,
            Some(SCRIPTING_IFACE),
            "loadScript",
            &(path_str.as_str(),),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<i32>().ok());
    match id {
        Some(id) => {
            if let Ok(mut slot) = SCRIPT.lock() {
                *slot = Some(id);
            }
            log::debug!("activate: KWin script loaded as Script{id}");
            true
        }
        None => {
            log::debug!("activate: KWin refused to load the raise script");
            false
        }
    }
}

/// 把我们的面板窗口抬到最前（并给它们键盘焦点）。没装脚本时什么都不做。
pub fn raise_panels() {
    let Some(id) = SCRIPT.lock().ok().and_then(|s| *s) else {
        return;
    };
    let Ok(conn) = Connection::session() else {
        return;
    };
    let path = format!("{SCRIPTING_PATH}/Script{id}");
    if let Err(err) = conn.call_method(Some(SERVICE), path.as_str(), Some(SCRIPT_IFACE), "run", &())
    {
        log::debug!("activate: run failed: {err}");
    }
}

/// 一次性脚本的序号：脚本文件名/插件名不能撞车。
static PLACE_SEQ: AtomicU64 = AtomicU64::new(0);

/// JS 字符串字面量（标题是配置/翻译来的，必须转义）。
fn js_quote(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// 请 KWin 把标题为 `caption` 的窗口摆到 **compositor 逻辑坐标** `(x, y)`。
///
/// 为什么必须借合成器的手：Wayland 的 `xdg_toplevel` **没有位置**，
/// winit 的 `set_outer_position()` 在 Wayland 后端是空实现。右键菜单要贴着
/// 小兽长出来，只能让 KWin 的脚本把窗口搬过去。
///
/// 窗口是**异步**注册进 KWin 的，所以脚本先扫一遍 `windowList()`，再挂
/// `windowAdded`；装好后由后台线程在 1.5 秒后卸载，脚本不会常驻。
/// 非 KDE / D-Bus 不可用时返回 false，菜单退回合成器的默认摆放。
pub fn place_window(caption: &str, x: i32, y: i32, width: i32, height: i32) -> bool {
    let Ok(conn) = Connection::session() else {
        return false;
    };
    let seq = PLACE_SEQ.fetch_add(1, Ordering::Relaxed);
    let plugin = format!("zouwu-place-{}-{seq}", std::process::id());
    let path = runtime_dir().join(format!("{plugin}.js"));
    let caption = js_quote(caption);
    let body = format!(
        r#"function zouwuPlace(w) {{
    if (!w || w.caption !== {caption}) {{
        return;
    }}
    // Qt 在 KWin 6 的脚本环境里不可用（`Qt.rect` 会报 "Qt is not defined"），
    // 但 WRITE 属性的 RectF 能直接吃这种普通对象。宽高也写死：窗口刚注册时
    // `frameGeometry` 可能还是默认尺寸，只改 x/y 会让第一次摆放错位。
    w.frameGeometry = {{ x: {x}, y: {y}, width: {width}, height: {height} }};
}}
workspace.windowList().forEach(zouwuPlace);
workspace.windowAdded.connect(zouwuPlace);
"#
    );
    if let Err(err) = std::fs::write(&path, body) {
        log::debug!("activate: cannot write {}: {err}", path.display());
        return false;
    }
    let Some(path_str) = path.to_str().map(str::to_string) else {
        return false;
    };
    let loaded = conn.call_method(
        Some(SERVICE),
        SCRIPTING_PATH,
        Some(SCRIPTING_IFACE),
        "loadScript",
        &(path_str.as_str(), plugin.as_str()),
    );
    let id: i32 = match loaded
        .ok()
        .and_then(|reply| reply.body().deserialize::<i32>().ok())
    {
        Some(id) if id >= 0 => id,
        _ => {
            log::debug!("activate: KWin refused to load the place script");
            let _ = std::fs::remove_file(&path);
            return false;
        }
    };
    // 有些 KWin 版本 `loadScript` 不会立刻执行；重复执行是幂等的。
    let script = format!("{SCRIPTING_PATH}/Script{id}");
    if let Err(err) = conn.call_method(
        Some(SERVICE),
        script.as_str(),
        Some(SCRIPT_IFACE),
        "run",
        &(),
    ) {
        log::debug!("activate: place script run failed: {err}");
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        if let Ok(conn) = Connection::session() {
            let _ = conn.call_method(
                Some(SERVICE),
                SCRIPTING_PATH,
                Some(SCRIPTING_IFACE),
                "unloadScript",
                &(plugin.as_str(),),
            );
        }
        let _ = std::fs::remove_file(&path);
    });
    true
}

/// 退出时停掉脚本。
pub fn shutdown() {
    let Some(id) = SCRIPT.lock().ok().and_then(|s| *s) else {
        return;
    };
    if let Ok(conn) = Connection::session() {
        let path = format!("{SCRIPTING_PATH}/Script{id}");
        let _ = conn.call_method(
            Some(SERVICE),
            path.as_str(),
            Some(SCRIPT_IFACE),
            "stop",
            &(),
        );
    }
    if let Ok(mut slot) = SCRIPT.lock() {
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_strings_are_escaped() {
        assert_eq!(js_quote("形象"), "\"形象\"");
        assert_eq!(js_quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn script_targets_only_titled_windows_of_ours() {
        let body = script_body();
        assert!(body.contains("resourceClass === \"zouwu\""));
        // 小兽本体标题是空的，必须排除，否则会把键盘从用户手里抢走。
        assert!(body.contains("w.caption !== \"\""));
        assert!(body.contains("normalWindow"));
    }
}
