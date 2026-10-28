//! 在截图里找二维码并解码。
//!
//! 这一层只认像素：调用方给一块 RGBA（整块虚拟桌面的截图），这里返回"找到
//! 几个、每个的内容和位置"。屏幕编号、坐标系换算、怎么展示，都是上层的事。
//!
//! ## 为什么整块截、不逐屏截
//!
//! 逐屏截要枚举显示器、按各自 DC 取位图，然后拼；而"虚拟桌面"的位图里
//! 多出来的只是显示器之间的空隙（黑），对扫码毫无影响，却省掉了
//! 逐屏的边界处理与拼接。代价是一张大图（双 4K ≈ 7680×2160），
//! 所以下面顺手做了 2 倍降采样。
//!
//! ## 降采样的理由与代价
//!
//! 二维码是黑白方块，屏幕上的码通常 ≥120px 宽、每模块 ≥4px；降 2 倍后
//! 每模块仍有 2px，检测器（rqrr 的 capstone 搜索）依然认得出，而处理量
//! 降到 1/4。但如果用户把码缩得极小（每模块 2px），降采样就会把码糊掉——
//! 所以**只在图像大到值得降的时候降**，并且降完再试一遍原图
//! （见 `scan` 的第二轮），避免"大屏上小码扫不出"这种玄学。

use rqrr::PreparedImage;

/// 一张截图：RGBA8，行优先，`width * height * 4` 字节。
pub struct Shot<'a> {
    pub width: usize,
    pub height: usize,
    pub rgba: &'a [u8],
}

/// 一个解出来的二维码。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// 码里的内容（通常是链接，也可能是纯文本/WiFi 配置）。
    pub text: String,
    /// 在截图坐标系里的包围盒：`(x, y, w, h)`，单位是像素。
    pub rect: (u32, u32, u32, u32),
}

/// 一次扫描的结果。
#[derive(Clone, Debug, Default)]
pub struct Scan {
    /// 解码成功的码，按位置从左到右、从上到下。
    pub hits: Vec<Hit>,
    /// 检测到但解不开的码（图糊了、或者不是标准 QR）。用来区分
    /// "屏幕上没有码"和"有码但读不出来"——这两种情况对用户的意义完全不同。
    pub unreadable: usize,
}

/// 超过这个像素数就先降采样再看（约 2.4 MP，1080p 的 1.2 倍）。
const DOWNSCALE_ABOVE: usize = 2_400_000;

/// 扫一张截图。
///
/// 大图走"先降采样、扫不出再扫原图"：正常情况省 4 倍时间，
/// 小码情况退化为全分辨率，不会漏。
pub fn scan(shot: &Shot<'_>) -> Scan {
    let pixels = shot.width * shot.height;
    if shot.rgba.len() < pixels * 4 || pixels == 0 {
        return Scan::default();
    }

    if pixels > DOWNSCALE_ABOVE {
        let half = half_size(shot);
        let small = scan_at(half.0, half.1, &half.2);
        if !small.hits.is_empty() {
            return remap(small, 2);
        }
        // 降采样后什么都没找到：可能是极小的码，回原图再试一次。
        return scan_at(shot.width, shot.height, shot.rgba);
    }

    scan_at(shot.width, shot.height, shot.rgba)
}

/// 把 RGBA 转灰度、交给 rqrr 检测并解码。
fn scan_at(width: usize, height: usize, rgba: &[u8]) -> Scan {
    // 先转灰度：rqrr 只吃灰度，而 RGBA 的拷贝省不了（它要按任意顺序访问）。
    let mut gray = Vec::with_capacity(width * height);
    for px in rgba.chunks_exact(4) {
        let (r, g, b) = (px[0] as u32, px[1] as u32, px[2] as u32);
        // 整数近似 BT.601：省掉浮点，1080p 上快一个数量级。
        gray.push(((r * 299 + g * 587 + b * 114) / 1000) as u8);
    }

    let mut image =
        PreparedImage::prepare_from_greyscale(width, height, |x, y| gray[y * width + x]);
    let grids = image.detect_grids();

    let mut scan = Scan::default();
    for grid in grids {
        // `bounds` 是四个角（左上、右上、右下、左下），取包围盒当命中区域。
        // `Point` 的字段是 `i32`（像素下标），不是浮点。
        let (mut x0, mut y0) = (i32::MAX, i32::MAX);
        let (mut x1, mut y1) = (i32::MIN, i32::MIN);
        for p in grid.bounds {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        match grid.decode() {
            Ok((_, text)) => {
                let (x, y) = (x0.max(0) as u32, y0.max(0) as u32);
                let w = (x1 - x0).max(0) as u32;
                let h = (y1 - y0).max(0) as u32;
                scan.hits.push(Hit {
                    text,
                    rect: (x, y, w, h),
                });
            }
            // 检测到了格子但解不开：码被截断、压缩失真、或者根本不是 QR。
            Err(_) => scan.unreadable += 1,
        }
    }

    // 阅读顺序：先上后下、再左右。人看屏幕是这个顺序，列表也该是。
    scan.hits.sort_by_key(|hit| (hit.rect.1 / 64, hit.rect.0));
    scan
}

/// 2 倍盒式降采样（取 2×2 平均，等价于抗锯齿后再抽点，比直接抽点抗混叠）。
fn half_size(shot: &Shot<'_>) -> (usize, usize, Vec<u8>) {
    let (w, h) = (shot.width / 2, shot.height / 2);
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u16; 4];
            for dy in 0..2 {
                for dx in 0..2 {
                    let i = ((y * 2 + dy) * shot.width + (x * 2 + dx)) * 4;
                    for (c, slot) in acc.iter_mut().enumerate() {
                        *slot += shot.rgba[i + c] as u16;
                    }
                }
            }
            for c in acc {
                out.push((c / 4) as u8);
            }
        }
    }
    (w, h, out)
}

/// 降采样图上找到的坐标要乘回去，否则命中框会落在屏幕左上角的四分之一处。
fn remap(mut scan: Scan, factor: u32) -> Scan {
    for hit in &mut scan.hits {
        let (x, y, w, h) = hit.rect;
        hit.rect = (x * factor, y * factor, w * factor, h * factor);
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用 qrcode 生成一个码，铺到指定倍率，四周留白（QR 规范要求 4 模块静区）。
    ///
    /// 留白不是可选的：没有静区，检测器找不到定位图案，测试会误报成"算法不行"。
    fn render(text: &str, scale: u32) -> (usize, usize, Vec<u8>) {
        let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
        let modules = code.width() as u32;
        let quiet = 4;
        let side = (modules + quiet * 2) * scale;
        let colors = code.to_colors();
        let mut rgba = vec![0u8; (side * side * 4) as usize];
        for y in 0..side {
            for x in 0..side {
                let mx = (x / scale) as i32 - quiet as i32;
                let my = (y / scale) as i32 - quiet as i32;
                let dark = mx >= 0
                    && my >= 0
                    && (mx as u32) < modules
                    && (my as u32) < modules
                    && colors[(my as u32 * modules + mx as u32) as usize] == qrcode::Color::Dark;
                let v = if dark { 0 } else { 255 };
                let i = ((y * side + x) * 4) as usize;
                rgba[i] = v;
                rgba[i + 1] = v;
                rgba[i + 2] = v;
                rgba[i + 3] = 255;
            }
        }
        (side as usize, side as usize, rgba)
    }

    #[test]
    fn finds_and_decodes_a_link() {
        let (w, h, rgba) = render("https://example.com/zouwu", 4);
        let scan = scan(&Shot {
            width: w,
            height: h,
            rgba: &rgba,
        });
        assert_eq!(scan.hits.len(), 1, "should find exactly one code");
        assert_eq!(scan.hits[0].text, "https://example.com/zouwu");
        assert_eq!(scan.unreadable, 0);
    }

    #[test]
    fn empty_screen_reports_nothing() {
        let rgba = vec![255u8; 400 * 300 * 4];
        let scan = scan(&Shot {
            width: 400,
            height: 300,
            rgba: &rgba,
        });
        assert!(scan.hits.is_empty());
        assert_eq!(scan.unreadable, 0);
    }

    #[test]
    fn two_codes_are_both_found_left_to_right() {
        // 一张 1000×500 的"桌面"上并排放两个码，中间隔 40px 黑。
        let (bw, bh, body) = render("https://a.example", 4);
        let (cw, ch, code) = render("https://b.example", 4);
        let (w, h) = (bw + cw + 40, bh.max(ch));
        let mut rgba = vec![20u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                rgba[i + 3] = 255;
            }
        }
        let mut blit = |src: &Vec<u8>, sw: usize, sh: usize, ox: usize| {
            for y in 0..sh {
                let dst = ((y * w) + ox) * 4;
                let n = sw * 4;
                rgba[dst..dst + n].copy_from_slice(&src[y * sw * 4..y * sw * 4 + n]);
            }
        };
        blit(&body, bw, bh, 10);
        blit(&code, cw, ch, bw + 30);

        let scan = scan(&Shot {
            width: w,
            height: h,
            rgba: &rgba,
        });
        assert_eq!(scan.hits.len(), 2, "both codes should decode");
        assert_eq!(scan.hits[0].text, "https://a.example");
        assert_eq!(scan.hits[1].text, "https://b.example");
    }

    #[test]
    fn downscaled_path_still_finds_a_normal_size_code() {
        // 造一张"4K 桌面"：中间放一个 240px 的码，其余是桌面壁纸那种噪点。
        let (cw, ch, code) = render("https://big.screen", 6);
        let (w, h) = (3840, 2160);
        assert!(
            w * h > DOWNSCALE_ABOVE,
            "test must exercise the downscale path"
        );
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                // 渐变底：纯色会让"降采样后还能不能认"这个测试变得过于乐观。
                let v = ((x * 7 + y * 3) % 200) as u8;
                rgba[i] = v;
                rgba[i + 1] = v;
                rgba[i + 2] = v;
                rgba[i + 3] = 255;
            }
        }
        let (ox, oy) = (900, 700);
        for y in 0..ch {
            let dst = (((oy + y) * w) + ox) * 4;
            let n = cw * 4;
            rgba[dst..dst + n].copy_from_slice(&code[y * cw * 4..y * cw * 4 + n]);
        }

        let scan = scan(&Shot {
            width: w,
            height: h,
            rgba: &rgba,
        });
        assert_eq!(scan.hits.len(), 1, "code on a 4K desktop should be found");
        // 降采样路径的坐标要乘回去：命中框必须落回 (900, 700) 附近，而不是它的四分之一处。
        let (x, y, ..) = scan.hits[0].rect;
        assert!(
            (x as i64 - ox as i64).abs() < 40 && (y as i64 - oy as i64).abs() < 40,
            "hit should map back to its real position, got {x},{y}"
        );
    }

    #[test]
    fn truncated_pixels_do_not_panic() {
        // 缓冲比声明的尺寸小（截图失败时会这样）：不 panic，老实返回空结果。
        let rgba = vec![0u8; 16];
        let scan = scan(&Shot {
            width: 100,
            height: 100,
            rgba: &rgba,
        });
        assert!(scan.hits.is_empty());
    }
}
