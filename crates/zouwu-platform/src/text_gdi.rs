//! Windows GDI 文本栅格化。
//!
//! ## 为什么不用字体库
//!
//! 第一版用 `fontdue` 自己解析字体。功能没问题，但**内存不可接受**：实测
//! `msyh.ttc`（19MB）光解析就要常驻 **175MB**，六族候选字面一起加载把设置
//! 面板的 RSS 顶到 **868MB**——对一个桌面挂件来说是致命的。
//!
//! GDI 的代价结构完全不同：字体由系统共享，我们只付"一次 rig 的位图"。
//! 实测常驻增量 ≈ 0MB，而且中文、假名、emoji 的系统 fallback 全是白送的，
//! 不用自己维护候选字体列表。
//!
//! ## 做法
//!
//! 把字符串画进一块 32bpp DIB（黑底白字），**红色通道就是覆盖率**，
//! 再拿这个覆盖率和目标颜色相乘混进目标 RGBA 缓冲。这样：不做遮罩图、
//! 不需要 premultiply 细节，只要一次逐像素乘加。

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::ffi::c_void;

type Hdc = *mut c_void;
type Hbitmap = *mut c_void;
type Hgdiobj = *mut c_void;
type Hfont = *mut c_void;
type Bool = i32;

const DIB_RGB_COLORS: u32 = 0;
const BI_RGB: u32 = 0;
const TRANSPARENT: i32 = 1;
const FW_NORMAL: i32 = 400;
const FW_BOLD: i32 = 700;
const DEFAULT_CHARSET: u32 = 1;
const OUT_TT_PRECIS: u32 = 4;
const CLIP_DEFAULT_PRECIS: u32 = 0;
const ANTIALIASED_QUALITY: u32 = 4;
const DEFAULT_PITCH: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Size {
    cx: i32,
    cy: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RgbQuad {
    blue: u8,
    green: u8,
    red: u8,
    reserved: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [RgbQuad; 1],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TextMetricW {
    height: i32,
    ascent: i32,
    descent: i32,
    internal_leading: i32,
    external_leading: i32,
    ave_char_width: i32,
    max_char_width: i32,
    weight: i32,
    overhang: i32,
    digitized_aspect_x: i32,
    digitized_aspect_y: i32,
    italic: u8,
    underlined: u8,
    struck_out: u8,
    pitch_and_family: u8,
    char_set: u8,
    flags: u8,
    reserved: u8,
    first_char: u16,
    last_char: u16,
    default_char: u16,
    break_char: u16,
    italic_angle: i32,
    pitch_and_family2: u8,
    reserved2: [u8; 3],
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    fn CreateDIBSection(
        hdc: Hdc,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut c_void,
        section: *mut c_void,
        offset: u32,
    ) -> Hbitmap;
    fn SelectObject(hdc: Hdc, obj: Hgdiobj) -> Hgdiobj;
    fn DeleteObject(obj: Hgdiobj) -> Bool;
    fn DeleteDC(hdc: Hdc) -> Bool;
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike_out: u32,
        char_set: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face: *const u16,
    ) -> Hfont;
    fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
    fn SetTextColor(hdc: Hdc, color: u32) -> u32;
    fn TextOutW(hdc: Hdc, x: i32, y: i32, string: *const u16, count: i32) -> Bool;
    fn GetTextExtentPoint32W(hdc: Hdc, string: *const u16, count: i32, size: *mut Size) -> Bool;
    fn GetTextMetricsW(hdc: Hdc, metrics: *mut TextMetricW) -> Bool;
    /// 把字体文件注册给本进程（`FR_PRIVATE`）：不写注册表、不改系统设置，
    /// 进程退出即失效。用户"导入一个字体包"就是往这儿喂一个文件。
    fn AddFontResourceExW(name: *const u16, flags: u32, reserved: *mut c_void) -> i32;
}

/// `AddFontResourceExW` 的标志：只对本进程可见，且不污染系统字体列表。
const FR_PRIVATE: u32 = 0x10;

#[link(name = "user32")]
extern "system" {
    fn GetDC(hwnd: *mut c_void) -> Hdc;
    fn ReleaseDC(hwnd: *mut c_void, hdc: Hdc) -> i32;
}

/// 一块可复用的离屏 DIB：每次绘制前只清空用到的那一块。
struct Scratch {
    dc: Hdc,
    bitmap: Hbitmap,
    old: Hgdiobj,
    bits: *mut u32,
    width: i32,
    height: i32,
}

// GDI 句柄本身不绑定线程（只有 GetDC(hwnd) 拿到的 HDC 才和窗口线程有关，
// 而我们的 DC 是 CreateCompatibleDC 出来的内存 DC）。所有访问都发生在
// `with_gdi` 的全局锁里，所以跨线程移动是安全的。
unsafe impl Send for Scratch {}

impl Scratch {
    fn new() -> Option<Self> {
        let screen = unsafe { GetDC(std::ptr::null_mut()) };
        if screen.is_null() {
            return None;
        }
        let dc = unsafe { CreateCompatibleDC(screen) };
        unsafe { ReleaseDC(std::ptr::null_mut(), screen) };
        if dc.is_null() {
            return None;
        }
        Some(Self {
            dc,
            bitmap: std::ptr::null_mut(),
            old: std::ptr::null_mut(),
            bits: std::ptr::null_mut(),
            width: 0,
            height: 0,
        })
    }

    /// 保证至少有 `w × h`，并把这块区域清成黑色。
    fn prepare(&mut self, w: i32, h: i32) -> bool {
        let w = w.max(1);
        let h = h.max(1);
        if w > self.width || h > self.height {
            let new_w = w.max(self.width).max(64);
            let new_h = h.max(self.height).max(64);
            let mut info = BitmapInfo::default();
            info.header.size = std::mem::size_of::<BitmapInfoHeader>() as u32;
            info.header.width = new_w;
            // 负高度 = 自上而下，行序和我们的缓冲一致，省一次翻转。
            info.header.height = -new_h;
            info.header.planes = 1;
            info.header.bit_count = 32;
            info.header.compression = BI_RGB;
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = unsafe {
                CreateDIBSection(
                    self.dc,
                    &info,
                    DIB_RGB_COLORS,
                    &mut bits,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if bitmap.is_null() || bits.is_null() {
                return false;
            }
            if !self.bitmap.is_null() {
                unsafe { SelectObject(self.dc, self.old) };
                unsafe { DeleteObject(self.bitmap) };
            }
            self.old = unsafe { SelectObject(self.dc, bitmap) };
            self.bitmap = bitmap;
            self.bits = bits as *mut u32;
            self.width = new_w;
            self.height = new_h;
        }
        let count = (self.width * h) as usize;
        unsafe { std::ptr::write_bytes(self.bits, 0, count) };
        true
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        unsafe {
            if !self.bitmap.is_null() {
                SelectObject(self.dc, self.old);
                DeleteObject(self.bitmap);
            }
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
        }
    }
}

/// 一次文本栅格化的度量。
pub struct Metrics {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
}

/// 没选自定义字体时用的家族。选它是因为它自带中文与西文回退，
/// 且从 Win7 起就随系统存在 —— 我们不需要随包带字体文件。
pub const DEFAULT_FAMILY: &str = "Microsoft YaHei UI";

/// 注册一个字体文件（进程私有）。返回系统是否接受了它。
///
/// 注意：注册成功**不代表**字体家族是我们猜的那个名字，所以调用方应该
/// 用 [`zouwu_platform::fontfile`] 从文件里读家族名，而不是靠枚举差集。
pub fn add_font_file(path: &std::path::Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let added = unsafe { AddFontResourceExW(wide.as_ptr(), FR_PRIVATE, std::ptr::null_mut()) };
    if added > 0 {
        log::info!("font registered: {} ({added} face(s))", path.display());
        true
    } else {
        log::warn!("font rejected by the system: {}", path.display());
        false
    }
}

/// 当前使用的字体家族（`None` = 系统默认）。
pub fn family() -> Option<String> {
    with_gdi(|gdi| gdi.family.clone()).flatten()
}

/// 切换字体家族。返回是否真的变了。
///
/// 换家族必须清掉字体与宽度缓存 —— 缓存键里只有 (px, bold)，
/// 家族换了而键不变的话，我们会拿旧字体的度量去排版新字体。
pub fn set_family(family: Option<String>) -> bool {
    let target = family.filter(|f| !f.trim().is_empty());
    let changed = with_gdi(|gdi| {
        if gdi.family == target {
            return false;
        }
        gdi.family = target.clone();
        gdi.fonts.clear();
        gdi.widths.clear();
        true
    })
    .unwrap_or(false);
    if changed {
        match &target {
            Some(f) => log::info!("font family -> {f}"),
            None => log::info!("font family -> system default ({DEFAULT_FAMILY})"),
        }
    }
    changed
}

/// 单行文本的浮点像素宽度（已含 GDI 的进位取整）。
pub fn measure(text: &str, px: f32, bold: bool) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    with_gdi(|gdi| gdi.text_width(text, px, bold))
        .unwrap_or_else(|| text.chars().count() as f32 * px * 0.55)
}

/// 一行文本的纵向度量。
pub fn metrics(px: f32, bold: bool) -> Metrics {
    with_gdi(|gdi| gdi.vmetrics(px, bold))
        .map(|(ascent, descent, line_height)| Metrics {
            width: 0.0,
            ascent,
            descent,
            line_height,
        })
        .unwrap_or(Metrics {
            width: 0.0,
            ascent: px * 0.8,
            descent: px * 0.2,
            line_height: px * 1.2,
        })
}

/// 把文本合成到目标 pixmap（预乘 RGBA，行优先）。
///
/// `baseline` 是基线的 y 坐标；`clip` 是物理像素的裁剪矩形（x0,y0,x1,y1）。
#[allow(clippy::too_many_arguments)]
pub fn draw(
    pixmap: &[u8],
    pm_width: usize,
    pm_height: usize,
    text: &str,
    x: f32,
    baseline: f32,
    px: f32,
    color: [u8; 4],
    bold: bool,
    clip: Option<(f32, f32, f32, f32)>,
) -> Option<Vec<u8>> {
    if text.is_empty() {
        return None;
    }
    with_gdi(|gdi| {
        draw_with(
            gdi, pixmap, pm_width, pm_height, text, x, baseline, px, color, bold, clip,
        )
    })
    .flatten()
}

#[allow(clippy::too_many_arguments)]
fn draw_with(
    gdi: &mut Gdi,
    pixmap: &[u8],
    pm_width: usize,
    pm_height: usize,
    text: &str,
    x: f32,
    baseline: f32,
    px: f32,
    color: [u8; 4],
    bold: bool,
    clip: Option<(f32, f32, f32, f32)>,
) -> Option<Vec<u8>> {
    // 关键：**自己把字体选进 DC**。
    //
    // 原来这里不选字体，靠"上一步 `text_width` 刚好替我们选过同一个字号"——
    // 那是个隐式假设，而 `text_width` 有宽度缓存，命中缓存就直接返回、不碰 DC。
    // 于是一旦相邻两次文字的字号不同（12pt 的页脚 → 11pt 的按键提示），
    // 后一次就用前一次的字号画，而且擦洗缓冲区按**当前**字号的宽度分配：
    // 画得比缓冲区宽 → 右边一两个字被切掉，看起来就是"文字遮挡/被裁"。
    let (font, ascent, _, line_height) = gdi.font(px, bold)?;
    unsafe { SelectObject(gdi.scratch.dc, font) };
    // 留几像素余量：CJK 全角字距与字形外扩在取整后可能略微超出行宽。
    let width = gdi.text_width(text, px, bold).ceil() as i32 + 6;
    let height = line_height.ceil() as i32 + 4;
    let ascent = ascent.ceil() as i32;
    if !gdi.scratch.prepare(width, height.max(1)) {
        return None;
    }

    // 黑底白字：红色通道就是覆盖率。
    let utf16: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        SetBkMode(gdi.scratch.dc, TRANSPARENT);
        SetTextColor(gdi.scratch.dc, 0x00FF_FFFF);
        TextOutW(gdi.scratch.dc, 1, 1, utf16.as_ptr(), utf16.len() as i32);
    }

    let mut out = pixmap.to_vec();
    let origin_x = x.round() as i32 - 1;
    let origin_y = baseline.round() as i32 - ascent - 1;
    let (cx0, cy0, cx1, cy1) = clip.unwrap_or((0.0, 0.0, pm_width as f32, pm_height as f32));
    let px_bitmap = gdi.scratch.bits;

    for row in 0..height {
        let dst_y = origin_y + row;
        if dst_y < 0 || dst_y >= pm_height as i32 {
            continue;
        }
        if (dst_y as f32) < cy0 || (dst_y as f32) >= cy1 {
            continue;
        }
        for col in 0..width {
            let dst_x = origin_x + col;
            if dst_x < 0 || dst_x >= pm_width as i32 {
                continue;
            }
            if (dst_x as f32) < cx0 || (dst_x as f32) >= cx1 {
                continue;
            }
            let src = unsafe { *px_bitmap.add((row * gdi.scratch.width + col) as usize) };
            let coverage = src & 0xFF; // 红通道
            if coverage == 0 {
                continue;
            }
            let idx = ((dst_y as usize) * pm_width + dst_x as usize) * 4;
            let alpha = color[3] as u32 * coverage / 255;
            // 目标与源都按预乘处理：src 已乘过自身 alpha，再按覆盖率叠加。
            for ch in 0..3 {
                let s = color[ch] as u32 * coverage / 255;
                let d = out[idx + ch] as u32;
                out[idx + ch] = (s + d * (255 - alpha) / 255).min(255) as u8;
            }
            let d = out[idx + 3] as u32;
            out[idx + 3] = (alpha + d * (255 - alpha) / 255).min(255) as u8;
        }
    }
    Some(out)
}

/// 全局唯一的 GDI 会话。GDI 句柄是线程亲和的，所以统一在这把锁里用。
struct Gdi {
    scratch: Scratch,
    /// (px×4, bold) → HFONT，连同它的纵向度量。
    fonts: HashMap<(u16, bool), (Hfont, f32, f32, f32)>,
    /// 文本宽度缓存：面板一帧要量几十次同样的字符串。
    widths: HashMap<(String, u16, bool), f32>,
    /// 当前家族（`None` = `DEFAULT_FAMILY`）。
    family: Option<String>,
}

// 同上：HFONT 也是进程级 GDI 对象，访问全在锁内。
unsafe impl Send for Gdi {}

fn with_gdi<R>(f: impl FnOnce(&mut Gdi) -> R) -> Option<R> {
    static GDI: std::sync::OnceLock<std::sync::Mutex<Option<Gdi>>> = std::sync::OnceLock::new();
    let slot = GDI.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = slot.lock().ok()?;
    if guard.is_none() {
        *guard = Gdi::create();
    }
    guard.as_mut().map(f)
}

impl Gdi {
    fn create() -> Option<Gdi> {
        let scratch = Scratch::new()?;
        Some(Gdi {
            scratch,
            fonts: HashMap::new(),
            widths: HashMap::new(),
            family: None,
        })
    }

    fn font(&mut self, px: f32, bold: bool) -> Option<(Hfont, f32, f32, f32)> {
        let key = (((px * 4.0).round() as i32).max(4) as u16, bold);
        if let Some(entry) = self.fonts.get(&key) {
            return Some(*entry);
        }
        let family = self.family.as_deref().unwrap_or(DEFAULT_FAMILY);
        let face: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
        let height = -(px.round() as i32).max(1);
        let font = unsafe {
            CreateFontW(
                height,
                0,
                0,
                0,
                if bold { FW_BOLD } else { FW_NORMAL },
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                DEFAULT_PITCH,
                face.as_ptr(),
            )
        };
        if font.is_null() {
            return None;
        }
        unsafe { SelectObject(self.scratch.dc, font) };
        let (ascent, descent, line_height) = self.read_vmetrics(px);
        let entry = (font, ascent, descent, line_height);
        self.fonts.insert(key, entry);
        Some(entry)
    }

    fn read_vmetrics(&self, _px: f32) -> (f32, f32, f32) {
        let mut tm = TextMetricW::default();
        let ok = unsafe { GetTextMetricsW(self.scratch.dc, &mut tm) };
        if ok == 0 {
            return (0.0, 0.0, 0.0);
        }
        (tm.ascent as f32, tm.descent as f32, tm.height as f32)
    }

    fn vmetrics(&mut self, px: f32, bold: bool) -> (f32, f32, f32) {
        match self.font(px, bold) {
            Some((_, ascent, descent, height)) => (ascent, descent, height),
            None => (px * 0.8, px * 0.2, px * 1.2),
        }
    }

    fn text_width(&mut self, text: &str, px: f32, bold: bool) -> f32 {
        let key = (
            text.to_string(),
            (((px * 4.0).round() as i32).max(4) as u16),
            bold,
        );
        if let Some(w) = self.widths.get(&key) {
            return *w;
        }
        let font = match self.font(px, bold) {
            Some((font, _, _, _)) => font,
            None => return text.chars().count() as f32 * px * 0.55,
        };
        unsafe { SelectObject(self.scratch.dc, font) };
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let mut size = Size::default();
        let ok = unsafe {
            GetTextExtentPoint32W(
                self.scratch.dc,
                utf16.as_ptr(),
                utf16.len() as i32,
                &mut size,
            )
        };
        let width = if ok != 0 { size.cx as f32 } else { 0.0 };
        // 缓存别无限长：面板的字符串集合是有限的，到量了整体清一次最省事。
        if self.widths.len() > 4096 {
            self.widths.clear();
        }
        self.widths.insert(key, width);
        width
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 字体家族是**进程级全局状态**，而 `cargo test` 默认多线程跑。
    ///
    /// 没有这把锁时，一个测试正在断言"Consolas 的 i 和 M 等宽"，
    /// 另一个测试刚好 `set_family(None)`，断言就会莫名其妙地失败 ——
    /// 实测撞到过一次（同一份代码重跑即通过）。测试要确定性，就得自己串行。
    static FONT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        FONT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    const W: usize = 800;
    const H: usize = 200;

    /// 在空缓冲上画一次，返回整张缓冲（预乘 RGBA）。
    fn render(text: &str, px: f32) -> Vec<u8> {
        let blank = vec![0u8; W * H * 4];
        draw(
            &blank,
            W,
            H,
            text,
            0.0,
            150.0,
            px,
            [255, 255, 255, 255],
            false,
            None,
        )
        .expect("draw must succeed")
    }

    /// 墨迹包围盒：x0, y0, x1, y1（闭区间）。
    fn bbox(out: &[u8]) -> (usize, usize, usize, usize) {
        let (mut x0, mut y0, mut x1, mut y1) = (W, H, 0usize, 0usize);
        for y in 0..H {
            for x in 0..W {
                if out[(y * W + x) * 4] > 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0, y0, x1, y1)
    }

    /// 换家族必须真的换字形，而且换回来要能换回去。
    ///
    /// 判据用等宽字体当探针：Consolas 里 `i` 和 `M` 一样宽，比例字体里差好几倍。
    /// 只看"宽度变了"不够 —— 缓存没清、字体没换都可能凑巧宽度不同，
    /// 所以两侧都比。
    #[test]
    fn switching_family_changes_glyph_widths() {
        let _guard = lock();
        let default_probe = {
            set_family(None);
            (measure("iiii", 20.0, false), measure("MMMM", 20.0, false))
        };
        if !set_family(Some("Consolas".to_string())) {
            eprintln!("Consolas not available, skipping");
            return;
        }
        let mono_probe = (measure("iiii", 20.0, false), measure("MMMM", 20.0, false));
        set_family(None);
        let back = (measure("iiii", 20.0, false), measure("MMMM", 20.0, false));

        assert!(
            (mono_probe.0 - mono_probe.1).abs() < 0.5,
            "Consolas 应该是等宽：i={} M={}",
            mono_probe.0,
            mono_probe.1
        );
        assert!(
            (default_probe.0 - default_probe.1).abs() > 2.0,
            "默认字体应该是比例字体：i={} M={}",
            default_probe.0,
            default_probe.1
        );
        assert_eq!(back.0, default_probe.0, "换回默认字体后度量必须复原");
        assert_eq!(back.1, default_probe.1);
    }

    /// 注册字体文件要说真话：坏文件必须返回 false，不能假装成功。
    #[test]
    fn add_font_file_rejects_garbage() {
        let _guard = lock();
        let dir = std::env::temp_dir().join("zouwu-font-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let bogus = dir.join("not-a-font.ttf");
        std::fs::write(&bogus, b"definitely not a font").expect("write");
        assert!(!add_font_file(&bogus));
        assert!(!add_font_file(&dir.join("missing.ttf")));
    }

    /// 一笔绘制的结果不能取决于"上一笔是多大字号"。
    ///
    /// 守的是一个隐式假设：`draw_with` 曾经不自己 `SelectObject`，而是指望
    /// "上一步 `text_width` 刚好替我们选过同一个字号"。宽度缓存一命中，
    /// DC 上留的还是上一笔的字体 —— 18px 的文字用 24px 的字形画出来，
    /// 既变宽（超出按 18px 分配的擦洗缓冲，右边被裁）又整体下移，
    /// 面板上看到的就是"文字被遮挡 / 缺字"。
    ///
    /// 判据用**墨迹包围盒**：先按 18px 量一遍作基准，再分别用 24/32/12px 预热后
    /// 画同一段 18px 文字，三次必须一样。实测三种预热分别得到高 16 / 21 / 19，
    /// 所以这条测试确实能抓住它（缓冲区裁剪会让"墨迹右边界"看不出来，别用那个）。
    #[test]
    fn output_does_not_depend_on_the_previous_draw() {
        let _guard = lock();
        let text = "国";
        let prime = |px: f32| {
            let _ = render(text, px);
        };

        prime(18.0);
        let (x0, y0, x1, y1) = bbox(&render(text, 18.0));
        let base = (x1 - x0, y1 - y0, y0);

        for other in [24.0f32, 32.0, 12.0] {
            prime(other);
            let (x0, y0, x1, y1) = bbox(&render(text, 18.0));
            assert_eq!(
                (x1 - x0, y1 - y0, y0),
                base,
                "{other}px 预热后画的 18px 字形与基准不同：字体没有被显式选进 DC"
            );
        }
    }

    /// 墨迹宽度必须与 `measure` 相称（防"字号没生效"这类退化）。
    #[test]
    fn ink_width_tracks_measure_across_sizes() {
        let _guard = lock();
        let text = "打开配置文件";
        for px in [12.0f32, 14.0, 18.0, 24.0] {
            let (x0, _, x1, _) = bbox(&render(text, px));
            let ink = (x1 - x0 + 1) as f32;
            let measured = measure(text, px, false);
            assert!(
                (ink - measured).abs() < px * 0.6,
                "{px}px: ink {ink} px vs measured {measured:.1} px"
            );
        }
    }
}
