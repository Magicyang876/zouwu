//! 按住 Alt 拖动窗口（Linux/KDE）。
//!
//! ## 为什么不是自己实现
//!
//! Windows 版的 `altdrag` 是一个全局鼠标钩子 + `SetWindowPos` 循环。Wayland 下这
//! 两件事都做不到：客户端拿不到全局指针抓取，也不能移动别人的窗口。唯一能做的是
//! 请合成器代劳——KWin 本来就有"按住 Meta 拖动任何窗口"的内置能力，配置项在
//! `kwinrc` 的 `[MouseBindings]` 段：
//!
//! ```ini
//! CommandAllKey=Meta             ; 拖动/缩放/调整大小用的那个修饰键
//! CommandAll1=Move              ; 按住该键 + 拖动左键 = 移动窗口
//! CommandAllWheel=Maximize/Restore ; 该键 + 滚轮：上滚最大化、下滚还原
//! CommandAll2=Minimize           ; 该键 + 中键 = 最小化
//! ```
//!
//! 所以这里做的是"把 KWin 的这几项能力切到 Alt，并在关掉时还原成原值"。
//! 好处是行为与 KDE 完全一致（贴边、贴磁、多屏、键盘修饰都跟着用户自己的设置走），
//! 坏处是修改了用户的桌面配置——因此必须**记下原值**、退出时还原，哪怕程序被
//! 强杀也要在下次启动时自愈（见 [`heal`]）。
//!
//! ## 和 Windows 版的差别
//!
//! `CommandAllWheel` 只有 `maximize/restore`，没有 `minimize`（KWin 的
//! `Options::mouseWheelCommand` 就这几个值），所以"滚轮下 = 最小化"只能落成
//! "滚轮下 = 还原"；最小化改由 Alt+中键（`CommandAll2`）承担。
//! "双击最大化"做不到：带修饰键的双击没有任何 KWin 配置项能接到，而 Wayland
//! 下客户端也抓不到全局指针（这点和 KWin 自己的 Meta 拖动一样，只能由合成器做）。
//! 标题栏双击最大化是 KWin 自带的（`[Windows] TitlebarDoubleClickCommand`），
//! 不需要按 Alt。

use std::io::Write;
use std::path::{Path, PathBuf};

const GROUP: &str = "MouseBindings";
/// 修饰键：KDE 的取值是 `Meta` / `Alt` / `Ctrl` / `Shift`。
const KEY: &str = "CommandAllKey";
/// 左键动作：`Move` = 移动窗口（其它常见值有 `Resize`/`Close`/`Lower`…）。
const MOVE_KEY: &str = "CommandAll1";
const WANT_KEY: &str = "Alt";
const WANT_MOVE: &str = "Move";
/// 滚轮：KWin 只认 `raise/lower`、`maximize/restore`、`above/below`、
/// `previous/next desktop`、`change opacity`、`nothing`。
const WHEEL_KEY: &str = "CommandAllWheel";
const WANT_WHEEL: &str = "Maximize/Restore";
/// 中键：`Minimize` 就是 KWin 自己的最小化动作，改由这个手势触发。
const MIDDLE_KEY: &str = "CommandAll2";
const WANT_MIDDLE: &str = "Minimize";
/// 原值为空（这个键本来就没设置过）时，还原的动作是"删掉这一行"。
const ABSENT: &str = "-";
/// KWin 用 Qt 键码报当前生效的修饰键：`Qt::Key_Meta = 0x01000022`、
/// `Qt::Key_Alt = 0x01000023`。写进 `kwinrc` 之后可以用它们反查
/// "到底生效没有"（见 [`runtime_mod_key`]）。
const QT_KEY_ALT: u32 = 0x0100_0023;

// ------------------------------------------------------------------ 公开 API

/// 打开/关闭。
pub fn apply(enabled: bool) -> Result<(), String> {
    // 先自愈：上次如果是被强杀的，磁盘上还留着"我们改过"的标记。
    heal();
    if !enabled {
        restore()?;
        return Ok(());
    }
    let path = kwinrc();
    let text = read(&path)?;
    if is_active() && has_wanted_bindings(&text) {
        // 已经改过了（比如上次运行时留下的）：不再写盘，但还得确认 KWin 手里
        // 真的是 Alt——kwinrc 改好了而 KWin 没重读的可能性是存在的。
        match runtime_mod_key() {
            Some(QT_KEY_ALT) => {
                log::debug!("alt drag: already applied, KWin has Alt");
                return Ok(());
            }
            Some(other) => log::warn!(
                "alt drag: kwinrc says Alt but KWin still reports {other}; re-applying"
            ),
            // 问不到就当它好好的，别为了一个拿不准的判断反复改人家的配置。
            None => return Ok(()),
        }
    }
    if !kwin_running() {
        return Err("alt drag needs KWin (KDE Plasma)".to_string());
    }
    // 原值只记一次：标记已存在时以它为准（否则升级后新增的手势会把我们的
    // 值当成"原值"记下来），没有标记时按当前 kwinrc 记。
    let saved = match read_marker() {
        Some(saved) => saved,
        None => {
            let saved = Saved::capture(&text);
            // 先落盘标记，再改配置：反过来的话"改完但没记"会永远还不了原。
            write_marker(&saved)?;
            saved
        }
    };

    let mut text = text;
    text = update(&text, GROUP, KEY, Some(WANT_KEY));
    // 只在用户没把左键绑成别的东西时才接管（尊重既有设置）。
    if saved.move_action == ABSENT || saved.move_action == WANT_MOVE {
        text = update(&text, GROUP, MOVE_KEY, Some(WANT_MOVE));
    }
    text = update(&text, GROUP, WHEEL_KEY, Some(WANT_WHEEL));
    text = update(&text, GROUP, MIDDLE_KEY, Some(WANT_MIDDLE));
    write(&path, &text)?;
    reconfigure()?;
    log::info!(
        "alt drag: {KEY}={WANT_KEY}, {MOVE_KEY}={WANT_MOVE}, \
         {WHEEL_KEY}={WANT_WHEEL}, {MIDDLE_KEY}={WANT_MIDDLE} written to kwinrc"
    );
    // 写文件 + reconfigure 只是"请求"，KWin 真收下了才算数。它把自己的运行时
    // 选项挂在 supportInformation 里，顺手读一眼就能把"改了半天没生效"从
    // 静默失败变成一条日志。
    //
    // 但 reconfigure 是异步的（NoReplyExpected），刚发完那一瞬间 KWin 还是旧值
    // ——实测 1~20ms 后就变过来了。所以这里等一会儿再判，别谎报"要重启 KWin"。
    let mut live = runtime_mod_key();
    for _ in 0..15 {
        if live == Some(QT_KEY_ALT) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(80));
        live = runtime_mod_key();
    }
    match live {
        Some(QT_KEY_ALT) => log::info!("alt drag: KWin reports the modifier is live (Alt)"),
        Some(other) => log::warn!(
            "alt drag: KWin still reports modifier key {other} (want {QT_KEY_ALT}); \
             restart KWin for it to take effect"
        ),
        None => log::debug!("alt drag: KWin did not report its modifier key"),
    }
    Ok(())
}

/// 还原（幂等）。
pub fn stop() {
    if let Err(err) = restore() {
        log::warn!("alt drag: restore failed: {err}");
    }
}

pub fn shutdown() {
    stop();
}

/// 是否处于"我们把它打开着"的状态。
pub fn is_active() -> bool {
    marker_path().exists()
}

// ------------------------------------------------------------------ 实现

struct Saved {
    key: String,
    move_action: String,
    wheel: String,
    middle: String,
}

impl Saved {
    fn capture(text: &str) -> Self {
        let own = |key: &str| get(text, GROUP, key).unwrap_or_else(|| ABSENT.to_string());
        Saved {
            key: own(KEY),
            move_action: own(MOVE_KEY),
            wheel: own(WHEEL_KEY),
            middle: own(MIDDLE_KEY),
        }
    }
}

/// kwinrc 里已经是我们要的样子了吗（升级后多出来的手势能靠这个补上）。
fn has_wanted_bindings(text: &str) -> bool {
    [
        (KEY, WANT_KEY),
        (WHEEL_KEY, WANT_WHEEL),
        (MIDDLE_KEY, WANT_MIDDLE),
    ]
    .iter()
    .all(|(key, want)| get(text, GROUP, key).as_deref() == Some(*want))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn kwinrc() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("kwinrc")
}

/// 记着"改过什么"的标记文件。放 zouwu 自己的配置目录，不污染别人的位置。
fn marker_path() -> PathBuf {
    crate::config_dir().join("altdrag.state")
}

fn read(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(format!("read {}: {err}", path.display())),
    }
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

fn write_marker(saved: &Saved) -> Result<(), String> {
    let path = marker_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let body = format!(
        "key={}\nmove={}\nwheel={}\nmiddle={}\n",
        saved.key, saved.move_action, saved.wheel, saved.middle
    );
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))
}

fn read_marker() -> Option<Saved> {
    let text = std::fs::read_to_string(marker_path()).ok()?;
    let mut saved = Saved {
        key: ABSENT.to_string(),
        move_action: ABSENT.to_string(),
        wheel: ABSENT.to_string(),
        middle: ABSENT.to_string(),
    };
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("key=") {
            saved.key = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("move=") {
            saved.move_action = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("wheel=") {
            saved.wheel = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("middle=") {
            saved.middle = value.trim().to_string();
        }
    }
    Some(saved)
}

/// 还原成原值并删掉标记。
fn restore() -> Result<(), String> {
    let Some(saved) = read_marker() else {
        return Ok(());
    };
    let path = kwinrc();
    let mut text = read(&path)?;
    text = update(&text, GROUP, KEY, opt(&saved.key));
    text = update(&text, GROUP, MOVE_KEY, opt(&saved.move_action));
    text = update(&text, GROUP, WHEEL_KEY, opt(&saved.wheel));
    text = update(&text, GROUP, MIDDLE_KEY, opt(&saved.middle));
    // 整段都是我们造出来的话，段头也一并收走：不要在用户的 kwinrc 里留空壳。
    text = prune_empty_group(&text, GROUP);
    write(&path, &text)?;
    reconfigure()?;
    let _ = std::fs::remove_file(marker_path());
    log::info!("alt drag: restored {KEY}={}", saved.key);    Ok(())
}

/// 有标记但当前值已经不是我们写的了（用户自己改过、或改完没生效）→ 清掉标记，
/// 别在下一次启动时把用户当前的设置覆盖回"原值"。
fn heal() {
    let Some(saved) = read_marker() else {
        return;
    };
    let text = read(&kwinrc()).unwrap_or_default();
    let current = get(&text, GROUP, KEY);
    if current.as_deref() != Some(WANT_KEY) {
        log::info!(
            "alt drag: stale marker (current {KEY}={current:?}), dropping it"
        );
        let _ = std::fs::remove_file(marker_path());
    } else {
        // 仍然是我们设的值：说明上次没走到还原，保持标记，等 apply(false) 处理。
        let _ = saved;
    }
}

fn opt(value: &str) -> Option<&str> {
    (value != ABSENT).then_some(value)
}

/// 读 KWin 当前生效的修饰键（Qt 键码）。
fn runtime_mod_key() -> Option<u32> {
    let conn = zbus::blocking::Connection::session().ok()?;
    let reply = conn
        .call_method(
            Some("org.kde.KWin"),
            "/KWin",
            Some("org.kde.KWin"),
            "supportInformation",
            &(),
        )
        .ok()?;
    let text: String = reply.body().deserialize().ok()?;
    parse_mod_key(&text)
}

/// 从 `supportInformation` 的报告里挑出 `keyCmdAllModKey: <数字>`。
fn parse_mod_key(report: &str) -> Option<u32> {
    report.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == "keyCmdAllModKey")
            .then(|| value.trim().parse::<u32>().ok())
            .flatten()
    })
}

fn kwin_running() -> bool {
    let Ok(conn) = zbus::blocking::Connection::session() else {
        return false;
    };
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &("org.kde.KWin",),
    )
    .ok()
    .and_then(|reply| reply.body().deserialize::<bool>().ok())
    .unwrap_or(false)
}

/// 让 KWin 重读 `kwinrc`。
///
/// 这个方法在 KDE 的 XML 里标了 `Q_NOREPLY`：**不会**有回信，用
/// `call_method` 会一直等到超时，所以只能"发出去就不管"。
fn reconfigure() -> Result<(), String> {
    use std::num::NonZeroU32;
    use std::sync::atomic::{AtomicU32, Ordering};

    // 只在这条连接上发这一条消息，序列号自增即可（没有回信，也就没有等待表）。
    static SERIAL: AtomicU32 = AtomicU32::new(1);
    let serial = NonZeroU32::new(SERIAL.fetch_add(1, Ordering::Relaxed)).unwrap_or(NonZeroU32::MIN);

    let conn = zbus::blocking::Connection::session().map_err(|e| format!("D-Bus session: {e}"))?;
    let message = zbus::Message::method_call("/KWin", "reconfigure")
        .and_then(|builder| builder.destination("org.kde.KWin"))
        .and_then(|builder| builder.interface("org.kde.KWin"))
        .and_then(|builder| builder.with_flags(zbus::message::Flags::NoReplyExpected))
        .map_err(|e| format!("build reconfigure: {e}"))?
        .serial(serial)
        .build(&())
        .map_err(|e| format!("build reconfigure: {e}"))?;
    conn.send(&message)
        .map_err(|e| format!("send reconfigure: {e}"))?;
    Ok(())
}

// ------------------------------------------------------------------ ini 编辑
//
// KConfig 文件就是 ini：`[Group]` 分段 + `key=value`。这里只做"改/删一个键"，
// 够用且不依赖 `kwriteconfig6`（那个工具在最小安装里可能没有）。

fn split_group<'a>(lines: &'a [String], group: &str) -> Option<(usize, usize)> {
    let header = format!("[{group}]");
    let start = lines
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case(&header))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.trim_start().starts_with('['))
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    Some((start, end))
}

fn get(text: &str, group: &str, key: &str) -> Option<String> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end) = split_group(&lines, group)?;
    for line in &lines[start + 1..end] {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case(key) {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// 把 `group` 段里的 `key` 设成 `value`（`None` = 删掉这行）。
fn update(text: &str, group: &str, key: &str, value: Option<&str>) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header = format!("[{group}]");
    let Some((start, end)) = split_group(&lines, group) else {
        // 整段都没有：追加到文件尾。
        let Some(value) = value else {
            return text.to_string();
        };
        if !lines.is_empty() && !lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(header);
        lines.push(format!("{key}={value}"));
        return finish(lines);
    };
    let existing = (start + 1..end).find(|index| {
        lines[*index]
            .split_once('=')
            .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(key))
    });
    match (existing, value) {
        (Some(index), Some(value)) => lines[index] = format!("{key}={value}"),
        (Some(index), None) => {
            lines.remove(index);
        }
        (None, Some(value)) => lines.insert(end, format!("{key}={value}")),
        (None, None) => {}
    }
    finish(lines)
}

/// `group` 段里一句有效配置都没有了 → 连段头一起删掉，并收掉因此多出来的空行。
fn prune_empty_group(text: &str, group: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let Some((start, end)) = split_group(&lines, group) else {
        return text.to_string();
    };
    if lines[start + 1..end].iter().any(|line| !line.trim().is_empty()) {
        return text.to_string();
    }
    lines.drain(start..end);
    // 文件尾的空行是垃圾；中间连着两行空白合成一行。
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        if line.trim().is_empty() && out.last().is_some_and(|prev| prev.trim().is_empty()) {
            continue;
        }
        out.push(line);
    }
    finish(out)
}

fn finish(lines: Vec<String>) -> String {
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// 写日志用：改完的段长什么样（debug 级别，出问题时能自证）。
#[allow(dead_code)]
fn dump_group(path: &Path, group: &str) -> String {
    let text = read(path).unwrap_or_default();
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    match split_group(&lines, group) {
        Some((start, end)) => lines[start..end].join("\n"),
        None => String::new(),
    }
}

#[allow(dead_code)]
fn ensure_dir(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(b"")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{get, update, ABSENT};

    const SAMPLE: &str = "[General]\nfoo=1\n\n[MouseBindings]\nCommandAllKey=Meta\nCommandAll1=Move\n\n[Plugins]\nbar=2\n";

    #[test]
    fn reads_existing_key() {
        assert_eq!(get(SAMPLE, "MouseBindings", "CommandAllKey").as_deref(), Some("Meta"));
        assert_eq!(get(SAMPLE, "MouseBindings", "Nope"), None);
        assert_eq!(get(SAMPLE, "Missing", "CommandAllKey"), None);
    }

    #[test]
    fn replaces_without_touching_other_groups() {
        let out = update(SAMPLE, "MouseBindings", "CommandAllKey", Some("Alt"));
        assert!(out.contains("CommandAllKey=Alt"));
        assert!(!out.contains("CommandAllKey=Meta"));
        // 别的段一个字都不能动。
        assert!(out.contains("[General]\nfoo=1"));
        assert!(out.contains("[Plugins]\nbar=2"));
        // 幂等。
        assert_eq!(update(&out, "MouseBindings", "CommandAllKey", Some("Alt")), out);
    }

    #[test]
    fn restores_by_deleting_the_line() {
        let out = update(SAMPLE, "MouseBindings", "CommandAllKey", None);
        assert!(!out.contains("CommandAllKey"));
        assert!(out.contains("CommandAll1=Move"), "不能顺手删掉别的键");
    }

    #[test]
    fn creates_group_when_missing() {
        let out = update("[General]\nfoo=1\n", "MouseBindings", "CommandAllKey", Some("Alt"));
        assert!(out.contains("[MouseBindings]\nCommandAllKey=Alt"));
        assert!(out.starts_with("[General]\nfoo=1"));
    }

    /// 首次开启时整段是我们造出来的，关掉时不能留下空的 `[MouseBindings]`。
    #[test]
    fn empty_group_is_removed_again() {
        let original = "[General]\nfoo=1\n";
        let on = update(original, "MouseBindings", "CommandAllKey", Some("Alt"));
        let back = update(&on, "MouseBindings", "CommandAllKey", None);
        assert!(back.contains("[MouseBindings]"), "还没 prune");
        assert_eq!(super::prune_empty_group(&back, "MouseBindings"), original);
    }

    #[test]
    fn absent_marker_means_delete() {
        let out = update(SAMPLE, "MouseBindings", "CommandAllKey", super::opt(ABSENT));
        assert!(!out.contains("CommandAllKey"));
    }

    /// KWin 的报告里，当前的修饰键就是这一行。
    #[test]
    fn parses_the_live_modifier_key() {
        let report = "snapOnlyWhenOverlapping: false\ncommandAll1: MouseUnrestrictedMove\n\
                      keyCmdAllModKey: 16777251\ndoubleClickBorderToMaximize: true\n";
        assert_eq!(super::parse_mod_key(report), Some(0x0100_0023));
        // 没有这一行（老版本 KWin）不能报错，只是查不到。
        assert_eq!(super::parse_mod_key("commandAll1: MouseUnrestrictedMove\n"), None);
        // Meta 是 KDE 的默认值，和 Alt 必须区分开。
        assert_ne!(super::parse_mod_key("keyCmdAllModKey: 16777250\n"), Some(0x0100_0023));
    }

    /// 开 → 关必须回到原文：四个键都是同一段里原地改 / 新增 / 删行。
    #[test]
    fn round_trip_returns_the_original() {
        let original = SAMPLE;
        let ours = [
            ("CommandAllKey", Some("Alt")),
            ("CommandAll1", Some("Move")),
            ("CommandAllWheel", Some("Maximize/Restore")),
            ("CommandAll2", Some("Minimize")),
        ];
        let mut text = original.to_string();
        for (key, value) in ours {
            text = update(&text, "MouseBindings", key, value);
        }
        assert!(text.contains("CommandAllWheel=Maximize/Restore"));
        assert!(text.contains("CommandAll2=Minimize"));
        // 原值：CommandAllKey=Meta、CommandAll1=Move，另外两个本来没有。
        text = update(&text, "MouseBindings", "CommandAllKey", Some("Meta"));
        text = update(&text, "MouseBindings", "CommandAll1", Some("Move"));
        text = update(&text, "MouseBindings", "CommandAllWheel", None);
        text = update(&text, "MouseBindings", "CommandAll2", None);
        assert_eq!(text, original);
    }

    /// 用户自己在这一段里有别的东西时，段头必须留着。
    #[test]
    fn group_with_other_keys_survives() {
        let text = "[MouseBindings]\nCommandAll1=Move\n";
        assert_eq!(super::prune_empty_group(text, "MouseBindings"), text);
    }
}