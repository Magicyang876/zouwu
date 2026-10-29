//! 跑一条用户写的命令 / 打开一个程序。
//!
//! 自定义气泡球最终都落到这里：`command` 是用户在设置面板里填的那一行。
//! 三种情况分开处理，因为它们的"正确行为"并不一样：
//!
//! 1. 就是一个存在的 `.exe`（或任何可执行文件路径）→ **直接** spawn。
//!    走 `cmd /C` 会多出一个 cmd 窗口一闪，而且带空格的路径要额外加引号；
//! 2. 其他命令行（`notepad.exe`, `code .`, `mstsc /v:host`）→ `cmd /C`。
//!    `cmd` 负责解析引号、找 PATH 里的程序、执行 `start` 这类内建命令；
//! 3. 空 → 用户还没填，什么都不做，也不算错误。
//!
//! **不等待子进程退出**：这是托盘小工具的动作按钮，不是脚本解释器。
//! 一个 `npm run dev` 会把整个程序卡到死。

use std::path::Path;
use std::process::{Command, Stdio};

/// 跑一条命令。返回 `Err` 时调用方应该弹窗告诉用户（托盘工具没人看日志）。
pub fn run(command: &str) -> Result<(), String> {
    let raw = command.trim();
    if raw.is_empty() {
        return Ok(());
    }

    let direct = Path::new(raw);
    let mut cmd = if direct.is_file() {
        let mut cmd = Command::new(direct);
        // 工作目录设成程序自己所在的目录：很多绿色软件按相对路径找自己的资源，
        // 而我们的当前目录是 zouwu 的安装目录。
        if let Some(dir) = direct.parent() {
            cmd.current_dir(dir);
        }
        cmd
    } else {
        let mut cmd = Command::new(if cfg!(windows) { "cmd.exe" } else { "sh" });
        cmd.arg(if cfg!(windows) { "/C" } else { "-c" }).arg(raw);
        cmd
    };

    // 三个标准流都丢掉：
    // - stdout/stderr 接着父进程的话，子进程的输出会写进 zouwu 自己的日志管道，
    //   长驻程序（`ping -t`）还会把管道塞满（ADR-037 就是这么卡死的）；
    // - stdin 继承的话，子进程可能抢走键盘输入。
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Windows：不弹黑色控制台窗口。0x0800_0000 = CREATE_NO_WINDOW。
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.spawn() {
        Ok(child) => {
            log::info!("launched `{raw}` (pid {})", child.id());
            Ok(())
        }
        Err(err) => Err(format!("{raw}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn empty_command_is_a_no_op() {
        // 用户刚加了一颗球、还没填命令：按下去什么都不该发生。
        assert!(run("").is_ok());
        assert!(run("   ").is_ok());
    }
}
