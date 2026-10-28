//! 卡死时"停在哪"的看门狗。
//!
//! 用户报的卡死（调历史窗 → 调二维码窗 → 关 → 再调 → 冻住）在本机用合成按键
//! 复现不出来，靠猜猜不完。所以加一条常开的最小证据：主线程在进入每个可能阻塞
//! 的阶段时把 `STAGE` 写下来，另起一个线程每 2 秒看一眼。
//!
//! 关键设计：**看门狗不经过 `log` 宏**，自己往 `logs/watchdog.log` 写。
//! 现场证据表明这类卡死会让整个进程冻住（连独立线程的 `log::info!` 都停了）。
//! 那很可能是因为主线程正握着堆锁/日志锁在等别的进程——`log::info!` 要格式化
//! （分配内存）就打不出来了。所以这里用**不分配内存**的写法（栈上定长缓冲 +
//! 自己持有的 File），这样能区分两件事：
//!
//! - `watchdog.log` 还在长，`zouwu.log` 停了 → 进程活着，主线程被锁在某个
//!   同步调用里（USER/GDI/堆锁），位置就是 `stage=`。
//! - 两个都停了 → 整个进程被冻死（连纯 syscall 都过不去）。
//!
//! 开销：一次 `store` + 一次 `fetch_add`（relaxed，纳秒级），加上每 2 秒
//! 一次不到 64 字节的 `write`。

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::Duration;

/// 每个阶段的编号。加阶段就往后加，别改已有的数字（日志要对得上）。
pub mod stage {
    pub const IDLE: u8 = 0;
    pub const ABOUT_TO_WAIT: u8 = 1;
    pub const ABOUT_TO_WAIT_END: u8 = 2;
    pub const WINDOW_EVENT: u8 = 3;
    pub const HISTORY_EVENT: u8 = 4;
    pub const REPORT_EVENT: u8 = 5;
    pub const SETTINGS_EVENT: u8 = 6;
    pub const OPEN_HISTORY: u8 = 7;
    pub const OPEN_REPORT: u8 = 8;
    pub const OPEN_SETTINGS: u8 = 9;
    pub const POLL_CLIPBOARD: u8 = 10;
    pub const RUN_ACTION: u8 = 11;
    pub const RENDER: u8 = 12;
    pub const PRESENT: u8 = 13;
    pub const EXIT: u8 = 14;
    pub const CLOSE_WINDOW: u8 = 15;
    pub const FOREGROUND: u8 = 16;
    pub const TRAY: u8 = 17;
}

static STAGE: AtomicU8 = AtomicU8::new(stage::IDLE);
static SEQ: AtomicU64 = AtomicU64::new(0);
static POLLS: AtomicU64 = AtomicU64::new(0);

/// 主线程进入某个阶段。幂等、无锁。
pub fn enter(stage: u8) {
    STAGE.store(stage, Ordering::Relaxed);
    SEQ.fetch_add(1, Ordering::Relaxed);
}

/// 一次 100ms 轮询（光标/热键/剪贴板）跑完了。
pub fn polled() {
    POLLS.fetch_add(1, Ordering::Relaxed);
}

/// 阶段编号 → 名字（日志里给人看）。
pub fn name(stage: u8) -> &'static str {
    match stage {
        stage::IDLE => "idle",
        stage::ABOUT_TO_WAIT => "about_to_wait",
        stage::ABOUT_TO_WAIT_END => "about_to_wait end",
        stage::WINDOW_EVENT => "window_event",
        stage::HISTORY_EVENT => "history_event",
        stage::REPORT_EVENT => "report_event",
        stage::SETTINGS_EVENT => "settings_event",
        stage::OPEN_HISTORY => "open_history",
        stage::OPEN_REPORT => "open_report",
        stage::OPEN_SETTINGS => "open_settings",
        stage::POLL_CLIPBOARD => "poll_clipboard",
        stage::RUN_ACTION => "run_action",
        stage::RENDER => "render",
        stage::PRESENT => "present",
        stage::EXIT => "exit",
        stage::CLOSE_WINDOW => "close_window",
        stage::FOREGROUND => "take_foreground",
        stage::TRAY => "tray",
        _ => "unknown",
    }
}

/// 不分配内存的写手：栈上定长缓冲 + 自己持有的文件句柄。
///
/// 之所以不用 `String`：这个线程存在的意义就是"在主线程可能握着堆锁时也能说话"。
struct RawLine {
    file: Option<std::fs::File>,
    buf: String,
}

impl RawLine {
    fn new() -> Self {
        let path = zouwu_platform::logging::log_dir().join("watchdog.log");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        if file.is_none() {
            log::warn!("watchdog: cannot open {}", path.display());
        }
        Self {
            file,
            // 一行实际不到 100 字节；预留 256 足够，写不超就不会再分配。
            buf: String::with_capacity(256),
        }
    }

    fn line(&mut self, args: std::fmt::Arguments<'_>) {
        self.buf.clear();
        let _ = self.buf.write_fmt(args);
        self.buf.push('\n');
        if let Some(file) = &mut self.file {
            let _ = file.write_all(self.buf.as_bytes());
        }
    }
}

/// 起看门狗线程。主线程连续 4 秒没换阶段就往两个日志里各打一条。
pub fn spawn() {
    let spawned = std::thread::Builder::new()
        .name("zouwu-watchdog".into())
        .spawn(|| {
            let mut sink = RawLine::new();
            let mut last_seq = SEQ.load(Ordering::Relaxed);
            let mut last_polls = POLLS.load(Ordering::Relaxed);
            let mut stuck = 0u32;
            let mut ticks = 0u32;
            loop {
                std::thread::sleep(Duration::from_secs(2));
                ticks += 1;
                let seq = SEQ.load(Ordering::Relaxed);
                let polls = POLLS.load(Ordering::Relaxed);
                let at = STAGE.load(Ordering::Relaxed);
                // 每 8 秒一条心跳：证明看门狗自己活着，也让人看出"卡死"时
                // 主线程到底还在不在转（`polls` 是否在涨）。
                if ticks % 4 == 0 {
                    sink.line(format_args!(
                        "beat stage={} polls=+{} seq=+{}",
                        name(at),
                        polls.wrapping_sub(last_polls),
                        seq.wrapping_sub(last_seq)
                    ));
                    last_polls = polls;
                }
                if seq == last_seq {
                    stuck += 1;
                    if stuck % 2 == 1 {
                        sink.line(format_args!(
                            "STUCK {}s in `{}` (stage={at}, seq={seq}, polls={polls})",
                            stuck * 2,
                            name(at)
                        ));
                    }
                } else {
                    if stuck > 0 {
                        sink.line(format_args!("resumed after {}s", stuck * 2));
                    }
                    stuck = 0;
                    last_seq = seq;
                }
            }
        });
    if let Err(err) = spawned {
        log::warn!("watchdog: cannot spawn thread: {err}");
    }
}
