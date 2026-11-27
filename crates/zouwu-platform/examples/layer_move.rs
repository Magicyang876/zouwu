//! 跨屏拖动的端到端检查：不碰鼠标，直接驱动 `PetSurface` 的位置，
//! 看它能不能从第一块屏平滑地挪到第二块屏（中间要重建表面 + 重算 margin）。
//!
//! 跑法：`cargo run -p zouwu-platform --example layer_move`
//! 观察日志里有没有 `Protocol error`、`pos` 是否跟着走。
//!
//! 只在 Linux 上有意义（`PetSurface` 是 layer-shell 的东西），别的平台上是个空壳——
//! 示例编不过不该挡住 Windows 上的 `cargo test`。

#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
fn main() -> Result<(), String> {
    let size = 340u32;
    let surface = Arc::new(zouwu_platform::layer::PetSurface::create(
        size,
        size,
        (40, 40),
    )?);
    surface.set_waker(Arc::new(|| {}));
    println!(
        "surface {} created, scale {}",
        surface.id(),
        surface.scale()
    );

    // `monitors()` 读的是活着的表面记下的输出表，得等表面建好之后才有值。
    std::thread::sleep(Duration::from_millis(500));
    let screen: Vec<(i32, i32, i32, i32, f64)> = zouwu_platform::layer::monitors();
    println!("monitors (physical x,y,w,h,scale): {screen:?}");
    let logical: Vec<(i32, i32, i32, i32)> = screen
        .iter()
        .map(|(x, y, w, h, s)| {
            let s = (*s).max(1.0) as i32;
            (x / s, y / s, w / s, h / s)
        })
        .collect();
    println!("monitors (logical): {logical:?}");

    // 一块纯色方块，四角留透明：肉眼看得见，input region 非空。
    let paint = |surface: &zouwu_platform::layer::PetSurface| -> Result<(), String> {
        let (w, h) = surface.size();
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for i in 0..(w * h) as usize {
            let x = i % w as usize;
            let y = i / w as usize;
            let inside = x > w as usize / 8
                && x < w as usize - w as usize / 8
                && y > h as usize / 8
                && y < h as usize - h as usize / 8;
            if inside {
                rgba[i * 4] = 255;
                rgba[i * 4 + 1] = 120;
                rgba[i * 4 + 2] = 40;
                rgba[i * 4 + 3] = 255;
            }
        }
        surface.present(&rgba)
    };
    paint(&surface)?;

    // 依次走过：第一块屏左中 → 右边缘 → 跨过接缝 → 第二块屏中间 → 第二块屏右边。
    let path: Vec<(i32, i32)> = {
        let (_, _, w0, h0) = logical[0];
        // 关键三段：屏内 → **骑在缝上**（位置允许越界，靠翻锚点表达）→ 落在第二块屏。
        let mut out = vec![
            (40, 40),
            (w0 - 400, h0 / 2),
            (w0 - 170, h0 / 2),
            (w0 - 907, h0 / 2),
        ];
        if logical.len() > 1 {
            let (x1, _, w1, h1) = logical[1];
            out.push((x1 + 40, h1 / 2));
            out.push((x1 + w1 / 2, h1 / 2));
            out.push((x1 + w1 - 400, h1 / 2));
        }
        out
    };
    for (i, (x, y)) in path.iter().enumerate() {
        surface.set_position(*x, *y);
        std::thread::sleep(Duration::from_millis(700));
        // 重建之后尺寸/缩放会变，重画一次才看得到。
        paint(&surface)?;
        std::thread::sleep(Duration::from_millis(200));
        println!(
            "step {i}: asked ({x},{y}) -> pos {:?}, scale {}, closed {}",
            surface.pos(),
            surface.scale(),
            surface.closed()
        );
    }
    // `ZOUWU_HOLD=6000` 会把表面停在接缝上并挂住 6s，方便截图看"骑在缝上"
    // 有没有被合成器裁掉（跨屏拖动就靠这个能力）。
    if let Some(ms) = std::env::var("ZOUWU_HOLD")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
    {
        let (_, _, w0, h0) = logical[0];
        let (x, y) = (w0 - 170, h0 / 2);
        surface.set_position(x, y);
        std::thread::sleep(Duration::from_millis(300));
        paint(&surface)?;
        println!("holding at the seam ({x},{y}) for {ms} ms");
        std::thread::sleep(Duration::from_millis(ms));
    }

    // 跨缝冲刺：8ms 一步、每步 8 逻辑像素横穿接缝。好的行为是**只在越缝时
    // 重建一次表面**（每次重建 = 一次 configure 往返）；重建多了就是拖动时那种
    // "一卡一卡"。
    if logical.len() > 1 {
        let (_, _, w0, h0) = logical[0];
        let (y, mut x, mut steps) = (h0 / 2, w0 - 400, 0);
        // 先站到起点、等它落定（从上一块屏跳回来本身也算一次重建）。
        surface.set_position(x, y);
        std::thread::sleep(Duration::from_millis(300));
        paint(&surface)?;
        let before = surface.rebuilds();
        while x < w0 + 800 {
            surface.set_position(x, y);
            std::thread::sleep(Duration::from_millis(8));
            x += 8;
            steps += 1;
        }
        let rebuilt = surface.rebuilds() - before;
        println!(
            "sprint: {steps} moves across the seam -> {rebuilt} rebuild(s), closed {}",
            surface.closed()
        );
        if rebuilt != 1 {
            return Err(format!(
                "crossing the seam should rebuild exactly once, got {rebuilt}"
            ));
        }
    }

    std::thread::sleep(Duration::from_millis(400));
    let err = surface.closed();
    drop(surface);
    if err {
        return Err("surface was closed during the walk (compositor killed it)".into());
    }
    println!(
        "ok: walked across {} output(s) without the surface dying",
        logical.len()
    );
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("layer_move: 只有 Linux（layer-shell）上才有意义");
}
