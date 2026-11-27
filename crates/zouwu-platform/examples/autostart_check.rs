//! 手动验收入口：开机自启的读写与回滚。
//!
//! 用法（会真的改注册表 `HKCU\...\Run` 里名为 `zouwu` 的值）：
//!
//! ```text
//! cargo run -p zouwu-platform --example autostart_check status
//! cargo run -p zouwu-platform --example autostart_check on
//! cargo run -p zouwu-platform --example autostart_check off
//! ```
//!
//! 单独做成 example 而不是单测，是因为它**有副作用**（写用户注册表），
//! 不该在 `cargo test` 里悄悄跑。

fn main() {
    // Windows-only：改的是 `HKCU\...\Run` 注册表值。别的平台给一条说明，
    // 别让 `cargo test --workspace` 因为"没有 main"挂掉。
    #[cfg(not(target_os = "windows"))]
    {
        println!("autostart_check: Windows-only example (writes HKCU\\...\\Run)");
        return;
    }
    #[cfg(target_os = "windows")]
    {
        run();
    }
}

#[cfg(target_os = "windows")]
fn main_windows() {}
fn run() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "status".into());
    match arg.as_str() {
        "status" => println!(
            "autostart enabled = {}",
            zouwu_platform::autostart::is_enabled()
        ),
        "on" => {
            let got = zouwu_platform::autostart::set_enabled(true);
            println!("set_enabled(true) -> {}", got);
        }
        "off" => {
            let got = zouwu_platform::autostart::set_enabled(false);
            println!("set_enabled(false) -> {}", got);
        }
        other => {
            eprintln!("unknown arg `{other}` (expected status|on|off)");
            std::process::exit(2);
        }
    }
}
