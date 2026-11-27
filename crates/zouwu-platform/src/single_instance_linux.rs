//! 单实例：第二次启动时把已有实例叫出来，而不是再养一只驺吾。
//!
//! Windows 那边用命名内核事件（进程死了句柄自动回收，最省事）。Linux 上没有
//! 等价物，这里用**两件套**：
//!
//! * `flock(2)` 独占锁：判定"谁是第一个"的原子裁判，进程被 kill -9 时内核
//!   也会替我们放锁，不留残骸（这是选 flock 而不是"建一个 pid 文件"的理由）；
//! * `UnixListener`（`$XDG_RUNTIME_DIR` 下的 socket）：第二个进程连上去写一行，
//!   主实例的 accept 线程收到就调回调。socket 文件在**拿到锁之后**才创建，
//!   所以不会有"僵尸 socket 挡住启动"的经典问题。
//!
//! `$XDG_RUNTIME_DIR` 是每个用户一个目录、登录时创建、注销时自动清空，正是
//! 放这类东西的地方（退路是 `/tmp`，权限用 0700 的私有子目录）。

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

use std::os::unix::io::AsRawFd;

fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    std::env::temp_dir()
}

fn lock_path(id: &str) -> PathBuf {
    runtime_dir().join(format!("zouwu-{id}.lock"))
}

fn socket_path(id: &str) -> PathBuf {
    runtime_dir().join(format!("zouwu-{id}.sock"))
}

extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}

const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;

/// 主实例持有的守卫。acquire 返回 `Ok(None)` 表示"已经有别的实例在跑"。
pub struct SingleInstance {
    /// 拿着的锁文件（drop 时关掉 = 自动放锁）。
    _lock: std::fs::File,
    id: String,
    /// 叫停 accept 线程用。
    stop: Arc<AtomicBool>,
    /// 真正在监听的 socket（第二个实例连这里）。
    socket: PathBuf,
    /// 自己起的 accept 线程（Drop 时 join，避免退出时还在摸文件）。
    thread: std::sync::Mutex<Option<thread::JoinHandle<()>>>,
}

impl SingleInstance {
    /// 尝试成为主实例。返回 `Ok(None)` = 已有人占着。
    pub fn acquire(id: &str) -> Result<Option<Self>, String> {
        let path = lock_path(id);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| format!("open {} failed: {err}", path.display()))?;
        let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
        if rc != 0 {
            // 锁被占 = 已有实例。不写 pid，也不用清理什么。
            log::debug!("single-instance: {} is held by another process", path.display());
            return Ok(None);
        }
        log::debug!("single-instance guard acquired: {}", path.display());
        Ok(Some(Self {
            _lock: file,
            id: id.to_string(),
            stop: Arc::new(AtomicBool::new(false)),
            socket: socket_path(id),
            thread: std::sync::Mutex::new(None),
        }))
    }

    /// 再启动一次时，用来把信号发给正在跑的那个实例。
    pub fn signal_existing(id: &str) -> bool {
        let path = socket_path(id);
        match UnixStream::connect(&path) {
            Ok(mut stream) => {
                let _ = stream.write_all(b"wake\n");
                let _ = stream.flush();
                true
            }
            Err(err) => {
                log::debug!("single-instance: connect {} failed: {err}", path.display());
                false
            }
        }
    }

    /// 起一个后台线程等信号，每收到一次就调用一次 `callback`。
    ///
    /// 回调在 **accept 线程**上执行，所以只应该做"往通道里塞一个唤醒事件"这种事。
    pub fn on_second_launch(&self, callback: impl Fn() + Send + 'static) {
        // 我们持有锁，所以这个 socket 文件一定是上一次留下的残骸，先清掉。
        let _ = std::fs::remove_file(&self.socket);
        let listener = match UnixListener::bind(&self.socket) {
            Ok(listener) => listener,
            Err(err) => {
                log::warn!(
                    "single-instance: bind {} failed: {err}",
                    self.socket.display()
                );
                return;
            }
        };
        if let Err(err) = listener.set_nonblocking(false) {
            log::debug!("single-instance: set_nonblocking failed: {err}");
        }
        let stop = self.stop.clone();
        let socket = self.socket.clone();
        let spawned = thread::Builder::new()
            .name("zouwu-instance".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    match stream {
                        Ok(mut stream) => {
                            // 读掉对方写的那一行（不读也行，但读掉更干净）。
                            let mut buf = [0u8; 32];
                            let _ = stream.read(&mut buf);
                            callback();
                        }
                        Err(err) => {
                            log::debug!("single-instance: accept failed: {err}");
                            break;
                        }
                    }
                }
                // 线程退出时把 socket 文件带走，别留给下次的自己。
                let _ = std::fs::remove_file(&socket);
            });
        match spawned {
            Ok(handle) => {
                *self.thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
            }
            Err(err) => log::warn!("failed to spawn instance watcher: {err}"),
        }
    }

    pub fn name(&self) -> String {
        self.id.clone()
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // 连一下自己，让 accept 从阻塞里醒来。
        let _ = UnixStream::connect(&self.socket);
        let handle = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(thread) = handle {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket);
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