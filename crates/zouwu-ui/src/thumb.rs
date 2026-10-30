//! 皮肤缩略图：把一套皮肤画成一张小图，给设置面板的皮肤页用。
//!
//! 复用主渲染管线（[`crate::paint::render`]）而不是另写一套"画个方块"：
//! 缩略图必须**和桌面上那只长得一样**，否则用户是照着假图在挑皮肤。
//! 代价是每套皮肤要渲染一次，好处是永远不会出现"预览与实际不一致"。
//!
//! 只在打开面板 / 皮肤列表变化时渲染一次，之后缓存复用；面板静止不重画。

use tiny_skia::Pixmap;
use zouwu_core::geom::Point;
use zouwu_core::theme::Theme;

use crate::paint::{self, Scene};
use crate::skin::Skin;

/// 一张**直通（非预乘）** RGBA 缩略图，尺寸为 `w × h`。
///
/// 为什么要转成直通 alpha：tiny-skia 内部是预乘的，而合成到窗口缓冲区时要按
/// 直通 alpha 做 SourceOver，两套约定混用会让边缘出现一圈暗边（黑边）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumb {
    pub w: u32,
    pub h: u32,
    /// `w * h * 4` 字节，RGBA8。
    pub pixels: Vec<u8>,
}

impl Thumb {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.w + x) * 4) as usize;
        [
            self.pixels[at],
            self.pixels[at + 1],
            self.pixels[at + 2],
            self.pixels[at + 3],
        ]
    }

    /// 可见像素数（alpha > 8）。示例/测试用来确认"图没塌"。
    pub fn visible_pixels(&self) -> usize {
        self.pixels.chunks_exact(4).filter(|px| px[3] > 8).count()
    }
}

/// 渲染一套皮肤的缩略图。`skin = None` → 代码绘制的那只。
pub fn render(skin: Option<&Skin>, theme: &Theme, size: u32) -> Result<Thumb, String> {
    let size = size.clamp(16, 256);
    let mut pixmap = Pixmap::new(size, size).ok_or_else(|| "cannot alloc thumb".to_string())?;
    let s = size as f32;
    let scene = Scene {
        width: s,
        height: s,
        scale: 1.0,
        // 略微偏下：投影在脚下，视觉重心才居中（和主窗口同一套构图）。
        buddy_center: Point::new(s * 0.5, s * 0.54),
        buddy_radius: s * 0.32,
        buddy_scale: 1.0,
        buddy_alpha: 1.0,
        // 缩略图要"睁眼微笑"的那一帧：闭眼的小图容易被读成"缺一块"。
        blink: 0.0,
        breath: 0.0,
        theme,
        bubbles: &[],
        beams: &[],
        skin,
    };
    paint::render(&scene, &mut pixmap);

    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for px in pixmap.data().chunks_exact(4) {
        let a = px[3];
        if a == 0 {
            pixels.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            // 预乘 → 直通：c = c_pre / a
            let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            pixels.extend_from_slice(&[un(px[0]), un(px[1]), un(px[2]), a]);
        }
    }
    Ok(Thumb {
        w: size,
        h: size,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zouwu_core::theme::Theme;

    #[test]
    fn procedural_thumb_has_visible_pixels_and_fits() {
        let theme = Theme::fallback();
        let thumb = render(None, &theme, 64).expect("render");
        assert_eq!(thumb.pixels.len(), 64 * 64 * 4);
        assert!(
            thumb.visible_pixels() > 200,
            "thumb looks empty: {}",
            thumb.visible_pixels()
        );
    }

    #[test]
    fn corners_are_transparent_but_center_is_not() {
        let theme = Theme::fallback();
        let thumb = render(None, &theme, 64).expect("render");
        // 圆形小兽：四角必然透明，中心必然不透明 —— 顺便验证没把整个画布填满。
        assert_eq!(thumb.pixel(0, 0)[3], 0);
        assert_eq!(thumb.pixel(63, 63)[3], 0);
        assert!(thumb.pixel(32, 34)[3] > 200);
    }

    #[test]
    fn size_is_clamped() {
        let theme = Theme::fallback();
        let tiny = render(None, &theme, 1).expect("render");
        assert_eq!(tiny.w, 16);
        let huge = render(None, &theme, 1000).expect("render");
        assert_eq!(huge.w, 256);
    }
}
