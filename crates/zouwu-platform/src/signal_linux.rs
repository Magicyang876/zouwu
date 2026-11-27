//! 收 SIGTERM / SIGINT / SIGHUP 时走**正常退出**。
//!
//! 这条不是为了优雅，是为了不留垃圾：`alt drag` 会把 KDE 的窗口拖动修饰键
//! 临时改成 Alt（`~/.config/kwinrc`），全靠退出时还原。默认的 SIGTERM 处理是
//! 立刻终止——`App::exiting()` 一行都不跑，用户下次打开 KDE 就会发现自己的
//! Meta 拖动不见了、变成 Alt。注销、关机、`pkill` 都会走这条路径。
//!
//! 信号处理器里不能干任何花活（async-signal-safe），所以只做最朴素的事：
//! 往一根自管道写一个字节。真正干活的是一个普通线程——它读管道，然后回调
//! 主线程（用事件循环的 proxy 唤醒，让 `about_to_wait` 去收拾）。

use std::os::unix::io::FromRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// 收到过终止信号吗（`about_to_wait` 查它）。
static TERMINATE: AtomicBool = AtomicBool::new(false);
/// 自管道写端。信号处理器只碰它，别的都不碰。
static SIGNAL_FD: OnceLock<i32> = OnceLock::new();

extern "C" fn on_signal(_sig: libc::c_int) {
    // 只有 `write(2)` 是安全的；失败也不管（管道满了说明已经有人收到通知）。
    if let Some(fd) = SIGNAL_FD.get() {
        let byte = 1u8;
        unsafe {
            libc::write(*fd, (&byte as *const u8).cast(), 1);
        }
    }
}

/// 装处理器 + 起监听线程。`wake` 用来把主线程叫醒（通常是事件循环 proxy）。
///
/// 重复调用只会更新 `wake`，不会重复装处理器。
pub fn install(wake: Arc<dyn Fn() + Send + Sync>) {
    static WAKE: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);
    if let Ok(mut slot) = WAKE.lock() {
        *slot = Some(wake);
    }
    if SIGNAL_FD.get().is_some() {
        return;
    }

    let mut fds = [0 as libc::c_int; 2];
    // 阻塞写端（默认）没问题：一个字节而已；读端非阻塞方便线程退出。
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        log::warn!(
            "signal: cannot create self-pipe: {}",
            std::io::Error::last_os_error()
        );
        return;
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    if SIGNAL_FD.set(write_fd).is_err() {
        return;
    }

    unsafe {
        libc::signal(libc::SIGTERM, on_signal as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_signal as libc::sighandler_t);
    }

    std::thread::Builder::new()
        .name("zouwu-signal".to_string())
        .spawn(move || {
            use std::os::unix::io::AsRawFd;
            let file = unsafe { std::fs::File::from_raw_fd(read_fd) };
            let mut buf = [0u8; 8];
            loop {
                let n = unsafe { libc::read(file.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    log::debug!("signal: reader stopped: {err}");
                    return;
                }
                if n == 0 {
                    return;
                }
                TERMINATE.store(true, Ordering::SeqCst);
                log::info!("signal: termination requested, shutting down cleanly");
                if let Ok(slot) = WAKE.lock() {
                    if let Some(wake) = slot.as_ref() {
                        wake();
                    }
                }
                return;
            }
        })
        .ok();
}

/// 收到过终止信号？
pub fn terminate_requested() -> bool {
    TERMINATE.load(Ordering::SeqCst)
}
