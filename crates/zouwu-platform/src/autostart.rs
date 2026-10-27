//! 开机自启（Windows：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`）。
//!
//! 为什么用注册表 Run 键而不是「启动文件夹快捷方式」或计划任务：
//! - 不需要管理员权限（`HKCU` 是当前用户）；
//! - 不产生需要清理的 `.lnk` 文件，卸载时删一个值即可；
//! - 系统在登录后拉起，语义就是"开机自启"。
//!
//! 危害控制：命令行**只写 exe 路径本身**，绝不把运行期的用户输入拼进去；
//! 删除时只删自己的值名，不碰别人。

use std::path::Path;

use crate::reg;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// 值名：稳定、可辨认，卸载时靠它精确删除。
const VALUE_NAME: &str = "zouwu";

/// 当前可执行文件路径（带引号，容忍路径里有空格）。
fn quoted(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

pub fn is_enabled() -> bool {
    reg::get_string(RUN_KEY, VALUE_NAME).is_some()
}

/// 读回注册表里登记的命令行（去掉外层引号），给「自启指向哪里」这类诊断用。
///
/// 卸载/重装后会用它确认"自启没有指向一个已经被挪走的旧 exe"。
pub fn command_path() -> Option<String> {
    let raw = reg::get_string(RUN_KEY, VALUE_NAME)?;
    Some(raw.trim_matches('"').to_string())
}

/// 让自启指向**指定**的可执行文件。
///
/// 装机时必须用这个而不是 [`set_enabled`]：安装器是"副本自己在跑"，
/// `current_exe()` 指向的是 zip 里解出来的临时目录，不是安装后的位置。
pub fn set_command(exe: &Path) -> bool {
    let ok = reg::set_string(RUN_KEY, VALUE_NAME, &quoted(exe));
    if !ok {
        log::warn!("autostart update failed (want {})", exe.display());
    }
    let actual = is_enabled();
    log::info!("autostart -> {actual} ({})", exe.display());
    actual
}

/// 打开/关闭自启，命令指向**当前进程**的 exe；返回是否**最终生效**。
pub fn set_enabled(enabled: bool) -> bool {
    if enabled {
        match std::env::current_exe() {
            Ok(exe) => set_command(&exe),
            Err(err) => {
                log::warn!("cannot resolve current exe: {err}");
                false
            }
        }
    } else {
        reg::delete_value(RUN_KEY, VALUE_NAME);
        let actual = is_enabled();
        log::info!("autostart set to false -> {actual}");
        actual
    }
}

/// 打开配置目录（托盘里的「打开配置目录」）。
pub fn open_config_dir() -> bool {
    let dir = crate::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    open_path(&dir)
}

/// 用系统默认方式打开一个路径（目录 → 资源管理器）。
pub fn open_path(path: &Path) -> bool {
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
