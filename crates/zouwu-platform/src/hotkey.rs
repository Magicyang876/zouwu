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
    /// 环上第 `index` 颗球（按**配置顺序**，不是屏幕位置）。
    ///
    /// 用下标而不是固定槽位：球的清单是用户配的（ADR-042），
    /// 槽位得跟着清单走——每一颗球都可以有自己的一条热键。
    RingItem(usize),
}

/// 真正碰系统的那两个动作（`RegisterHotKey` / `UnregisterHotKey`）。
///
/// 抽成 trait 只为了可测：单元测试里既不该也不注册得上热键，而
/// "先让出旧键、再注册新键、注册失败把旧键放回去"这套顺序恰恰是最容易写错的部分。
pub trait Backend {
    fn register(&self, hotkey: HotKey) -> Result<(), String>;
    fn unregister(&self, hotkey: HotKey) -> Result<(), String>;
}

impl Backend for GlobalHotKeyManager {
    fn register(&self, hotkey: HotKey) -> Result<(), String> {
        GlobalHotKeyManager::register(self, hotkey).map_err(|err| err.to_string())
    }

    fn unregister(&self, hotkey: HotKey) -> Result<(), String> {
        GlobalHotKeyManager::unregister(self, hotkey).map_err(|err| err.to_string())
    }
}

/// 热键注册器：按槽位各保留一条。
pub struct HotkeyRegistrar<B: Backend = GlobalHotKeyManager> {
    manager: B,
    slots: Vec<(Slot, HotKey)>,
}

impl HotkeyRegistrar<GlobalHotKeyManager> {
    /// 必须在主线程（有消息循环的线程）上调用。
    pub fn new() -> Result<Self, String> {
        let manager =
            GlobalHotKeyManager::new().map_err(|err| format!("hotkey init failed: {err}"))?;
        Ok(Self {
            manager,
            slots: Vec::new(),
        })
    }
}

impl<B: Backend> HotkeyRegistrar<B> {
    /// 给某个槽位换成新的热键。注册失败时**保留该槽位的旧热键**，
    /// 并把错误交给调用方写日志。
    ///
    /// 顺序是**先让旧键、再注册新键**：反过来写的话，配置重载时（键位没变）
    /// 系统会回 `HotKey already registered`——占着它的是本进程自己，
    /// 于是每次重载都留一条 WARN，真正的注册失败反而看不见了。
    pub fn set(&mut self, slot: Slot, spec: &Hotkey) -> Result<(), String> {
        let hotkey = HotKey::new(modifiers(spec), code(spec.key)?);
        let index = self.slots.iter().position(|(s, _)| *s == slot);
        // 键没变就别惊动系统：改配置、切皮肤都会重放一遍，这是常态。
        if let Some(index) = index {
            if self.slots[index].1 == hotkey {
                log::debug!("hotkey unchanged: {spec} for {slot:?}");
                return Ok(());
            }
        }
        let old = index.map(|index| self.slots.remove(index).1);
        if let Some(old) = old {
            // 旧的可能已被系统回收；失败不算错（我们只是要让出这个键位）。
            if let Err(err) = self.manager.unregister(old) {
                log::debug!("unregister old hotkey failed: {err}");
            }
        }
        match self.manager.register(hotkey) {
            Ok(()) => {
                self.slots.push((slot, hotkey));
                log::info!(
                    "hotkey registered: {spec} for {slot:?} (id {})",
                    hotkey.id()
                );
                Ok(())
            }
            Err(err) => {
                // 新的没注册上就把原来那颗放回去：呼出热键丢了，用户就打不开小兽。
                match old {
                    Some(old) => match self.manager.register(old) {
                        Ok(()) => {
                            self.slots.push((slot, old));
                            log::warn!("hotkey `{spec}` rejected ({err}); kept the previous one");
                        }
                        Err(back) => log::warn!(
                            "hotkey `{spec}` rejected ({err}), previous one could not be restored ({back})"
                        ),
                    },
                    None => log::warn!("hotkey `{spec}` rejected ({err})"),
                }
                Err(format!("register {spec} failed: {err}"))
            }
        }
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

    /// 放掉下标 `>= len` 的球槽位（删球/减球之后要调）。
    ///
    /// 不调的话，被删掉那颗球的热键仍然被本进程占着：别的程序抢不到，
    /// 用户按下去我们也只会查到一个不存在的球（静默无反应）——最难查的那种。
    pub fn retain_ring_items(&mut self, len: usize) {
        let stale: Vec<Slot> = self
            .slots
            .iter()
            .filter_map(|(slot, _)| match slot {
                Slot::RingItem(index) if *index >= len => Some(*slot),
                _ => None,
            })
            .collect();
        for slot in stale {
            self.clear(slot);
        }
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

impl<B: Backend> Drop for HotkeyRegistrar<B> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 记账用的假后端：既不碰系统，也能按需要拒绝某个键。
    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<String>>,
        reject: RefCell<Vec<Key>>,
    }

    impl Fake {
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }

        fn reject(&self, key: Key) {
            self.reject.borrow_mut().push(key);
        }
    }

    impl Backend for Fake {
        fn register(&self, hotkey: HotKey) -> Result<(), String> {
            let name = format!("register {}", hotkey.id());
            self.calls.borrow_mut().push(name);
            let refused = self
                .reject
                .borrow()
                .iter()
                .any(|key| code(*key).is_ok_and(|code| code == hotkey.key));
            if refused {
                return Err("HotKey already registered".to_string());
            }
            Ok(())
        }

        fn unregister(&self, hotkey: HotKey) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("unregister {}", hotkey.id()));
            Ok(())
        }
    }

    fn registrar() -> HotkeyRegistrar<Fake> {
        HotkeyRegistrar {
            manager: Fake::default(),
            slots: Vec::new(),
        }
    }

    fn spec(raw: &str) -> Hotkey {
        Hotkey::parse(raw).expect("parse hotkey")
    }

    #[test]
    fn reloading_the_same_hotkey_does_not_touch_the_system() {
        let mut registrar = registrar();
        let summon = spec("Alt+Space");
        registrar.set(Slot::Summon, &summon).expect("first");
        let first = registrar.manager.calls();
        registrar.set(Slot::Summon, &summon).expect("second");
        // 第二次一条调用都不该发生：这正是配置重载刷出
        // `HotKey already registered` 那个假警告的原因。
        assert_eq!(registrar.manager.calls(), first);
    }

    #[test]
    fn a_changed_hotkey_is_released_before_the_new_one_is_taken() {
        let mut registrar = registrar();
        registrar
            .set(Slot::Summon, &spec("Alt+Space"))
            .expect("first");
        registrar
            .set(Slot::Summon, &spec("Ctrl+Alt+Space"))
            .expect("second");
        let calls = registrar.manager.calls();
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert!(calls[0].starts_with("register "), "{calls:?}");
        // 先让出旧键，再注册新键。
        assert!(calls[1].starts_with("unregister "), "{calls:?}");
        assert!(calls[2].starts_with("register "), "{calls:?}");
        assert_eq!(registrar.slots.len(), 1);
        assert_eq!(registrar.slots[0].1.key, Code::Space);
        assert_eq!(
            registrar.slots[0].1.mods,
            Modifiers::CONTROL | Modifiers::ALT
        );
    }

    #[test]
    fn a_rejected_hotkey_leaves_the_previous_one_working() {
        let mut registrar = registrar();
        registrar
            .set(Slot::Summon, &spec("Alt+Space"))
            .expect("first");
        registrar.manager.reject(Key::Tab);
        let err = registrar
            .set(Slot::Summon, &spec("Ctrl+Alt+Tab"))
            .expect_err("must be rejected");
        assert!(err.contains("Ctrl+Alt+Tab"), "{err}");
        // 呼出热键丢了就打不开小兽：必须换回原来那颗。
        assert!(registrar.is_registered(Slot::Summon));
        assert_eq!(registrar.slots[0].1.key, Code::Space);
        assert_eq!(registrar.slots[0].1.mods, Modifiers::ALT);
    }

    #[test]
    fn clearing_a_slot_releases_the_key() {
        let mut registrar = registrar();
        registrar
            .set(Slot::RingItem(0), &spec("Ctrl+Alt+V"))
            .expect("set");
        registrar.clear(Slot::RingItem(0));
        assert!(!registrar.is_registered(Slot::RingItem(0)));
        let calls = registrar.manager.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[1].starts_with("unregister "), "{calls:?}");
    }

    #[test]
    fn dropping_a_ball_releases_its_hotkey() {
        let mut registrar = registrar();
        registrar
            .set(Slot::RingItem(0), &spec("Ctrl+Alt+V"))
            .expect("0");
        registrar
            .set(Slot::RingItem(1), &spec("Ctrl+Alt+Q"))
            .expect("1");
        registrar.retain_ring_items(1);
        assert!(registrar.is_registered(Slot::RingItem(0)));
        assert!(!registrar.is_registered(Slot::RingItem(1)));
    }
}
