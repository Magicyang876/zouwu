//! 端到端自检：截全屏 → 找二维码 → 打印结果与耗时。
//!
//! 用法：`cargo run -p zouwu-ui --example scan_qr`
//! 屏幕上摆一张码再跑，才检验得到"整条链路"（截图 → 灰度 → 检测 → 解码）。

use std::time::Instant;

fn main() {
    let t0 = Instant::now();
    let shot = match zouwu_platform::screen::capture_virtual_screen() {
        Ok(shot) => shot,
        Err(err) => {
            eprintln!("capture failed: {err}");
            std::process::exit(1);
        }
    };
    let captured = t0.elapsed();
    println!(
        "captured {}x{} at {},{} in {:?}",
        shot.width, shot.height, shot.x, shot.y, captured
    );

    let t1 = Instant::now();
    let scan = zouwu_core::qr::scan(&zouwu_core::qr::Shot {
        width: shot.width,
        height: shot.height,
        rgba: &shot.rgba,
    });
    let scanned = t1.elapsed();

    println!(
        "scan in {scanned:?}: {} code(s), {} unreadable",
        scan.hits.len(),
        scan.unreadable
    );
    for (i, hit) in scan.hits.iter().enumerate() {
        let (x, y, w, h) = hit.rect;
        let (sx, sy) = shot.to_desktop(x, y);
        println!("  #{i} at {sx},{sy} ({w}x{h}): {:?}", hit.text);
    }
}
