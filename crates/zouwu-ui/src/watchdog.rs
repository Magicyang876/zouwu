//! 卡死时"停在哪"的看门狗。
//!
//! 用户报的卡死（调历史窗 → 调二维码窗 → 关 → 再调历史窗）在本机用合成按键
//! 复现不出来，靠猜是猜不完的。所以加一条常开的最小证据：主线程在进入每个可能
//! 阻塞的阶段时把 `STAGE` 写下来，另起一个线程每 2 秒看一眼。
//!
//! 如果主线程卡住（死锁、或者被 `SendMessage` 拖住），看门狗会**继续**打日志，
//! 并且一直打出同一个阶段名 —— 那一行就是卡住的位置。反过来，如果看门狗也不
//! 打了，说明整个进程被冻（`DestroyWindow` 之类跨进程同步），那是另一种问题。
//!
//! 开销：一次 `store` + 一次 `fetch_add`，都是 relaxed，纳秒级。

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
}

static STAGE: AtomicU8 = AtomicU8::new(stage::IDLE);
static SEQ: AtomicU64 = AtomicU64::new(0);

/// 主线程进入某个阶段。幂等、无锁。
pub fn enter(stage: u8) {
    STAGE.store(stage, Ordering::Relaxed);
    SEQ.fetch_add(1, Ordering::Relaxed);
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
        _ => "unknown",
    }
}

/// 起看门狗线程。主线程连续 5 秒（约 2 拍）没换阶段就打一条 warn。
pub fn spawn() {
    let spawned = std::thread::Builder::new()
        .name("zouwu-watchdog".into())
        .spawn(|| {
            let mut last_seq = SEQ.load(Ordering::Relaxed);
            let mut stuck = 0u32;
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let seq = SEQ.load(Ordering::Relaxed);
                if seq == last_seq {
                    stuck += 1;
                    if stuck % 3 == 1 {
                        let at = STAGE.load(Ordering::Relaxed);
                        log::warn!(
                            "watchdog: main thread stuck in `{}` for {}s (stage={at}, seq={seq})",
                            name(at),
                            stuck * 2
                        );
                    }
                } else {
                    if stuck > 0 {
                        log::info!("watchdog: main thread resumed after {}s", stuck * 2);
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
