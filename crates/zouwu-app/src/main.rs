//! zouwu 可执行入口。
//!
//! 两种运行形态：
//!
//! - **无参数**：作为常驻小工具启动（单实例、托盘、全局热键）。
//! - **带参数**：一次性命令行动作（安装/卸载/版本），做完就退出，不碰 GUI。
//!
//! 为什么把安装做进 exe 而不是单独一个 setup.exe：便携形态要保留
//! （拷个 exe 就能用），而"装一下"只是同一份文件的另一种调用方式。
//! 两个 exe 意味着两套签名、两份更新、两种报错。

use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    // 日志由 `zouwu_platform::logging` 接管（落盘 + 轮转 + panic hook）。
    //
    // 这里**不能**再装 env_logger：`log::set_logger` 只成功一次，先装 env_logger
    // 会让后面真装的文件日志静默失效——"日志文件一直是 0 字节"就是这么来的。
    zouwu_platform::logging::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        return command(&args);
    }

    log::info!(
        "zouwu starting {VERSION} (platform={}, config={}, installed={})",
        zouwu_platform::platform_name(),
        zouwu_platform::config_path().display(),
        cfg!(windows) && zouwu_platform::install::is_installed(),
    );

    if let Err(err) = zouwu_ui::run() {
        log::error!("fatal: {err}");
        eprintln!("zouwu failed to start: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// 命令行模式。所有子命令都必须是**显式**的——把任意参数当"配置项"会让
/// `zouwu --version` 这种最常见的调用变得难以预期。
fn command(args: &[String]) -> ExitCode {
    let silent = args.iter().any(|a| a == "--silent" || a == "-s");
    match args[0].as_str() {
        "--version" | "-V" | "version" => {
            println!("zouwu {VERSION} ({})", zouwu_platform::platform_name());
            ExitCode::SUCCESS
        }
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "--install" | "install" => install(silent),
        "--uninstall" | "uninstall" => uninstall(args, silent),
        other => {
            eprintln!("zouwu: unknown argument `{other}`\n");
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn install(silent: bool) -> ExitCode {
    match zouwu_platform::install::install(VERSION) {
        Ok(report) => {
            if !silent {
                println!("zouwu {VERSION} installed");
                println!("  program  {}", report.exe.display());
                if report.replaced {
                    println!("  (replaced the previous copy)");
                }
                println!(
                    "  autostart  {}",
                    if report.autostart { "on" } else { "off" }
                );
                match &report.shortcut {
                    Some(lnk) => println!("  shortcut   {}", lnk.display()),
                    None => println!("  shortcut   (skipped)"),
                }
                println!("  uninstall  \"{}\" --uninstall", report.exe.display());
                println!("\nRun `zouwu --uninstall` to remove it, or delete the folder.");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            if silent {
                log::error!("install failed: {err}");
            } else {
                eprintln!("install failed: {err}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn install(_silent: bool) -> ExitCode {
    eprintln!("install is only implemented for Windows");
    ExitCode::FAILURE
}

#[cfg(windows)]
fn uninstall(args: &[String], silent: bool) -> ExitCode {
    let purge = args.iter().any(|a| a == "--purge");
    match zouwu_platform::install::uninstall() {
        Ok(()) => {
            if purge {
                if let Err(err) = zouwu_platform::install::purge_data() {
                    eprintln!("could not purge data: {err}");
                } else if !silent {
                    println!("removed user data too");
                }
            }
            if !silent {
                println!("zouwu uninstalled");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            if silent {
                log::error!("uninstall failed: {err}");
            } else {
                eprintln!("uninstall failed: {err}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn uninstall(_args: &[String], _silent: bool) -> ExitCode {
    eprintln!("uninstall is only implemented for Windows");
    ExitCode::FAILURE
}

const USAGE: &str = "\
zouwu - a desktop companion that expands into a ring of shortcuts

USAGE:
    zouwu                     start the widget (tray + global hotkey)
    zouwu --install [--silent]      install into %LOCALAPPDATA%\\Programs\\zouwu
    zouwu --uninstall [--purge] [--silent]
                              remove program files, registry entries, shortcut
                              (--purge also deletes settings, skins and logs)
    zouwu --version
    zouwu --help";
