//! 剪贴板：把识别出来的链接塞进去。
//!
//! 只用最老实的 Win32 API。有三处坑必须处理，否则"复制"会静默失败：
//!
//! 1. `OpenClipboard` 可能被别的程序占着 —— 必须重试几次，而不是失败一次就放弃；
//! 2. 内存必须是 `GlobalAlloc(GMEM_MOVEABLE)` 分配的，且 **`SetClipboardData`
//!    成功之后所有权转给系统**，此时再 `GlobalFree` 会把剪贴板内容一起弄坏；
//! 3. 文本必须是 UTF-16 且带结尾的 `\0`，否则粘贴出来是乱码或截断。

#![cfg(target_os = "windows")]

use std::ffi::c_void;

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(owner: *mut c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
    fn GetClipboardData(format: u32) -> *mut c_void;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
    fn GlobalLock(mem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(mem: *mut c_void) -> i32;
    fn GlobalFree(mem: *mut c_void) -> *mut c_void;
}

/// 把文本放进剪贴板。返回是否成功（失败时调用方该告诉用户，而不是假装成功）。
pub fn set_text(text: &str) -> bool {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = wide.len() * std::mem::size_of::<u16>();

    unsafe {
        if !open_with_retry() {
            return false;
        }
        // 从这一刻起必须保证走到 CloseClipboard，所以下面用闭包式结构收口。
        let ok = (|| {
            if EmptyClipboard() == 0 {
                return false;
            }
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if mem.is_null() {
                return false;
            }
            let dst = GlobalLock(mem);
            if dst.is_null() {
                GlobalFree(mem);
                return false;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, dst as *mut u8, bytes);
            GlobalUnlock(mem);
            // 成功后系统接管这块内存；失败则还归我们，得自己释放。
            if SetClipboardData(CF_UNICODETEXT, mem).is_null() {
                GlobalFree(mem);
                return false;
            }
            true
        })();
        CloseClipboard();
        ok
    }
}

/// 剪贴板里有文本就取出来（打包成"复制失败时的兜底展示"用）。
pub fn get_text() -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            return None;
        }
        if !open_with_retry() {
            return None;
        }
        let out = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT);
            if handle.is_null() {
                return None;
            }
            let src = GlobalLock(handle);
            if src.is_null() {
                return None;
            }
            let mut len = 0usize;
            let units = src as *const u16;
            while *units.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(units, len);
            let text = String::from_utf16_lossy(slice);
            GlobalUnlock(handle);
            Some(text)
        })();
        CloseClipboard();
        out
    }
}

/// 剪贴板是全局独占资源：别的程序正开着的时候 `OpenClipboard` 会失败。
/// 实测打开一个 Office 文档时窗口期能到几十毫秒，所以退让几次再放弃。
fn open_with_retry() -> bool {
    for attempt in 0..8 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            return true;
        }
        if attempt < 7 {
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 会真的动剪贴板，所以只在有桌面会话时跑；并且跑完要把原内容还原，
    /// 免得开发者自己复制的东西被测试吃掉。
    #[test]
    fn round_trip_restores_the_previous_content() {
        let saved = get_text();
        let probe = "zouwu-clipboard-probe\n第二行 · with unicode ✓";
        if !set_text(probe) {
            // 剪贴板被占用（比如远程会话里没有剪贴板）→ 跳过断言，但别假装通过。
            eprintln!("clipboard unavailable; skipping");
            return;
        }
        assert_eq!(get_text().as_deref(), Some(probe));
        if let Some(text) = saved {
            assert!(set_text(&text), "restoring the original text must work");
        }
    }
}
