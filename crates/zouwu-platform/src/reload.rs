//! 配置文件热重载：只做「磁盘变了吗、变成什么了」，应用交给调用方。
//!
//! 为什么不用 `notify`：
//! - 我们要的只是一条 `config.toml`，而 `notify` 会为一个文件起一个后台监视线程
//!   （Windows 上是 `ReadDirectoryChangesW`）。对一个承诺"空闲零开销"的常驻小工具，
//!   这笔常驻线程/句柄成本比一次 `metadata()` 贵得多。
//! - 我们本来就有 100ms 的输入轮询节拍，借它顺手做一次 `metadata()`（~5µs，
//!   再被 500ms 节流）就够了，还省掉"跨线程唤醒 UI"的一整套麻烦。
//!
//! 判定用 `(mtime, len)` 戳而不是"读出来比字符串"：没变的时候连一次读盘都不做。
//! 变了才读、才解析；解析失败**保留旧配置**（用户正在编辑器里敲到一半时，
//! 不能因为半个文件把一只好好的驺吾改坏）。

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use zouwu_core::config::Config;

/// 一次检查的结果。
#[derive(Clone, Debug, PartialEq)]
pub enum ReloadEvent {
    /// 没变（绝大多数时候都是这个）。
    Unchanged,
    /// 文件变了且解析成功：这是要应用的新配置。
    Changed(Box<Config>),
    /// 文件变了但内容不可用（语法错误 / 读不了）。旧配置继续生效。
    Invalid(String),
}

/// 文件的"指纹"：修改时间 + 长度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

/// 配置文件监视器（非阻塞、无后台线程）。
///
/// 内部状态用 `Cell`（而不是 `&mut self`）：调用方经常在 `&self` 的保存路径里
/// 需要"把戳对齐到我刚写的内容"（[`ConfigWatcher::sync`]），用 `Cell` 比把半个
/// `App` 的方法签名都改成 `&mut` 划算。它本身不值钱（两个字段），也不跨线程。
pub struct ConfigWatcher {
    path: PathBuf,
    stamp: Cell<Option<Stamp>>,
    last_check: Cell<Instant>,
    interval: Duration,
    /// 连续"文件不在"的次数。编辑器保存常走"写临时文件 → 删 → 改名"，
    /// 中间那一下不是错误，别每次都喊。
    missing: Cell<u32>,
}

impl ConfigWatcher {
    /// 默认 500ms 检查一次：人改完文件到看见变化，手感上是"立刻"。
    pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(500);

    pub fn new(path: PathBuf) -> Self {
        Self::with_interval(path, Self::DEFAULT_INTERVAL)
    }

    /// 自定义检查间隔（测试里用 `Duration::ZERO` 关掉节流）。
    pub fn with_interval(path: PathBuf, interval: Duration) -> Self {
        let stamp = stamp_of(&path);
        Self {
            path,
            stamp: Cell::new(stamp),
            last_check: Cell::new(Instant::now()),
            interval,
            missing: Cell::new(0),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 检查一次。被 `interval` 节流，所以可以放心地每帧调。
    pub fn poll(&mut self) -> ReloadEvent {
        let now = Instant::now();
        if now.duration_since(self.last_check.get()) < self.interval {
            return ReloadEvent::Unchanged;
        }
        self.last_check.set(now);

        let next = stamp_of(&self.path);
        if next == self.stamp.get() {
            self.missing.set(0);
            return ReloadEvent::Unchanged;
        }

        let Some(_) = next else {
            // 文件不在：可能是编辑器保存的中间态，也可能是用户删了。
            // 都**不动**当前配置，等它回来（最多每 5 次提醒一句）。
            let misses = self.missing.get() + 1;
            self.missing.set(misses);
            if misses == 5 {
                log::warn!(
                    "config file missing ({}); keeping the running config",
                    self.path.display()
                );
            }
            return ReloadEvent::Unchanged;
        };
        self.missing.set(0);
        self.stamp.set(next);

        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) => return ReloadEvent::Invalid(format!("read failed: {err}")),
        };
        match Config::from_toml(&text) {
            Ok(cfg) => {
                let cfg = cfg.migrated();
                // 迁移可能需要补字段：只有真的不同才回报"变了"，
                // 否则每次 touch 都会走一遍应用流程（虽然幂等，但日志很吵）。
                if stamp_of(&self.path) != next {
                    // 读的同时文件又变了（保存中）：等下一轮再处理。
                    return ReloadEvent::Unchanged;
                }
                ReloadEvent::Changed(Box::new(cfg))
            }
            Err(err) => ReloadEvent::Invalid(summarize(&err.to_string())),
        }
    }

    /// 我们刚用原子写覆盖了这个文件：把戳对齐到写入后的状态，
    /// 免得下一次 `poll` 把自己写的内容当成人家的修改再应用一遍。
    pub fn sync(&self) {
        self.stamp.set(stamp_of(&self.path));
        self.last_check.set(Instant::now());
        self.missing.set(0);
    }

    /// 磁盘上是否有我们**还没读过**的改动。只查 metadata，几乎不要钱。
    ///
    /// 用途：在覆盖文件之前留一句日志——用户手改的配置如果被托盘操作悄悄盖掉，
    /// 至少日志里能找到证据（M2 换成 `toml_edit` 的保留注释写回就彻底不冲突了）。
    pub fn has_external_change(&self) -> bool {
        stamp_of(&self.path) != self.stamp.get()
    }
}

/// 把 TOML 的多行报错压成**一行**。
///
/// `toml` 的 Display 会带上三行带插入符的源码片段，在日志里会把
/// "配置没生效"这条关键信息淹没在一片竖线里。我们只留两个有用的东西：
/// 位置（`at line 33, column 14`）和最后那句人话（`unclosed table, expected `]``）。
fn summarize(err: &str) -> String {
    let mut lines = err.lines().map(str::trim).filter(|l| !l.is_empty());
    let head = lines.next().unwrap_or("invalid config");
    let tail = err.lines().map(str::trim).rev().find(|l| {
        !l.is_empty()
            && !l.starts_with('|')
            && !l.contains('^')
            && *l != head
            && !l
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_whitespace() || c == '|')
    });
    match tail {
        Some(tail) => format!("{head}: {tail}"),
        None => head.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "zouwu-reload-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn watcher(path: &Path) -> ConfigWatcher {
        ConfigWatcher::with_interval(path.to_path_buf(), Duration::ZERO)
    }

    #[test]
    fn unchanged_file_reports_nothing() {
        let dir = TempDir::new("unchanged");
        let path = dir.file("config.toml");
        fs::write(&path, Config::default().to_toml()).unwrap();
        let mut w = watcher(&path);
        assert_eq!(w.poll(), ReloadEvent::Unchanged);
    }

    #[test]
    fn valid_edit_is_reported_once() {
        let dir = TempDir::new("valid");
        let path = dir.file("config.toml");
        fs::write(&path, Config::default().to_toml()).unwrap();
        let mut w = watcher(&path);

        fs::write(&path, "[interaction]\nhover_delay_ms = 111\n").unwrap();
        match w.poll() {
            ReloadEvent::Changed(cfg) => assert_eq!(cfg.interaction.hover_delay_ms, 111),
            other => panic!("expected Changed, got {other:?}"),
        }
        // 同一份内容不该重复上报
        assert_eq!(w.poll(), ReloadEvent::Unchanged);
        assert!(!w.has_external_change());
    }

    #[test]
    fn invalid_edit_keeps_running_config_and_is_reported_once() {
        let dir = TempDir::new("invalid");
        let path = dir.file("config.toml");
        fs::write(&path, Config::default().to_toml()).unwrap();
        let mut w = watcher(&path);

        fs::write(&path, "[interaction\nhover_delay_ms = = 1\n").unwrap();
        assert!(matches!(w.poll(), ReloadEvent::Invalid(_)));
        // 同一份坏内容不该每 500ms 刷一条日志
        assert_eq!(w.poll(), ReloadEvent::Unchanged);
    }

    #[test]
    fn missing_file_is_not_an_error_and_recovers() {
        let dir = TempDir::new("missing");
        let path = dir.file("config.toml");
        let mut w = watcher(&path);
        assert_eq!(w.poll(), ReloadEvent::Unchanged);

        fs::write(&path, "[buddy]\nradius = 60.0\n").unwrap();
        match w.poll() {
            ReloadEvent::Changed(cfg) => assert_eq!(cfg.buddy.radius, 60.0),
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn sync_suppresses_self_written_edit() {
        let dir = TempDir::new("sync");
        let path = dir.file("config.toml");
        fs::write(&path, Config::default().to_toml()).unwrap();
        let mut w = watcher(&path);

        let mut cfg = Config::default();
        cfg.general.language = "en-US".into();
        fs::write(&path, cfg.to_toml()).unwrap();
        assert!(w.has_external_change());
        w.sync();
        assert!(!w.has_external_change());
        assert_eq!(w.poll(), ReloadEvent::Unchanged);
    }

    #[test]
    fn invalid_message_is_a_single_line() {
        let raw = "TOML parse error at line 33, column 14\n   |\n33 | [interaction\n   |              ^\nunclosed table, expected `]`\n";
        let line = summarize(raw);
        assert!(!line.contains('\n'), "must be one line: {line:?}");
        assert!(line.contains("line 33"), "{line}");
        assert!(line.contains("unclosed table"), "{line}");
    }

    #[test]
    fn default_interval_throttles() {
        let dir = TempDir::new("throttle");
        let path = dir.file("config.toml");
        fs::write(&path, Config::default().to_toml()).unwrap();
        let mut w = ConfigWatcher::new(path.clone());
        // 默认构造时戳已经对齐，且刚检查过 → 立刻 poll 什么都不做
        assert_eq!(w.poll(), ReloadEvent::Unchanged);
        fs::write(&path, "[buddy]\nradius = 40.0\n").unwrap();
        assert_eq!(w.poll(), ReloadEvent::Unchanged, "500ms 节流内不该读盘");
    }
}
