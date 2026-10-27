//! 日志落盘的定点自检。
#![cfg(target_os = "windows")]

fn main() {
    use log::Log;
    let logger = zouwu_platform::logging::init();
    log::info!("log_check: hello from example");
    log::warn!("log_check: warn line");
    logger.flush();
    println!(
        "log path = {}",
        zouwu_platform::logging::log_path().display()
    );
    println!("file_enabled = {}", logger.file_enabled());
    println!(
        "bytes = {:?}",
        std::fs::metadata(zouwu_platform::logging::log_path()).map(|m| m.len())
    );
}
