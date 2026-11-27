//! 全局热键（Linux）。
//!
//! ## 为什么不是 `global-hotkey`
//!
//! `global-hotkey` 在 Linux 上用的是 **X11** 的 `XGrabKey`。XWayland 里
//! `XGrabKey` 依然会"成功返回"，但它只抓 XWayland 自己的键事件——合成器
//! 根本不会把按键转发给它，于是用户看到的状态是"注册成功了，按下去毫无反应"。
//!
//! 纯 Wayland 下客户端拿不到全局键盘，只有两条正路：
//!
//! 1. `org.freedesktop.portal.GlobalShortcuts`（跨桌面，但每次绑定都要弹一个
//!    授权对话框，对常驻托盘的小工具是灾难——改一次配置弹一次）；
//! 2. KDE 的 `org.kde.kglobalaccel`（KWin 自己维护的全局快捷键服务）。
//!
//! 这里走第 2 条：`doRegister` + `setShortcutKeys` 直接落进 KDE 的快捷键系统
//! （`kglobalshortcutsrc`，用户在「系统设置 → 快捷键」里能看见、能改、能停用），
//! 绑定过程不打扰用户，按下时由 `/component/<名字>` 发
//! `globalShortcutPressed` 信号给我们。
//!
//! 没有 kglobalaccel（非 KDE、或 X11 会话）时退回到 `global-hotkey`——
//! 那是纯 X11 会话下唯一可用的路径，也是改造前的行为。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use zouwu_core::hotkey::{Hotkey, Key};

pub use crate::hotkey_x11::Slot;

/// UI 侧塞进来的"醒一醒"回调：热键是**从 D-Bus 线程**来的，winit 的事件循环
/// 在歇着（`ControlFlow::Wait`），不叫它一声就没人去取队列。
type Waker = Arc<dyn Fn() + Send + Sync>;

fn waker_slot() -> &'static Mutex<Option<Waker>> {
    static WAKER: OnceLock<Mutex<Option<Waker>>> = OnceLock::new();
    WAKER.get_or_init(|| Mutex::new(None))
}

/// 注册唤醒回调（`zouwu-ui` 在装上自己的 event loop proxy 之后调一次）。
pub fn set_waker(waker: Waker) {
    if let Ok(mut slot) = waker_slot().lock() {
        *slot = Some(waker);
    }
}

fn wake() {
    let waker = waker_slot().lock().ok().and_then(|slot| slot.clone());
    if let Some(waker) = waker {
        waker();
    }
}

/// 热键注册器：KDE 的 kglobalaccel 优先，退化到 X11。
pub struct HotkeyRegistrar {
    inner: Inner,
}

enum Inner {
    Kde(kde::Registrar),
    X11(crate::hotkey_x11::HotkeyRegistrar),
}

impl HotkeyRegistrar {
    /// 会挑后端：kglobalaccel 在就用它（Wayland 下唯一可靠的一条），否则退回 X11。
    pub fn new() -> Result<Self, String> {
        match kde::Registrar::new() {
            Ok(registrar) => {
                log::info!("hotkeys: using KDE kglobalaccel (Wayland-safe)");
                Ok(Self {
                    inner: Inner::Kde(registrar),
                })
            }
            Err(err) => {
                log::warn!("hotkeys: kglobalaccel unavailable ({err}); falling back to X11");
                let registrar = crate::hotkey_x11::HotkeyRegistrar::new()?;
                Ok(Self {
                    inner: Inner::X11(registrar),
                })
            }
        }
    }

    pub fn set(&mut self, slot: Slot, spec: &Hotkey) -> Result<(), String> {
        match &mut self.inner {
            Inner::Kde(registrar) => registrar.set(slot, spec),
            Inner::X11(registrar) => registrar.set(slot, spec),
        }
    }

    pub fn clear(&mut self, slot: Slot) {
        match &mut self.inner {
            Inner::Kde(registrar) => registrar.clear(slot),
            Inner::X11(registrar) => registrar.clear(slot),
        }
    }

    pub fn is_registered(&self, slot: Slot) -> bool {
        match &self.inner {
            Inner::Kde(registrar) => registrar.is_registered(slot),
            Inner::X11(registrar) => registrar.is_registered(slot),
        }
    }

    pub fn retain_ring_items(&mut self, len: usize) {
        match &mut self.inner {
            Inner::Kde(registrar) => registrar.retain_ring_items(len),
            Inner::X11(registrar) => registrar.retain_ring_items(len),
        }
    }

    pub fn poll_pressed(&self) -> Option<Slot> {
        match &self.inner {
            Inner::Kde(registrar) => registrar.poll_pressed(),
            Inner::X11(registrar) => registrar.poll_pressed(),
        }
    }
}

/// KDE 的全局快捷键服务。
mod kde {
    use super::*;

    const SERVICE: &str = "org.kde.kglobalaccel";
    const PATH: &str = "/kglobalaccel";
    const IFACE: &str = "org.kde.KGlobalAccel";
    /// 组件名：会变成 `kglobalshortcutsrc` 里的 `[zouwu]` 段，也是
    /// `/component/zouwu` 这个对象路径。
    const COMPONENT: &str = "zouwu";
    const COMPONENT_FRIENDLY: &str = "Zouwu";

    /// `KGlobalAccel::NoAutoloading`（KF6 `kglobalaccel.h`：`NoAutoloading = 0x4`）。
    ///
    /// 它同时是“别预加载旧值”和“我这次给的键就是准的，请应用”的开关。KDE 的文档把
    /// 另一半说得很直白：不带这个标志就是 autoloading——动作已经登记过就**保留旧键**、
    /// 把新键当没看见，而回包照样是非空的（回的是旧键），看起来和成功一模一样。
    ///
    /// 这里曾经填的是 `2`：一个不存在的标志值，行为等同 autoloading。于是首次注册能上、
    /// 运行期改键永远不生效，日志还写着 `hotkey registered: …`（ADR-073）。
    pub(super) const NO_AUTOLOADING: u32 = 0x4;

    // Qt 的键码：字母/数字与小写 ASCII 同值，功能键从 0x01000030 起。
    const QT_KEY_ENTER: i32 = 0x0100_0004;
    const QT_KEY_TAB: i32 = 0x0100_0001;
    const QT_KEY_ESC: i32 = 0x0100_0000;
    const QT_KEY_BACKSPACE: i32 = 0x0100_0003;
    const QT_KEY_DELETE: i32 = 0x0100_0007;
    const QT_KEY_LEFT: i32 = 0x0100_0012;
    const QT_KEY_UP: i32 = 0x0100_0013;
    const QT_KEY_RIGHT: i32 = 0x0100_0014;
    const QT_KEY_DOWN: i32 = 0x0100_0015;
    const QT_KEY_F1: i32 = 0x0100_0030;

    const QT_SHIFT: i32 = 0x0200_0000;
    const QT_CTRL: i32 = 0x0400_0000;
    const QT_ALT: i32 = 0x0800_0000;
    const QT_META: i32 = 0x1000_0000;

    /// 按下过的快捷键（动作名），由 D-Bus 线程写、UI 线程取。
    ///
    /// 用全局队列而不是 channel：后端只会有**一个**注册器（和 Windows 那边
    /// "每个注册器都会建自己的隐藏窗口"是同一个理由），全局队列省掉一堆
    /// 所有权搬运，也不会因为重复构造而丢事件。
    fn pressed_queue() -> &'static Mutex<Vec<String>> {
        static QUEUE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
        QUEUE.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// 信号监听线程只启动一次：它属于服务而不是某个注册器。
    fn listener_started() -> &'static AtomicBool {
        static STARTED: AtomicBool = AtomicBool::new(false);
        &STARTED
    }

    /// Qt 的键码 + 修饰位，和 `QKeySequence` 的整型表示同构。
    pub(super) fn qt_sequence(hotkey: &Hotkey) -> i32 {
        let mut code = match hotkey.key {
            Key::Space => ' ' as i32,
            Key::Enter => QT_KEY_ENTER,
            Key::Tab => QT_KEY_TAB,
            Key::Escape => QT_KEY_ESC,
            Key::Backspace => QT_KEY_BACKSPACE,
            Key::Delete => QT_KEY_DELETE,
            Key::Up => QT_KEY_UP,
            Key::Down => QT_KEY_DOWN,
            Key::Left => QT_KEY_LEFT,
            Key::Right => QT_KEY_RIGHT,
            Key::Letter(c) => c.to_ascii_uppercase() as i32,
            Key::Digit(c) => c as i32,
            Key::Function(n) => QT_KEY_F1 + (n.clamp(1, 35) as i32 - 1),
        };
        if hotkey.shift {
            code |= QT_SHIFT;
        }
        if hotkey.ctrl {
            code |= QT_CTRL;
        }
        if hotkey.alt {
            code |= QT_ALT;
        }
        if hotkey.meta {
            code |= QT_META;
        }
        code
    }

    pub(super) fn action_name(slot: Slot) -> String {
        match slot {
            Slot::Summon => "summon".to_string(),
            Slot::RingItem(index) => format!("ring-{index}"),
        }
    }

    fn action_friendly(slot: Slot) -> String {
        match slot {
            Slot::Summon => "Summon".to_string(),
            Slot::RingItem(index) => format!("Ring item {}", index + 1),
        }
    }

    fn action_id(slot: Slot) -> Vec<String> {
        vec![
            COMPONENT.to_string(),
            action_name(slot),
            COMPONENT_FRIENDLY.to_string(),
            action_friendly(slot),
        ]
    }

    pub struct Registrar {
        conn: zbus::blocking::Connection,
        /// 槽位 → 动作名（信号回来时用来反查槽位）。
        bound: HashMap<Slot, String>,
        /// 动作名 → 槽位。
        reverse: HashMap<String, Slot>,
        /// 已经绑上的槽位（失败的不算）。
        active: HashSet<Slot>,
    }

    impl Registrar {
        pub fn new() -> Result<Self, String> {
            let conn = zbus::blocking::Connection::session()
                .map_err(|err| format!("D-Bus session: {err}"))?;
            let owned = conn
                .call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "NameHasOwner",
                    &(SERVICE,),
                )
                .map_err(|err| format!("kglobalaccel probe: {err}"))?
                .body()
                .deserialize::<bool>()
                .map_err(|err| format!("kglobalaccel probe: {err}"))?;
            if !owned {
                return Err(format!("{SERVICE} is not running"));
            }
            let registrar = Self {
                conn,
                bound: HashMap::new(),
                reverse: HashMap::new(),
                active: HashSet::new(),
            };
            registrar.spawn_listener()?;
            Ok(registrar)
        }

        /// 起一条线程收 `globalShortcutPressed`。
        ///
        /// 线程只负责"收到 → 入队 → 叫醒 UI"，不碰任何 UI 状态；
        /// 事件循环退出后线程会因为连接被关掉而自己结束。
        fn spawn_listener(&self) -> Result<(), String> {
            if listener_started().swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.kde.kglobalaccel.Component")
                .map_err(|err| format!("match rule: {err}"))?
                .member("globalShortcutPressed")
                .map_err(|err| format!("match rule: {err}"))?
                .build();
            let conn = self.conn.clone();
            std::thread::Builder::new()
                .name("zouwu-hotkeys".into())
                .spawn(move || {
                    let mut iter = match zbus::blocking::MessageIterator::for_match_rule(
                        rule,
                        &conn,
                        Some(64),
                    ) {
                        Ok(iter) => iter,
                        Err(err) => {
                            log::error!("hotkeys: cannot watch kglobalaccel signals: {err}");
                            return;
                        }
                    };
                    for msg in iter.by_ref() {
                        let Ok(msg) = msg else {
                            break;
                        };
                        let Ok((component, action, _timestamp)) =
                            msg.body().deserialize::<(String, String, i64)>()
                        else {
                            continue;
                        };
                        if component != COMPONENT {
                            continue;
                        }
                        log::debug!("hotkeys: kglobalaccel fired `{action}`");
                        if let Ok(mut queue) = pressed_queue().lock() {
                            queue.push(action);
                        }
                        wake();
                    }
                    log::debug!("hotkeys: kglobalaccel listener stopped");
                })
                .map_err(|err| format!("hotkey thread: {err}"))?;
            Ok(())
        }

        pub fn set(&mut self, slot: Slot, spec: &Hotkey) -> Result<(), String> {
            let id = action_id(slot);
            // 1) 先登记这个动作。重复登记是幂等的（KDE 会直接忽略）。
            self.conn
                .call_method(
                    Some(SERVICE),
                    PATH,
                    Some(IFACE),
                    "doRegister",
                    &(id.clone(),),
                )
                .map_err(|err| format!("doRegister({}): {err}", action_name(slot)))?;
            // 2) 再把键绑上去。返回值是**实际生效**的键：被别的程序占用时是空表。
            let keys: Vec<(Vec<i32>,)> = vec![(vec![qt_sequence(spec)],)];
            let assigned = self
                .conn
                .call_method(
                    Some(SERVICE),
                    PATH,
                    Some(IFACE),
                    "setShortcutKeys",
                    &(id, keys, NO_AUTOLOADING),
                )
                .map_err(|err| format!("setShortcutKeys({}): {err}", action_name(slot)))?
                .body()
                .deserialize::<Vec<(Vec<i32>,)>>()
                .map_err(|err| format!("setShortcutKeys({}): {err}", action_name(slot)))?;
            if assigned.is_empty() {
                self.active.remove(&slot);
                return Err(format!("`{spec}` is already taken by another application"));
            }
            // 回包是“实际生效的键”。非空但不是我们给的那颗，就是系统把新键丢了、
            // 留着旧的（未知标志、被占用后回退都是这个形状）。这种也必须报错，
            // 否则日志里的 `hotkey registered` 是假的，用户只能靠自己按按看。
            let wanted = qt_sequence(spec);
            let applied = assigned.first().and_then(|(keys,)| keys.first()).copied();
            if applied != Some(wanted) {
                self.active.remove(&slot);
                return Err(match applied {
                    Some(kept) => format!("kglobalaccel kept 0x{kept:08x}, not `{spec}`"),
                    None => format!("kglobalaccel assigned nothing for `{spec}`"),
                });
            }
            let action = action_name(slot);
            self.reverse.insert(action.clone(), slot);
            self.bound.insert(slot, action.clone());
            self.active.insert(slot);
            log::info!("hotkey registered: {spec} for {slot:?} (kglobalaccel `{action}`)");
            Ok(())
        }

        pub fn clear(&mut self, slot: Slot) {
            self.active.remove(&slot);
            let Some(action) = self.bound.remove(&slot) else {
                return;
            };
            self.reverse.remove(&action);
            // 直接注销动作：`kglobalshortcutsrc` 里不会留下 `none,none` 这种垃圾行。
            let id = action_id(slot);
            if let Err(err) =
                self.conn
                    .call_method(Some(SERVICE), PATH, Some(IFACE), "unRegister", &(id,))
            {
                log::warn!("hotkeys: unRegister({action}): {err}");
            }
        }

        pub fn is_registered(&self, slot: Slot) -> bool {
            self.active.contains(&slot)
        }

        pub fn retain_ring_items(&mut self, len: usize) {
            let extra: Vec<Slot> = self
                .bound
                .keys()
                .copied()
                .filter(|slot| matches!(slot, Slot::RingItem(index) if *index >= len))
                .collect();
            for slot in extra {
                self.clear(slot);
            }
        }

        pub fn poll_pressed(&self) -> Option<Slot> {
            let action = {
                let mut queue = pressed_queue().lock().ok()?;
                if queue.is_empty() {
                    return None;
                }
                queue.remove(0)
            };
            let slot = self.reverse.get(&action).copied();
            if slot.is_none() {
                log::debug!("hotkeys: pressed unknown action `{action}`");
            }
            slot
        }
    }
}

#[cfg(test)]
mod tests {
    use super::kde;
    use zouwu_core::hotkey::Hotkey;

    fn seq(spec: &str) -> i32 {
        kde::qt_sequence(&Hotkey::parse(spec).expect("valid hotkey"))
    }

    /// 这几个值是从 kglobalaccel 真实往返里读回来的（`shortcutKeys`）：
    /// Alt+Space = 0x08000020。写错一位，快捷键就会静默失效。
    #[test]
    fn qt_key_codes_match_what_kde_stores() {
        assert_eq!(seq("Alt+Space"), 0x0800_0020);
        assert_eq!(seq("Ctrl+Q"), 0x0400_0000 | 'Q' as i32);
        assert_eq!(seq("Shift+F5"), 0x0200_0000 | (0x0100_0030 + 4));
        assert_eq!(seq("Ctrl+Alt+V"), 0x0400_0000 | 0x0800_0000 | 'V' as i32);
    }

    /// 动作名会进 `kglobalshortcutsrc`，必须稳定且大小写一致。
    #[test]
    fn action_names_are_stable() {
        use super::Slot;
        assert_eq!(kde::action_name(Slot::Summon), "summon");
        assert_eq!(kde::action_name(Slot::RingItem(2)), "ring-2");
    }

    /// `NoAutoloading` 的取值来自 KF6 头文件，不是我们自己定的。KF6 里 `0x4` 才是
    /// NoAutoloading，`0x2` 什么都不是；拿 `0x2` 去改键，kglobalaccel 会当作 autoloading
    /// 丢掉新键（已在真机上对着 `org.kde.KGlobalAccel.setShortcutKeys` 逐个试过：
    /// 0/2 返回旧键，4/8/10/12 返回新键）。
    #[test]
    fn no_autoloading_matches_kf6_header() {
        assert_eq!(kde::NO_AUTOLOADING, 0x4);
    }
}
