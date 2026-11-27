//! 日志落盘的定点自检。
fn main() {
    // 这个例子当初是为 Windows 的落盘路径写的；在别的平台留一条说明而不是
    // 让 `cargo test --workspace` 因为"没有 main"直接编译失败。
    #[cfg(not(target_os = "windows"))]
    {
        println!("log_check: Windows-only example; run it on Windows to inspect the log file");
        return;
    }
    #[cfg(target_os = "windows")]
    {
        run();
    }
}

#[cfg(target_os = "windows")]
fn run() {
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
