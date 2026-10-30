//! 控制台：GUI 子系统的 exe 怎么才能"恰好"在终端里打印。
//!
//! ## 为什么需要这一层
//!
//! 同一个 exe 有两种活法，而它们对**子系统**的要求正好相反：
//!
//! | 活法 | 需要什么 | 子系统 |
//! |---|---|---|
//! | 双击 / 开机自启，常驻小工具 | 绝不能跳出一个黑窗 | `WINDOWS` |
//! | `zouwu --version` / `--install` | 必须能打印到调用它的终端 | `CONSOLE` |
//!
//! 子系统在链接期就定了，二选一。选 `WINDOWS`（黑窗是**每个人**每天都会看到的，
//! 命令行输出只有装机和排查时才有用），再用 `AttachConsole(ATTACH_PARENT_PROCESS)`
//! 临时把输出接回调用者的控制台。
//!
//! ## 只在"没有句柄"的时候才接管
//!
//! 这里的判断顺序很重要：**已有的 stdout 一律不动**。
//!
//! - `zouwu --version > log.txt`：父进程给的是文件句柄，本来就能写，接管理论上还能写
//!   但会把输出从文件抢回屏幕；
//! - `zouwu --version | findstr zouwu`：给的是管道句柄，同上；
//! - 从 cmd / PowerShell / Windows Terminal 直接跑：句柄可能已经随 STARTUPINFO 继承下来。
//!
//! 只有句柄是 `NULL` / `INVALID_HANDLE_VALUE`（典型的"从资源管理器双击"或
//! "GUI 子进程没拿到标准句柄"）时，才 attach 再自己开一个 `CONOUT$`。
//!
//! 代价：从**资源管理器**双击一个带参数的命令（比如 `--install`）时，输出没有去处；
//! 那本来就不是该双击的东西，装机入口在托盘菜单里。

use std::ffi::c_void;

type Handle = isize;

/// `(DWORD)-1`：attach 到父进程的控制台。
const ATTACH_PARENT_PROCESS: u32 = (-1i32) as u32;
const STD_OUTPUT_HANDLE: u32 = (-11i32) as u32;
const STD_ERROR_HANDLE: u32 = (-12i32) as u32;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE_VALUE: Handle = -1;

#[link(name = "kernel32")]
extern "system" {
    fn AttachConsole(process_id: u32) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn GetStdHandle(which: u32) -> Handle;
    fn SetStdHandle(which: u32, handle: Handle) -> i32;
}

/// 当前进程有没有能写的 stdout。
fn stdout_usable() -> bool {
    let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    handle != 0 && handle != INVALID_HANDLE_VALUE
}

/// 把 stdout / stderr 接回调用者的控制台。返回是否真的接管了。
///
/// 必须在任何一次 `println!` **之前**调用（`std` 每次写都现取句柄，
/// 所以"晚一点"也不算错，只是那之前的输出就丢了）。
pub fn attach_parent() -> bool {
    if stdout_usable() {
        return false;
    }
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return false;
        }
        // `AttachConsole` 只接上控制台，**不会**帮我们建标准句柄：得自己开 CONOUT$。
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        let out = CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut::<c_void>() as Handle,
        );
        if out == INVALID_HANDLE_VALUE {
            return false;
        }
        SetStdHandle(STD_OUTPUT_HANDLE, out);
        SetStdHandle(STD_ERROR_HANDLE, out);
        out != 0
    }
}

#[cfg(test)]
mod tests {
    /// 测试进程本来就有控制台（cargo test 是控制台程序），这条路径必须早退，
    /// 否则会把测试进程的 stdout 换成 CONOUT$ —— 那会让 `cargo test` 的输出串台。
    #[test]
    fn does_not_touch_an_existing_stdout() {
        assert!(!super::attach_parent());
    }
}
