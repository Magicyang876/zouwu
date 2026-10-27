//! 日志落盘。
//!
//! 为什么不用 `env_logger`：这个程序**没有控制台**。用户从资源管理器双击起来，
//! stderr 直接进了黑洞；出问题时唯一能拿到的东西是磁盘上的文件。所以日志必须
//! 同时写文件，并且这份文件要能自己长大而不撑爆磁盘（轮转）。
//!
//! 设计取舍：
//! - **自己实现 `log::Log`**（约 200 行）而不是套一层 env_logger：需要运行期
//!   改级别（设置面板里就能改）、需要轮转、需要写 Panic。这三件事 env_logger
//!   都不做，套壳反而更绕。
//! - **`RUST_LOG` 优先于配置**：临时排查时不想为了看一眼 debug 日志去改用户的
//!   持久配置，也不想因此让配置与磁盘上的值"看起来不一致"。
//! - **写失败只吞掉**：日志系统自己报错会形成递归，宁可少一行日志也不能让
//!   常驻程序因为磁盘满而崩。

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// 只抱怨一次，避免日志故障本身刷屏。
static WRITE_FAILED: AtomicBool = AtomicBool::new(false);
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

use log::{LevelFilter, Log, Metadata, Record};
use zouwu_core::config::LogCfg;

/// 日志文件名（不含轮转后缀）。
const FILE_NAME: &str = "zouwu.log";

/// 进程内唯一的日志器。
static LOGGER: OnceLock<FileLogger> = OnceLock::new();

/// 文件写入状态。`None` = 关掉了文件日志（配置里 `to_file = false`）。
struct Sink {
    file: Option<File>,
    max_bytes: u64,
    keep_files: u32,
}

struct Filter {
    /// 全局兜底级别。
    global: LevelFilter,
    /// `module=level` 指令，按模块前缀匹配。
    directives: Vec<(String, LevelFilter)>,
    /// `RUST_LOG` 存在时，配置里的级别不生效。
    locked_by_env: bool,
}

pub struct FileLogger {
    filter: RwLock<Filter>,
    sink: Mutex<Sink>,
}

impl FileLogger {
    /// 当前生效的级别（设置面板展示用）。
    pub fn level(&self) -> LevelFilter {
        self.filter
            .read()
            .map(|f| f.global)
            .unwrap_or(LevelFilter::Info)
    }

    /// 是否正在写文件。
    pub fn file_enabled(&self) -> bool {
        self.sink.lock().map(|s| s.file.is_some()).unwrap_or(false)
    }

    fn enabled_for(&self, metadata: &Metadata) -> bool {
        let Ok(filter) = self.filter.read() else {
            return false;
        };
        let mut best: Option<(usize, LevelFilter)> = None;
        let target = metadata.target();
        for (prefix, level) in &filter.directives {
            if target.starts_with(prefix.as_str()) {
                let len = prefix.len();
                if best.map(|(cur, _)| len >= cur).unwrap_or(true) {
                    best = Some((len, *level));
                }
            }
        }
        let level = best.map(|(_, l)| l).unwrap_or(filter.global);
        metadata.level() <= level
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.enabled_for(metadata)
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} {}: {}\n",
            timestamp(),
            record.level(),
            record.target(),
            record.args()
        );

        // stderr：开发时（有控制台）能立刻看到；从托盘启动时写失败也无所谓。
        let _ = std::io::stderr().write_all(line.as_bytes());

        match self.sink.lock() {
            Ok(mut sink) => {
                if let Some(file) = sink.file.as_mut() {
                    // 单次失败就够吵一次了：静默吞掉写入错误会让"日志没落盘"
                    // 变成一个极难查的问题（我们就这么查过一次）。
                    if let Err(err) = file.write_all(line.as_bytes()) {
                        if !WRITE_FAILED.swap(true, Ordering::Relaxed) {
                            eprintln!("zouwu: log write failed: {err}");
                        }
                    }
                    // 常驻进程可能被直接杀掉，行缓冲比攒在内存里安全得多。
                    let _ = file.flush();
                }
            }
            Err(_) => {
                if !WRITE_FAILED.swap(true, Ordering::Relaxed) {
                    eprintln!("zouwu: log sink lock poisoned");
                }
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut sink) = self.sink.lock() {
            if let Some(file) = sink.file.as_mut() {
                let _ = file.flush();
            }
        }
    }
}

/// 日志目录：`%APPDATA%\zouwu\logs`。
pub fn log_dir() -> PathBuf {
    crate::config_dir().join("logs")
}

/// 当前日志文件路径。
pub fn log_path() -> PathBuf {
    log_dir().join(FILE_NAME)
}

/// 在资源管理器里打开日志目录。
pub fn open_log_dir() -> bool {
    crate::open_dir(&log_dir())
}

/// 安装日志器（幂等）。默认级别 `info`，`RUST_LOG` 存在时以它为准。
///
/// 必须尽早调用：它同时安装 panic hook，让"启动即崩"也能留下证据。
pub fn init() -> &'static FileLogger {
    let logger = LOGGER.get_or_init(|| {
        let (filter, max_bytes, keep_files) = match std::env::var("RUST_LOG") {
            Ok(spec) if !spec.trim().is_empty() => (Filter::parse(&spec), 2048 * 1024, 3),
            _ => (
                Filter {
                    global: LevelFilter::Info,
                    directives: Vec::new(),
                    locked_by_env: false,
                },
                2048 * 1024,
                3,
            ),
        };
        let sink = Sink {
            file: open_sink(max_bytes, keep_files),
            max_bytes,
            keep_files,
        };
        FileLogger {
            filter: RwLock::new(filter),
            sink: Mutex::new(sink),
        }
    });

    // `set_logger` 只允许成功一次；重复调用（测试/多次 run）直接忽略。
    let _ = log::set_logger(logger);
    log::set_max_level(LevelFilter::Trace);
    install_panic_hook();
    logger
}

/// 把配置里的日志设置应用上去。
///
/// 每次调用都会重新评估轮转（改小上限后立刻生效），但不会删除正在写的文件。
pub fn apply(cfg: &LogCfg) {
    let logger = LOGGER.get_or_init(|| {
        let _ = log::set_logger(&NOP_LOGGER);
        FileLogger {
            filter: RwLock::new(Filter {
                global: LevelFilter::Info,
                directives: Vec::new(),
                locked_by_env: false,
            }),
            sink: Mutex::new(Sink {
                file: None,
                max_bytes: 2048 * 1024,
                keep_files: 3,
            }),
        }
    });

    if let Ok(mut filter) = logger.filter.write() {
        if !filter.locked_by_env {
            let level = parse_level(&cfg.level).unwrap_or(LevelFilter::Info);
            filter.global = level;
            filter.directives.clear();
        }
    }

    let max_bytes = cfg.max_size_kb.saturating_mul(1024).max(64 * 1024);
    let keep = cfg.keep_files.min(32);
    // 注意：**不能在持有 sink 锁时打日志**。`log::info!` 会再次去锁同一个
    // Mutex，`std::sync::Mutex` 不可重入，结果是进程静默死锁（真踩过）。
    let mut just_disabled = false;
    if let Ok(mut sink) = logger.sink.lock() {
        let reopen = sink.max_bytes != max_bytes || sink.keep_files != keep;
        sink.max_bytes = max_bytes;
        sink.keep_files = keep;
        if !cfg.to_file {
            just_disabled = sink.file.take().is_some();
        } else if sink.file.is_none() || reopen {
            sink.file = open_sink(max_bytes, keep);
        }
    }
    if just_disabled {
        log::info!("file logging disabled by config");
    }

    log::info!(
        "log: level={} file={} max={}KB keep={}",
        logger.level(),
        if cfg.to_file {
            log_path().display().to_string()
        } else {
            "-".into()
        },
        max_bytes / 1024,
        keep
    );
}

/// 一个什么都不做的兜底日志器（只在 `apply` 早于 `init` 时用一次）。
static NOP_LOGGER: NopLogger = NopLogger;

struct NopLogger;

impl Log for NopLogger {
    fn enabled(&self, _: &Metadata) -> bool {
        false
    }
    fn log(&self, _: &Record) {}
    fn flush(&self) {}
}

impl Filter {
    /// 解析 `RUST_LOG` 风格的指令：`debug`、`zouwu_ui=trace`、`a=info,b=off`。
    fn parse(spec: &str) -> Self {
        let mut filter = Filter {
            global: LevelFilter::Info,
            directives: Vec::new(),
            locked_by_env: true,
        };
        // BTreeMap 只用于去重；顺序无关（匹配时取最长前缀）。
        let mut seen: BTreeMap<String, LevelFilter> = BTreeMap::new();
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match part.split_once('=') {
                Some((target, level)) => {
                    if let Some(level) = parse_level(level) {
                        seen.insert(target.trim().to_string(), level);
                    } else {
                        eprintln!("zouwu: ignoring RUST_LOG part `{part}` (unknown level)");
                    }
                }
                None => {
                    if let Some(level) = parse_level(part) {
                        filter.global = level;
                    } else {
                        // 形如 `zouwu_ui` 单独出现时，按 "该模块开 debug" 处理更符合直觉。
                        seen.insert(part.to_string(), LevelFilter::Debug);
                    }
                }
            }
        }
        filter.directives = seen.into_iter().collect();
        filter
    }
}

fn parse_level(text: &str) -> Option<LevelFilter> {
    match text.trim().to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" | "warning" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

/// 打开（必要时先轮转）日志文件。
fn open_sink(max_bytes: u64, keep_files: u32) -> Option<File> {
    let dir = log_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        eprintln!("zouwu: cannot create log dir {}: {err}", dir.display());
        return None;
    }
    let path = dir.join(FILE_NAME);
    rotate_if_needed(&path, max_bytes, keep_files);
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => Some(file),
        Err(err) => {
            eprintln!("zouwu: cannot open log file {}: {err}", path.display());
            None
        }
    }
}

/// 超过上限就从最老的一份开始腾位置：`a.log` → `a.1.log`，`a.3.log` 删除。
fn rotate_if_needed(path: &Path, max_bytes: u64, keep_files: u32) {
    let too_big = fs::metadata(path)
        .map(|m| m.len() > max_bytes)
        .unwrap_or(false);
    if !too_big {
        return;
    }
    if keep_files == 0 {
        let _ = fs::remove_file(path);
        return;
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(FILE_NAME);
    let oldest = dir.join(format!("{stem}.{keep_files}.log"));
    let _ = fs::remove_file(oldest);
    for n in (1..keep_files).rev() {
        let from = dir.join(format!("{stem}.{n}.log"));
        let to = dir.join(format!("{stem}.{}.log", n + 1));
        if from.exists() {
            let _ = fs::rename(&from, &to);
        }
    }
    let _ = fs::rename(path, dir.join(format!("{stem}.1.log")));
}

/// 安装 panic hook：崩溃也是日志的一部分。
fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "unknown".to_string());
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic>".to_string());
            log::error!("PANIC at {location}: {payload}");
            // 让默认 hook 继续负责打印与（配置下的）abort。
            default_hook(info);
        }));
    });
}

// ── 时间戳 ────────────────────────────────────────────────
// 不引 `chrono`/`time`：只需要"人能读的本地时间"，一行 FFI 就够，
// 而多一个依赖意味着多一份跨平台编译风险（见 ADR-001）。

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(out: *mut SystemTime);
}

/// `YYYY-MM-DD HH:MM:SS.mmm`（本地时间）。
///
/// 非 Windows 平台退回 Unix epoch 秒：v1 只发 Windows（ADR-001），
/// 结论性的一行仍需可读，这里不做完整移植。
pub fn timestamp() -> String {
    #[cfg(target_os = "windows")]
    {
        let mut t = SystemTime::default();
        unsafe { GetLocalTime(&mut t) };
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            t.year, t.month, t.day, t.hour, t.minute, t.second, t.milliseconds
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("{secs}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_global_and_module_levels() {
        let f = Filter::parse("debug");
        assert_eq!(f.global, LevelFilter::Debug);
        assert!(f.directives.is_empty());
        assert!(f.locked_by_env);

        let f = Filter::parse("warn,zouwu_ui=trace,zouwu_platform::tray=off");
        assert_eq!(f.global, LevelFilter::Warn);
        assert_eq!(f.directives.len(), 2);
        assert_eq!(
            f.directives[0],
            ("zouwu_platform::tray".into(), LevelFilter::Off)
        );
        assert_eq!(f.directives[1], ("zouwu_ui".into(), LevelFilter::Trace));
    }

    #[test]
    fn unknown_levels_are_reported_not_fatal() {
        let f = Filter::parse("info,verbose");
        assert_eq!(f.global, LevelFilter::Info);
        // `verbose` 不是级别 → 当成模块名开 debug
        assert_eq!(f.directives, vec![("verbose".into(), LevelFilter::Debug)]);
    }

    #[test]
    fn rotation_keeps_at_most_keep_files() {
        let dir = std::env::temp_dir().join("zouwu-test-log-rotate");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        fs::write(&path, "0123456789").unwrap();

        // 第一轮：超限 → 变成 zouwu.1.log
        rotate_if_needed(&path, 4, 2);
        assert!(dir.join("zouwu.1.log").exists());
        assert!(!path.exists());

        // 第二轮：又写满 → 旧的 1 推到 2，新的成 1
        fs::write(&path, "abcdefghij").unwrap();
        rotate_if_needed(&path, 4, 2);
        let one = fs::read_to_string(dir.join("zouwu.1.log")).unwrap();
        let two = fs::read_to_string(dir.join("zouwu.2.log")).unwrap();
        // `.1` 永远是最新的一份，`.2` 是被它挤下去的上一份。
        assert_eq!(one, "abcdefghij");
        assert_eq!(two, "0123456789");

        // 第三轮：超过 keep_files 的最老一份被删
        fs::write(&path, "x".repeat(16)).unwrap();
        rotate_if_needed(&path, 4, 2);
        assert!(!dir.join("zouwu.3.log").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn timestamp_looks_like_a_date() {
        let ts = timestamp();
        assert_eq!(ts.len(), 23, "got {ts}");
        assert!(ts.starts_with("20"), "got {ts}");
        assert_eq!(&ts[4..5], "-");
    }
}
