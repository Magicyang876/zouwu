//! 开机自启（Windows：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`）。
//!
//! 为什么用注册表 Run 键而不是「启动文件夹快捷方式」或计划任务：
//! - 不需要管理员权限（`HKCU` 是当前用户）；
//! - 不产生需要清理的 `.lnk` 文件，卸载时删一个值即可；
//! - 系统在登录后拉起，语义就是"开机自启"。
//!
//! 危害控制：命令行**只写 exe 路径本身**，绝不把运行期的用户输入拼进去；
//! 删除时只删自己的值名，不碰别人。

use std::ptr;

type Hkey = usize;

/// `HKEY_CURRENT_USER`（0x80000001，符号扩展成 isize 表示）。
const HKEY_CURRENT_USER: Hkey = 0x8000_0001;
const KEY_SET_VALUE: u32 = 0x0002;
const KEY_QUERY_VALUE: u32 = 0x0001;
const REG_SZ: u32 = 1;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// 值名：稳定、可辨认，卸载时靠它精确删除。
const VALUE_NAME: &str = "zouwu";

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(
        hkey: Hkey,
        sub_key: *const u16,
        options: u32,
        sam: u32,
        result: *mut Hkey,
    ) -> i32;
    fn RegSetValueExW(
        hkey: Hkey,
        value_name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        len: u32,
    ) -> i32;
    fn RegQueryValueExW(
        hkey: Hkey,
        value_name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> i32;
    fn RegDeleteValueW(hkey: Hkey, value_name: *const u16) -> i32;
    fn RegCloseKey(hkey: Hkey) -> i32;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_run_key(sam: u32) -> Option<Hkey> {
    let sub = wide(RUN_KEY);
    let mut key: Hkey = 0;
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, sam, &mut key) };
    if status != 0 {
        log::debug!("RegOpenKeyExW(Run) failed: {status}");
        return None;
    }
    Some(key)
}

/// 当前可执行文件路径（带引号，容忍路径里有空格）。
fn command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\"", exe.display()))
}

pub fn is_enabled() -> bool {
    let Some(key) = open_run_key(KEY_QUERY_VALUE | KEY_SET_VALUE) else {
        return false;
    };
    let name = wide(VALUE_NAME);
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    unsafe { RegCloseKey(key) };
    status == 0
}

/// 打开/关闭自启；返回是否**最终生效**（读回校验，避免"看起来成功了"）。
pub fn set_enabled(enabled: bool) -> bool {
    let Some(key) = open_run_key(KEY_SET_VALUE | KEY_QUERY_VALUE) else {
        log::warn!("cannot open Run key; autostart unchanged");
        return is_enabled();
    };
    let name = wide(VALUE_NAME);

    let status = if enabled {
        let Some(cmd) = command() else {
            unsafe { RegCloseKey(key) };
            return is_enabled();
        };
        // REG_SZ 需要以 UTF-16 NUL 结尾，长度含结尾的 0。
        let data: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            )
        }
    } else {
        // 值不存在时返回非 0，这不是错误。
        unsafe { RegDeleteValueW(key, name.as_ptr()) }
    };
    unsafe { RegCloseKey(key) };

    if status != 0 {
        log::warn!("autostart update failed: {status} (want {enabled})");
    }
    let actual = is_enabled();
    log::info!("autostart set to {enabled} -> {actual}");
    actual
}

/// 打开配置目录（托盘里的「打开配置目录」）。
pub fn open_config_dir() -> bool {
    let dir = crate::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    open_path(&dir)
}

/// 用系统默认方式打开一个路径（目录 → 资源管理器）。
pub fn open_path(path: &std::path::Path) -> bool {
    let mut cmd = std::process::Command::new("explorer");
    cmd.arg(path);
    match cmd.spawn() {
        Ok(_) => true,
        Err(err) => {
            log::warn!("explorer failed: {err}");
            false
        }
    }
}
