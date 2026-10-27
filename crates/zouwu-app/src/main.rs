//! zouwu 可执行入口。
//!
//! M0 阶段直接把演示窗口拉起来；M1 起这里会先做单实例检查、托盘图标、
//! 全局热键注册，再决定是"直接显示"还是"安静启动到托盘"。

fn main() {
    // 日志由 `zouwu_platform::logging` 接管（stderr + 落盘 + 轮转 + panic hook）。
    //
    // 这里**不能**再装 env_logger：`log::set_logger` 只成功一次，先装 env_logger
    // 会让后面真装的文件日志静默失效——"日志文件一直是 0 字节"就是这么来的。
    zouwu_platform::logging::init();

    log::info!(
        "zouwu starting (platform={}, config={})",
        zouwu_platform::platform_name(),
        zouwu_platform::config_path().display()
    );

    if let Err(err) = zouwu_ui::run() {
        log::error!("fatal: {err}");
        eprintln!("zouwu failed to start: {err}");
        std::process::exit(1);
    }
}
