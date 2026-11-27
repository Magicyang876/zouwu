//! 剪贴板（Linux/Wayland：`wl_data_device`，经 `wl-clipboard-rs`）。
//!
//! 和 Windows 版的语义差异是**协议级**的，不是实现偷懒：
//!
//! * Windows 的 `GetClipboardSequenceNumber` 只是读一个计数器，所以那边可以
//!   每 16ms 轮询一次；Wayland 没有这种东西。想"看见对方复制了什么"必须
//!   先持有选区、再走一次 `wl_data_device` 的 offer→pipe 流程（`wl-clipboard-rs`
//!   用 fork 出的子进程完成握手，所以每次读都有一次 fork + 一次 uid 切换的代价）。
//!   于是这里把"读"做成**惰性 + 带缓存**：`sequence()` 每次最多真读一次，
//!   内容没变就不涨序号，UI 侧的轮询循环自然退化成"没有变化"。
//! * "被动监听"（别人一复制我们就知道）在 Wayland 下拿不到，除非用户给了
//!   privileged data-control 协议（KWin 只给 wlroots 系或 KDE 自己）。所以
//!   剪贴板历史是**轮询式**的，和 Windows 版行为一致，只是粒度粗一点。
//!
//! 写方向（`set_text`）没问题：`copy` 会 fork 一个常驻子进程持有选区，
//! 我们的 UI 进程退出后剪贴板内容依然有效——和 Windows 上的行为一样。

use std::io::Read;
use std::sync::{Mutex, OnceLock};

use wl_clipboard_rs::copy::{MimeType as CopyMime, Options, Source};
use wl_clipboard_rs::paste::{get_contents, ClipboardType, MimeType as PasteMime, Seat};

/// 缓存：`(序号, 上次读到的全文)`。序号只在内容变化时 +1。
struct Cache {
    seq: u32,
    text: String,
    /// 上次读失败的时间戳，避免在没有任何选区时空转 fork。
    last_fail: Option<std::time::Instant>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            seq: 0,
            text: String::new(),
            last_fail: None,
        })
    })
}

fn lock_cache() -> std::sync::MutexGuard<'static, Cache> {
    cache().lock().unwrap_or_else(|err| err.into_inner())
}

/// 真读一次选区。失败（没有选区 / 对方不提供文本 / 超时）时返回 None。
fn read_selection() -> Option<String> {
    let (mut pipe, _mime) =
        match get_contents(ClipboardType::Regular, Seat::Unspecified, PasteMime::Text) {
            Ok(pair) => pair,
            Err(err) => {
                log::debug!("clipboard read failed: {err}");
                return None;
            }
        };
    let mut bytes = Vec::new();
    if let Err(err) = pipe.read_to_end(&mut bytes) {
        log::debug!("clipboard pipe read failed: {err}");
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// 读剪贴板文本（当前内容）。
pub fn get_text() -> Option<String> {
    let text = read_selection()?;
    let mut cache = lock_cache();
    if cache.text != text {
        cache.text = text.clone();
        cache.seq = cache.seq.wrapping_add(1);
    }
    Some(text)
}

/// 写剪贴板文本。返回 false 表示没写成功（选区被拒/没有合成器连接）。
pub fn set_text(text: &str) -> bool {
    let bytes = text.as_bytes().to_vec().into_boxed_slice();
    match Options::new().copy(Source::Bytes(bytes), CopyMime::Text) {
        Ok(()) => {
            // 自己写进去的东西当然算"变了"：让序号立刻前进，
            // 免得 UI 侧以为还是旧内容又去轮询两次。
            let mut cache = lock_cache();
            cache.text = text.to_string();
            cache.seq = cache.seq.wrapping_add(1);
            true
        }
        Err(err) => {
            log::warn!("clipboard write failed: {err}");
            false
        }
    }
}

/// 剪贴板序号：内容变了就变。没有剪贴板内容时返回 None。
///
/// 和 Windows 的 `GetClipboardSequenceNumber` 不同，这个序号是**本进程**的
/// 计数（每次内容变化 +1），但调用方的用法（"跟上一次不一样就重读"）是一致的。
pub fn sequence() -> Option<u32> {
    // 只是问"变了吗"的时候也要真读一次：没有别的办法。
    // 但没选区时别每次 fork：失败后 1 秒内不再试。
    {
        let cache = lock_cache();
        if let Some(at) = cache.last_fail {
            if at.elapsed() < std::time::Duration::from_secs(1) {
                return None;
            }
        }
    }
    match read_selection() {
        Some(text) => {
            let mut cache = lock_cache();
            cache.last_fail = None;
            if cache.text != text {
                cache.text = text;
                cache.seq = cache.seq.wrapping_add(1);
            }
            Some(cache.seq)
        }
        None => {
            let mut cache = lock_cache();
            cache.last_fail = Some(std::time::Instant::now());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_is_monotonic_after_a_write() {
        // 只测"写之后序号前进"，不测系统剪贴板里到底有没有内容
        // （测试机可能没有合成器连接）。
        let before = lock_cache().seq;
        let written = set_text("zouwu-clipboard-test");
        if written {
            assert!(lock_cache().seq > before);
        }
    }
}
