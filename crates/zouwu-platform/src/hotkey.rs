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

/// 一个热键的用途。
///
/// 名字是给"按下去之后该干什么"用的：注册器只负责把它们区分开，
/// 不关心具体动作。加功能时在这里加一个槽位，而不是再造一个注册器
/// （每个注册器都会建自己的隐藏窗口，没必要）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// 呼出 / 收起小兽。
    Summon,
    /// 扫屏幕上的二维码。
    ScanQr,
    /// 打开剪贴板历史。
    Clipboard,
}

/// 热键注册器：按槽位各保留一条。
pub struct HotkeyRegistrar {
    manager: GlobalHotKeyManager,
    slots: Vec<(Slot, HotKey)>,
}

impl HotkeyRegistrar {
    /// 必须在主线程（有消息循环的线程）上调用。
    pub fn new() -> Result<Self, String> {
        let manager =
            GlobalHotKeyManager::new().map_err(|err| format!("hotkey init failed: {err}"))?;
        Ok(Self {
            manager,
            slots: Vec::new(),
        })
    }

    /// 给某个槽位换成新的热键。注册失败时**保留该槽位的旧热键**，
    /// 并把错误交给调用方写日志。
    pub fn set(&mut self, slot: Slot, spec: &Hotkey) -> Result<(), String> {
        let hotkey = HotKey::new(modifiers(spec), code(spec.key)?);
        self.manager
            .register(hotkey)
            .map_err(|err| format!("register {spec} failed: {err}"))?;
        if let Some(index) = self.slots.iter().position(|(s, _)| *s == slot) {
            let old = std::mem::replace(&mut self.slots[index].1, hotkey);
            // 旧的可能已被系统回收；失败不算错。
            if let Err(err) = self.manager.unregister(old) {
                log::debug!("unregister old hotkey failed: {err}");
            }
        } else {
            self.slots.push((slot, hotkey));
        }
        log::info!(
            "hotkey registered: {spec} for {slot:?} (id {})",
            hotkey.id()
        );
        Ok(())
    }

    /// 取消某个槽位（配置写空 = 用户不要这条热键）。
    pub fn clear(&mut self, slot: Slot) {
        if let Some(index) = self.slots.iter().position(|(s, _)| *s == slot) {
            let (_, old) = self.slots.remove(index);
            let _ = self.manager.unregister(old);
            log::info!("hotkey cleared for {slot:?}");
        }
    }

    pub fn is_registered(&self, slot: Slot) -> bool {
        self.slots.iter().any(|(s, _)| *s == slot)
    }

    /// 非阻塞地取走一次"按下"事件。
    ///
    /// 只认 `Pressed`：`Released` 是库自己用线程轮询 `GetAsyncKeyState` 补出来的，
    /// 对"触发一次动作"没有意义。
    ///
    /// 一次可能积压多条（用户连按），这里取最后一条命中的槽位即可 ——
    /// 动作都是"立即执行一次"，重复触发没有意义。
    pub fn poll_pressed(&self) -> Option<Slot> {
        if self.slots.is_empty() {
            return None;
        }
        let mut fired = None;
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.state() != HotKeyState::Pressed {
                continue;
            }
            if let Some((slot, _)) = self.slots.iter().find(|(_, h)| h.id() == event.id()) {
                fired = Some(*slot);
            }
        }
        fired
    }
}

impl Drop for HotkeyRegistrar {
    fn drop(&mut self) {
        let slots: Vec<Slot> = self.slots.iter().map(|(s, _)| *s).collect();
        for slot in slots {
            self.clear(slot);
        }
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
