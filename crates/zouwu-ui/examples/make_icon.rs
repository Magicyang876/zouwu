//! 生成 Windows 图标 `assets/zouwu.ico`（默认应用图标：暖色方底 + 兽首 + 多尺寸）。
//!
//! ```bash
//! cargo run --release -p zouwu-ui --example make_icon
//! ```
//!
//! 为什么自己写 ICO 容器：换来的是"图标和桌面上的小兽永远是同一张脸"。
//! 用现成工具导出的话，改了小兽还得记得重导一次图标 —— 这份记忆迟早会失效。
//!
//! ## ICO 格式里两个必须知道的点
//!
//! 1. **尺寸 ≤ 128 用 DIB（BMP），256 用 PNG**。256² 的 DIB 是 256 KB，
//!    塞进 exe 白占体积；而 PNG-in-ICO 自 Vista 起是官方支持。
//! 2. **DIB 是"倒着"存的**（bottom-up），并且**高度写成两倍**
//!    （XOR 位图 + AND 掩码各占一份高度）。AND 掩码在 32bpp 下已经无用，
//!    但长度必须给足，否则资源管理器会读越界。

use std::path::PathBuf;

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap};

/// 图标里要包含的尺寸。16/20/24 是任务栏和标题栏，32/48 是桌面和资源管理器，
/// 256 是"大图标"视图。
const SIZES: &[u32] = &[16, 20, 24, 32, 48, 64, 128, 256];

fn out_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("zouwu.ico");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create assets dir");
    path
}

/// 一枚应用图标：暖色圆角方块 + 深色兽首（两只尖耳、两只眼、一个小鼻子）。
///
/// 和主窗口的 `paint::draw_buddy` 是**两套代码**（那边要动画/投影/表情，这边要在
/// 16px 下还能认出来），但配色取自同一个 `Theme`，所以看起来是一家人。
///
/// 三条硬约束（吃过亏）：
/// 1. **方形底 + 圆角**：纯透明底的头像放进任务栏/开始菜单会和背景糊成一片；
///    一块暖色底把对比度钉死，任何壁纸/主题下都认得出。
/// 2. **耳朵要压在圆头上**：先画耳、再把头盖上去，耳根就不会出现接缝。
/// 3. **每个"细节"都得有下限尺寸**：16px 里眼睛只剩 0.9px，再小就等于没有。
fn draw(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let s = size as f32;

    let warm = Color::from_rgba8(0xE8, 0x7A, 0x4A, 0xFF);
    let ink = Color::from_rgba8(0x2A, 0x1A, 0x14, 0xFF);
    let cream = Color::from_rgba8(0xF6, 0xE9, 0xDC, 0xFF);

    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };

    // 底：圆角方块，留 5% 边（贴边的圆角在 16px 里会被切成方角）。
    let pad = s * 0.05;
    let (bx, bw) = (pad, s - pad * 2.0);
    let br = bw * 0.26;
    let mut pb = PathBuilder::new();
    pb.move_to(bx + br, bx);
    pb.line_to(bx + bw - br, bx);
    pb.quad_to(bx + bw, bx, bx + bw, bx + br);
    pb.line_to(bx + bw, bx + bw - br);
    pb.quad_to(bx + bw, bx + bw, bx + bw - br, bx + bw);
    pb.line_to(bx + br, bx + bw);
    pb.quad_to(bx, bx + bw, bx, bx + bw - br);
    pb.line_to(bx, bx + br);
    pb.quad_to(bx, bx, bx + br, bx);
    pb.close();
    if let Some(path) = pb.finish() {
        paint.set_color(warm);
        pm.fill_path(&path, &paint, FillRule::Winding, Default::default(), None);
    }

    let (cx, cy) = (s * 0.5, s * 0.52);
    let r = s * 0.30;

    // 耳朵：耳根压在头顶两侧，头圆盖上去正好把接缝糊住；耳尖向外上方挑。
    paint.set_color(ink);
    for side in [-1.0f32, 1.0] {
        let mut pb = PathBuilder::new();
        pb.move_to(cx + side * r * 0.30, cy - r * 0.78);
        pb.line_to(cx + side * r * 0.88, cy - r * 0.60);
        pb.line_to(cx + side * r * 0.66, cy - r * 1.12);
        pb.close();
        if let Some(path) = pb.finish() {
            pm.fill_path(&path, &paint, FillRule::Winding, Default::default(), None);
        }
    }

    // 头：略扁的圆（正圆看着像"球"，扁一点才像"脸"）。
    let mut pb = PathBuilder::new();
    let steps = 64;
    for i in 0..steps {
        let t = i as f32 / steps as f32 * std::f32::consts::TAU;
        let (x, y) = (cx + r * t.cos(), cy + r * 0.94 * t.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &paint, FillRule::Winding, Default::default(), None);
    }

    // 眼睛：16px 时半径只剩 0.9px，给下限保证"有瞳孔"。
    paint.set_color(cream);
    let eye_r = (r * 0.15).max(0.9);
    for side in [-1.0f32, 1.0] {
        if let Some(path) = PathBuilder::from_circle(cx + side * r * 0.38, cy - r * 0.07, eye_r) {
            pm.fill_path(&path, &paint, FillRule::Winding, Default::default(), None);
        }
    }

    // 鼻子：比眼睛小一号，16px 下基本糊掉，但不影响识别。
    let mut pb = PathBuilder::new();
    let (nx, ny) = (cx, cy + r * 0.40);
    let (nrx, nry) = (r * 0.16, r * 0.12);
    for i in 0..steps {
        let t = i as f32 / steps as f32 * std::f32::consts::TAU;
        let (x, y) = (nx + nrx * t.cos(), ny + nry * t.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &paint, FillRule::Winding, Default::default(), None);
    }
    pm
}

/// 预乘 → 直通（ICO 的 DIB 与 PNG 都用非预乘存储）。
fn straight(pm: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pm.data().len());
    for px in pm.data().chunks_exact(4) {
        let a = px[3];
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            out.extend_from_slice(&[un(px[0]), un(px[1]), un(px[2]), a]);
        }
    }
    out
}

/// 一张 DIB（BITMAPINFOHEADER + BGRA 倒序 + AND 掩码）。
fn dib(pm: &Pixmap) -> Vec<u8> {
    let (w, h) = (pm.width(), pm.height());
    let rgba = straight(pm);
    let mut out = Vec::with_capacity(40 + (w * h * 4) as usize);

    // BITMAPINFOHEADER：高写两倍（XOR + AND），planes=1，bitCount=32，BI_RGB=0
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&((h * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(w * h * 4).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]); // 打印机相关字段，全 0

    // 像素：bottom-up + BGRA
    for y in (0..h).rev() {
        for x in 0..w {
            let at = ((y * w + x) * 4) as usize;
            out.extend_from_slice(&[rgba[at + 2], rgba[at + 1], rgba[at], rgba[at + 3]]);
        }
    }
    // AND 掩码：每行按 4 字节对齐，32bpp 下内容无所谓，但长度必须够。
    let mask_row = w.div_ceil(32) * 4;
    let mask_at = out.len();
    out.resize(mask_at + (mask_row * h) as usize, 0);
    out
}

fn png(pm: &Pixmap) -> Vec<u8> {
    // tiny-skia 的 `encode_png` 直接吃预乘数据并转回直通，不用我们插手。
    pm.encode_png().expect("encode png")
}

fn main() {
    let path = out_path();
    let mut entries: Vec<(u32, Vec<u8>)> = Vec::new();
    for &size in SIZES {
        let pm = draw(size);
        let visible = pm.data().chunks_exact(4).filter(|px| px[3] > 8).count();
        assert!(
            visible > (size * size) as usize / 6,
            "{size}px icon looks empty ({visible} visible px)"
        );
        // 256 用 PNG，其余用 DIB（见文件头注释）
        let bytes = if size >= 256 { png(&pm) } else { dib(&pm) };
        entries.push((size, bytes));
    }

    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type = icon
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());

    let mut offset = 6 + entries.len() * 16;
    for (size, bytes) in &entries {
        out.push(if *size >= 256 { 0 } else { *size as u8 }); // 0 表示 256
        out.push(if *size >= 256 { 0 } else { *size as u8 });
        out.push(0); // 调色板数
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += bytes.len();
    }
    for (_, bytes) in &entries {
        out.extend_from_slice(bytes);
    }

    std::fs::write(&path, &out).expect("write ico");
    println!(
        "wrote {} ({} bytes, {} sizes)",
        path.display(),
        out.len(),
        entries.len()
    );
}
