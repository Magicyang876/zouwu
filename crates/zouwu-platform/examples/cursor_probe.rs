//! 光标读数探针：按与 `poll_pointer` 完全相同的路径采样 `GetCursorPos`，
//! 用来区分「系统/注入本身有跳变」与「zouwu 的轮询逻辑有问题」。
//!
//! 用法：`cargo run -p zouwu-platform --example cursor_probe -- 3000`

use std::time::{Duration, Instant};

fn main() {
    let ms: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000);
    let start = Instant::now();
    let mut prev: Option<(i32, i32)> = None;
    let mut jumps = 0u32;
    let mut n = 0u32;
    while start.elapsed() < Duration::from_millis(ms) {
        let (pos, down) = match (
            zouwu_platform::layer::cursor_position(),
            zouwu_platform::layer::left_button_down(),
        ) {
            (Some(p), l) => (p, l),
            (None, l) => ((i32::MIN, i32::MIN), l),
        };
        n += 1;
        if let Some(p) = prev {
            let d = ((pos.0 - p.0).pow(2) + (pos.1 - p.1).pow(2)) as f64;
            if d > 16.0 {
                jumps += 1;
                println!(
                    "{:>6}ms  JUMP {:?} -> {:?} (d={:.0}) down={}",
                    start.elapsed().as_millis(),
                    p,
                    pos,
                    d.sqrt(),
                    down
                );
            }
        } else {
            println!("start at {pos:?} down={down}");
        }
        prev = Some(pos);
        std::thread::sleep(Duration::from_millis(5));
    }
    println!("samples={n} jumps={jumps}");
}
