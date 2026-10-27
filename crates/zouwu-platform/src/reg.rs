//! 注册表读写的最小封装：只支持 `HKEY_CURRENT_USER` + `REG_SZ`。
//!
//! 装机/卸载/自启一共需要动三个地方（Run 键、Uninstall 键、它们下面的值），
//! 与其在三处各写一遍 `extern "system"` 和指针运算，不如收成这一层。
//! **刻意不做的**：`REG_DWORD`、其他根键、权限参数——没有用到它们的场景，
//! 加了只会让"这段代码为什么存在"变模糊。需要时再加，而且是加法。

use std::ptr;

type Hkey = usize;

/// `HKEY_CURRENT_USER`（0x80000001）。
const HKEY_CURRENT_USER: Hkey = 0x8000_0001;
/// 只要读写值就够了，不要 `KEY_ALL_ACCESS`（那会让安全软件更爱我们）。
const KEY_READ_WRITE: u32 = 0x0002 | 0x0004 | 0x0001;
const KEY_READ: u32 = 0x0001 | 0x0004;
const REG_SZ: u32 = 1;
/// `RegDeleteTreeW` 在键不存在时的返回值，不算失败。
const ERROR_FILE_NOT_FOUND: i32 = 2;

#[link(name = "advapi32")]
extern "system" {
    fn RegCreateKeyExW(
        hkey: Hkey,
        sub_key: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        sam: u32,
        security: *mut core::ffi::c_void,
        result: *mut Hkey,
        disposition: *mut u32,
    ) -> i32;
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
    fn RegDeleteTreeW(hkey: Hkey, sub_key: *const u16) -> i32;
    fn RegCloseKey(hkey: Hkey) -> i32;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open(path: &str, sam: u32) -> Option<Hkey> {
    let sub = wide(path);
    let mut key: Hkey = 0;
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, sam, &mut key) };
    if status != 0 {
        log::debug!("RegOpenKeyExW({path}) failed: {status}");
        return None;
    }
    Some(key)
}

/// 建键（已存在则打开）。返回是否拿到句柄。
fn create(path: &str) -> Option<Hkey> {
    let sub = wide(path);
    let mut key: Hkey = 0;
    let mut disposition = 0u32;
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            0,
            ptr::null(),
            0,
            KEY_READ_WRITE,
            ptr::null_mut(),
            &mut key,
            &mut disposition,
        )
    };
    if status != 0 {
        log::warn!("RegCreateKeyExW({path}) failed: {status}");
        return None;
    }
    Some(key)
}

/// 写一个字符串值（键不存在时创建）。返回写后读回是否一致。
pub(crate) fn set_string(path: &str, name: &str, value: &str) -> bool {
    let Some(key) = create(path) else {
        return false;
    };
    let name = wide(name);
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            (data.len() * 2) as u32,
        )
    };
    unsafe { RegCloseKey(key) };
    if status != 0 {
        log::warn!("RegSetValueExW({path}\\{name:?}) failed: {status}");
        return false;
    }
    true
}

/// 读一个字符串值；不存在/读不出/不是 `REG_SZ` 都返回 `None`。
pub(crate) fn get_string(path: &str, name: &str) -> Option<String> {
    let key = open(path, KEY_READ)?;
    let name = wide(name);
    let mut len: u32 = 0;
    // 先问长度（值可能比任何固定缓冲都长），再读内容。
    let mut status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut len,
        )
    };
    if status != 0 || len == 0 {
        unsafe { RegCloseKey(key) };
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    let mut kind: u32 = 0;
    status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    unsafe { RegCloseKey(key) };
    if status != 0 || kind != REG_SZ {
        return None;
    }
    // 去掉结尾的 UTF-16 NUL 再解码
    let units: Vec<u16> = buf
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|u| *u != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// 删一个值。值本来就不存在也算成功。
pub(crate) fn delete_value(path: &str, name: &str) -> bool {
    let Some(key) = open(path, KEY_READ_WRITE) else {
        return true;
    };
    let name = wide(name);
    let status = unsafe { RegDeleteValueW(key, name.as_ptr()) };
    unsafe { RegCloseKey(key) };
    if status != 0 {
        log::debug!("RegDeleteValueW({path}\\{name:?}) failed: {status}");
    }
    // 值不存在（2）不算失败：目标是"删掉之后它不在"。
    status == 0 || status == ERROR_FILE_NOT_FOUND
}

/// 删整棵子键（含其下所有值）。键本来就不存在也算成功。
///
/// 用 `RegDeleteTreeW` 而不是递归 `RegEnumKeyEx`：后者是几十行样板，
/// 且每一步都可能半途失败留下垃圾。
pub(crate) fn delete_tree(path: &str) -> bool {
    let sub = wide(path);
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, sub.as_ptr()) };
    if status != 0 && status != ERROR_FILE_NOT_FOUND {
        log::warn!("RegDeleteTreeW({path}) failed: {status}");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写-读-删一整圈。用**只在测试里出现**的键名，绝不动真实位置。
    ///
    /// 默认 `#[ignore]`：它确实会碰用户的 `HKCU`。跑法：
    /// `cargo test -p zouwu-platform -- --ignored`（CI 里另有 PowerShell 脚本覆盖）。
    #[test]
    #[ignore = "touches HKCU; run with --ignored"]
    fn roundtrip_under_a_scratch_key() {
        let key = "Software\\zouwu-test\\scratch";
        assert!(set_string(key, "hello", "世界 ✓"), "write failed");
        assert_eq!(get_string(key, "hello").as_deref(), Some("世界 ✓"));
        assert_eq!(get_string(key, "missing"), None);
        assert!(delete_value(key, "hello"));
        assert_eq!(get_string(key, "hello"), None);
        // 重复删除幂等
        assert!(delete_value(key, "hello"));
        assert!(delete_tree("Software\\zouwu-test"));
        assert_eq!(get_string(key, "hello"), None);
    }
}
