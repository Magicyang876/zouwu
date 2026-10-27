//! 全局热键（Windows：`RegisterHotKey` 走 `global-hotkey`）。
//!
//! 关键约束（来自 `global-hotkey` 的 Windows 实现）：它在**创建它的线程**上
//! 建一个隐藏窗口，`WM_HOTKEY` 只会投递到那个线程的消息队列。所以要
//! **在 winit 主线程上创建**本结构，别丢到工作线程——否则按下热键不会有任何反应
//! （没有消息循环去取那条消息）。
//!
//! 这也是把它挂在 `App`（主线程）而不是某个后台服务上的原因。

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use zouwu_core::hotkey::{Hotkey, Key};

/// 热键注册器：同一时刻只保留一条（M1 只有"呼出/收起"一条）。
pub struct HotkeyRegistrar {
    manager: GlobalHotKeyManager,
    current: Option<HotKey>,
}

impl HotkeyRegistrar {
    /// 必须在主线程（有消息循环的线程）上调用。
    pub fn new() -> Result<Self, String> {
        let manager =
            GlobalHotKeyManager::new().map_err(|err| format!("hotkey init failed: {err}"))?;
        Ok(Self {
            manager,
            current: None,
        })
    }

    /// 换成新的热键。注册失败时**保留旧热键**，并把错误交给调用方写日志。
    pub fn set(&mut self, spec: &Hotkey) -> Result<(), String> {
        let hotkey = HotKey::new(modifiers(spec), code(spec.key)?);
        self.manager
            .register(hotkey)
            .map_err(|err| format!("register {spec} failed: {err}"))?;
        if let Some(old) = self.current.replace(hotkey) {
            // 旧的可能已被系统回收；失败不算错。
            if let Err(err) = self.manager.unregister(old) {
                log::debug!("unregister old hotkey failed: {err}");
            }
        }
        log::info!("hotkey registered: {spec} (id {})", hotkey.id());
        Ok(())
    }

    pub fn clear(&mut self) {
        if let Some(old) = self.current.take() {
            let _ = self.manager.unregister(old);
        }
    }

    /// 非阻塞地取走一次"按下"事件；返回 `true` 表示该呼出/收起了。
    ///
    /// 只认 `Pressed`：`Released` 是库自己用线程轮询 `GetAsyncKeyState` 补出来的，
    /// 对"切换显隐"没有意义。
    pub fn poll_pressed(&self) -> bool {
        let Some(current) = self.current else {
            return false;
        };
        let id = current.id();
        let mut fired = false;
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id() == id && event.state() == HotKeyState::Pressed {
                fired = true;
            }
        }
        fired
    }
}

impl Drop for HotkeyRegistrar {
    fn drop(&mut self) {
        self.clear();
    }
}

fn modifiers(spec: &Hotkey) -> Option<Modifiers> {
    let mut mods = Modifiers::empty();
    if spec.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if spec.alt {
        mods |= Modifiers::ALT;
    }
    if spec.shift {
        mods |= Modifiers::SHIFT;
    }
    if spec.meta {
        mods |= Modifiers::META;
    }
    (!mods.is_empty()).then_some(mods)
}

fn code(key: Key) -> Result<Code, String> {
    let code = match key {
        Key::Space => Code::Space,
        Key::Enter => Code::Enter,
        Key::Tab => Code::Tab,
        Key::Escape => Code::Escape,
        Key::Backspace => Code::Backspace,
        Key::Delete => Code::Delete,
        Key::Up => Code::ArrowUp,
        Key::Down => Code::ArrowDown,
        Key::Left => Code::ArrowLeft,
        Key::Right => Code::ArrowRight,
        Key::Letter(c) => match c.to_ascii_uppercase() {
            'A' => Code::KeyA,
            'B' => Code::KeyB,
            'C' => Code::KeyC,
            'D' => Code::KeyD,
            'E' => Code::KeyE,
            'F' => Code::KeyF,
            'G' => Code::KeyG,
            'H' => Code::KeyH,
            'I' => Code::KeyI,
            'J' => Code::KeyJ,
            'K' => Code::KeyK,
            'L' => Code::KeyL,
            'M' => Code::KeyM,
            'N' => Code::KeyN,
            'O' => Code::KeyO,
            'P' => Code::KeyP,
            'Q' => Code::KeyQ,
            'R' => Code::KeyR,
            'S' => Code::KeyS,
            'T' => Code::KeyT,
            'U' => Code::KeyU,
            'V' => Code::KeyV,
            'W' => Code::KeyW,
            'X' => Code::KeyX,
            'Y' => Code::KeyY,
            'Z' => Code::KeyZ,
            other => return Err(format!("unsupported key letter {other}")),
        },
        Key::Digit(c) => match c {
            '0' => Code::Digit0,
            '1' => Code::Digit1,
            '2' => Code::Digit2,
            '3' => Code::Digit3,
            '4' => Code::Digit4,
            '5' => Code::Digit5,
            '6' => Code::Digit6,
            '7' => Code::Digit7,
            '8' => Code::Digit8,
            '9' => Code::Digit9,
            other => return Err(format!("unsupported digit {other}")),
        },
        Key::Function(n) => match n {
            1 => Code::F1,
            2 => Code::F2,
            3 => Code::F3,
            4 => Code::F4,
            5 => Code::F5,
            6 => Code::F6,
            7 => Code::F7,
            8 => Code::F8,
            9 => Code::F9,
            10 => Code::F10,
            11 => Code::F11,
            12 => Code::F12,
            13 => Code::F13,
            14 => Code::F14,
            15 => Code::F15,
            16 => Code::F16,
            17 => Code::F17,
            18 => Code::F18,
            19 => Code::F19,
            20 => Code::F20,
            21 => Code::F21,
            22 => Code::F22,
            23 => Code::F23,
            24 => Code::F24,
            other => return Err(format!("unsupported function key F{other}")),
        },
    };
    Ok(code)
}
