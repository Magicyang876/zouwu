//! zouwu 可执行入口。
//!
//! M0 阶段直接把演示窗口拉起来；M1 起这里会先做单实例检查、托盘图标、
//! 全局热键注册，再决定是"直接显示"还是"安静启动到托盘"。

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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
