//! 单实例：第二次启动时把已有窗口叫出来，而不是再养一只驺吾。
//!
//! 做法是 Windows 上最省事也最可靠的 **命名内核事件**：
//! - 主实例创建自动重置事件 `Local\zouwu-instance-<id>`，并起一个等待线程；
//! - 后续实例 `OpenEventW` 成功 → 说明有人在跑，`SetEvent` 打个招呼就退出；
//! - 主实例的等待线程收到信号 → 通过回调通知 UI 层（比如呼出窗口）。
//!
//! 为什么不用文件锁 / 互斥量：文件锁在异常退出后需要靠超时判定，而内核事件
//! 在进程死亡时由系统自动回收句柄，语义干净，也不用清理残留文件。
//!
//! 为什么手写 extern 而不引 `windows` crate：这 6 个函数是稳定的 kernel32 导出，
//! 少一个依赖就少一次 `dlltool`/raw-dylib 的踩坑机会（见 ADR-007）。

use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver};
use std::thread;

type Handle = *mut c_void;

const ERROR_ALREADY_EXISTS: u32 = 183;
const SYNCHRONIZE: u32 = 0x0010_0000;
const INFINITE: u32 = 0xFFFF_FFFF;
const WAIT_OBJECT_0: u32 = 0;

#[link(name = "kernel32")]
extern "system" {
    fn CreateEventW(
        attributes: *mut c_void,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> Handle;
    fn OpenEventW(desired_access: u32, inherit: i32, name: *const u16) -> Handle;
    fn SetEvent(handle: Handle) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetLastError() -> u32;
}

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 事件名字：带上 `id` 是为了让不同版本/不同 profile 能并行调试。
fn event_name(id: &str) -> String {
    format!("Local\\zouwu-single-{id}")
}

/// 主实例持有的守卫。`None` 表示"已经有别的实例在跑"。
pub struct SingleInstance {
    event: Handle,
    name: Vec<u16>,
}

impl SingleInstance {
    /// 尝试成为主实例。
    ///
    /// 返回 `Ok(None)` 表示另一个实例已经存在（调用方应当立刻退出，之前
    /// 已经调用过 `signal_existing` 把对方叫醒）。
    pub fn acquire(id: &str) -> Result<Option<Self>, String> {
        let name = wide(&event_name(id));
        // 先探一下：已有实例就直接通知它，别去抢创建。
        let existing = unsafe { OpenEventW(SYNCHRONIZE | 0x0002, 0, name.as_ptr()) };
        if !existing.is_null() {
            unsafe {
                SetEvent(existing);
                CloseHandle(existing);
            }
            return Ok(None);
        }

        let event = unsafe { CreateEventW(std::ptr::null_mut(), 0, 0, name.as_ptr()) };
        if event.is_null() {
            return Err(format!("CreateEventW failed (err {})", unsafe {
                GetLastError()
            }));
        }
        // 竞态兜底：两个进程同时走到这里时，后到的那个会拿到同名事件
        // 且 GetLastError 报 ERROR_ALREADY_EXISTS。
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(event);
            }
            return Ok(None);
        }
        log::debug!("single-instance guard acquired: {}", event_name(id));
        Ok(Some(Self { event, name }))
    }

    /// 再启动一次时，用来把信号发给正在跑的那个实例。
    pub fn signal_existing(id: &str) -> bool {
        let name = wide(&event_name(id));
        let event = unsafe { OpenEventW(SYNCHRONIZE | 0x0002, 0, name.as_ptr()) };
        if event.is_null() {
            return false;
        }
        let ok = unsafe { SetEvent(event) } != 0;
        unsafe {
            CloseHandle(event);
        }
        ok
    }

    /// 起一个后台线程等信号，每收到一次就调用一次 `callback`。
    ///
    /// 回调在**等待线程**上执行，所以只应该做"往通道里塞一个唤醒事件"这种事。
    pub fn on_second_launch(&self, callback: impl Fn() + Send + 'static) {
        let event = self.event as usize;
        thread::Builder::new()
            .name("zouwu-instance".into())
            .spawn(move || loop {
                let handle = event as Handle;
                match unsafe { WaitForSingleObject(handle, INFINITE) } {
                    WAIT_OBJECT_0 => callback(),
                    // 句柄没了（进程要退出）或者其它异常：结束等待线程。
                    _ => break,
                }
            })
            .map(|_| ())
            .unwrap_or_else(|err| log::warn!("failed to spawn instance watcher: {err}"));
    }

    pub fn name(&self) -> String {
        String::from_utf16_lossy(&self.name[..self.name.len().saturating_sub(1)])
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.event);
        }
    }
}

/// 供 UI 层使用的组合形态：守卫 + 一个可轮询的唤醒通道。
///
/// 用 `mpsc` 而不是直接把 `EventLoopProxy` 塞进回调，是为了让这一层不知道
/// 窗口/事件循环的存在（ADR-001 的分层规则）。
pub struct InstanceSignal {
    _guard: SingleInstance,
    rx: Receiver<()>,
}

impl InstanceSignal {
    /// `None` = 已有实例在跑。
    pub fn acquire(id: &str) -> Result<Option<Self>, String> {
        let Some(guard) = SingleInstance::acquire(id)? else {
            return Ok(None);
        };
        let (tx, rx) = mpsc::channel();
        guard.on_second_launch(move || {
            // 对方连点两次时通道里会堆消息，UI 侧只看"有没有"。
            let _ = tx.send(());
        });
        Ok(Some(Self { _guard: guard, rx }))
    }

    /// 有没有"又启动了一次"的信号（非阻塞，会一次排干）。
    pub fn take_signal(&self) -> bool {
        let mut any = false;
        while self.rx.try_recv().is_ok() {
            any = true;
        }
        any
    }
}
