//! 窗口图标：任务栏 / Alt-Tab / 标题栏都吃它。
//!
//! 用仓库里的 `assets/zouwu-icon.png`（由 `examples/make_icon.rs` 一并生成），
//! 而不是现场画小兽——窗口图标要的是"这是 zouwu 这个应用"，跟 exe 的图标是同一张脸。
//!
//! 为什么直接 decode PNG 而不是解 `.ico`：ICO 里的 128 尺寸是 DIB，自己解要处理
//! bottom-up、两倍高度、AND 掩码；PNG 一行 `decode_png` 就够，而这张 PNG 和 ICO
//! 是同一个 `draw()` 出来的，改配色两张一起改。

use tiny_skia::Pixmap;
use winit::window::Icon;

/// 图标 PNG 的内嵌副本（128×128，够 32/48px 的任务栏与 Alt-Tab 用）。
const PNG: &[u8] = include_bytes!("../../../assets/zouwu-icon.png");

/// 应用图标。解码失败就返回 `None`——图标不该拖垮窗口创建。
pub fn app_icon() -> Option<Icon> {
    let (rgba, width, height) = app_icon_rgba()?;
    Icon::from_rgba(rgba, width, height).ok()
}

/// 直通 alpha 的像素 + 尺寸（`Icon` 拿不到自己的尺寸，测试与调试都靠这个）。
fn app_icon_rgba() -> Option<(Vec<u8>, u32, u32)> {
    let pixmap = Pixmap::decode_png(PNG).ok()?;
    let rgba = straight_rgba(pixmap.data());
    Some((rgba, pixmap.width(), pixmap.height()))
}

/// tiny-skia 的像素是**预乘**的，`Icon` 要的是**直通** alpha，所以要除回去。
fn straight_rgba(premultiplied: &[u8]) -> Vec<u8> {
    let mut out = premultiplied.to_vec();
    for px in out.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 0 || a == 255 {
            continue;
        }
        // 加 a/2 再除，等同于四舍五入；不这么写，半透明边缘会整体偏暗一档。
        for c in &mut px[..3] {
            *c = (((*c as u32) * 255 + a / 2) / a).min(255) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_icon_decodes_to_a_square() {
        let (rgba, w, h) = app_icon_rgba().expect("assets/zouwu-icon.png must decode");
        assert_eq!(w, h, "窗口图标得是正方形");
        assert!(w >= 32, "太小了，任务栏上看不清");
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        // 底色的四角是透明的（圆角方底），中间是不透明的暖色
        assert_eq!(rgba[3], 0, "左上角应当透明");
        let mid = ((h / 2 * w + w / 2) * 4) as usize;
        assert_eq!(rgba[mid + 3], 255, "中心应当不透明");
    }

    #[test]
    fn un_premultiplying_restores_straight_alpha() {
        // 预乘 (128,64,32,128) 对应的直通色是 (255,128,64)
        let out = straight_rgba(&[128, 64, 32, 128, 10, 20, 30, 255, 0, 0, 0, 0]);
        assert_eq!(&out[0..4], &[255, 128, 64, 128]);
        // 全不透明的像素原样保留（否则会把图标主体的颜色改掉）
        assert_eq!(&out[4..8], &[10, 20, 30, 255]);
        // 全透明的像素保持 0，别除出 NaN
        assert_eq!(&out[8..12], &[0, 0, 0, 0]);
    }

    #[test]
    fn the_icon_png_matches_the_ico_script() {
        // 防止有人只更新了 .ico：PNG 必须存在且是正方形（生成脚本一起写两张）
        let pm = Pixmap::decode_png(PNG).expect("decode");
        assert_eq!(pm.width(), pm.height());
    }
}
