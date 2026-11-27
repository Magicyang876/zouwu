//! Linux 文本栅格化：fontconfig 找字体文件 + FreeType 出覆盖率。
//!
//! ## 为什么不是"纯 Rust 字体库"
//!
//! 和 Windows 侧同样的理由（见 `text_gdi`）：`fontdue` 这类库把字面整个读进
//! 进程，一套 CJK 就是上百 MB。这里走的是 GDI 的等价路线——字体文件由系统
//! 提供、fontconfig 负责挑、FreeType 只为我们真正用到的字形出位图，常驻增量
//! 和 Windows 版同一量级。
//!
//! ## 做法
//!
//! 和 Windows 版保持同一套契约，调用点（`zouwu_ui::text`、`settings`）零改动：
//!
//! - 字体由 `fc-match` 解析成 `(文件, ttc 内部序号)`，缺字时按字符回落到
//!   `:lang=zh-cn`（中文）、再由 fontconfig 兜底；
//! - 逐字符 `load_char` 出 8 位灰度覆盖率，按 Windows 版同一个公式混进目标
//!   预乘 RGBA 缓冲（`src = color * coverage`，再以 `alpha` 做 over）；
//! - 家族名不存在时不报错，静默换成系统默认字体——"拿不准就说能画"和
//!   Windows 版的行为一致。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use freetype::bitmap::PixelMode;
use freetype::face::LoadFlag;
use freetype::{Face, Library};

pub struct Metrics {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
}

/// 与 Windows 后端同名同义：系统默认字体的家族名。
///
/// Windows 上这是个 GDI 保留名；在这里表示"交给 fontconfig 选"。
pub const DEFAULT_FAMILY: &str = "system";

/// 字体选择里"没有指定家族"时问 fontconfig 的模式。
const SANS: &str = "sans-serif";

/// 缺字回落的模式：中文正文字体。
const CJK: &str = "sans-serif:lang=zh-cn";

/// 一个已解析的字体文件：`.ttc` 要用内部序号才选得中其中一面。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FontId {
    path: PathBuf,
    index: isize,
}

struct Engine {
    lib: Library,
    /// 已经打开的字体文件。FreeType 的 `Face` 不是 `Send`，但整个引擎都在
    /// 同一把锁里用（和 Windows 版把 GDI 关在锁里是同一做法）。
    faces: HashMap<FontId, Face>,
    /// 每个字体当前设到的字号（像素），省掉重复的 `set_pixel_sizes`。
    sizes: HashMap<FontId, u32>,
    /// 用户/皮肤额外注册的字体文件（`add_font_file`）。
    extra: Vec<(FontId, String)>,
    /// 家族 → (常规字体, 粗体字体)；`None` 表示"问过但解析不到"。
    resolved: HashMap<(Option<String>, bool), Option<FontId>>,
    cjk: Option<Option<FontId>>,
    /// 文本宽度缓存：面板一帧要量几十次同样的字符串。
    widths: HashMap<(String, u16, bool), f32>,
    family: Option<String>,
}

// 全部访问都在 `with_engine` 的锁内完成；FontId 只是路径。
unsafe impl Send for Engine {}

fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
    static ENGINE: OnceLock<Mutex<Option<Engine>>> = OnceLock::new();
    let slot = ENGINE.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().ok()?;
    if guard.is_none() {
        *guard = Engine::create();
    }
    guard.as_mut().map(f)
}

impl Engine {
    fn create() -> Option<Engine> {
        match Library::init() {
            Ok(lib) => Some(Engine {
                lib,
                faces: HashMap::new(),
                sizes: HashMap::new(),
                extra: Vec::new(),
                resolved: HashMap::new(),
                cjk: None,
                widths: HashMap::new(),
                family: None,
            }),
            Err(err) => {
                log::error!("text: cannot initialise FreeType: {err}");
                None
            }
        }
    }

    /// 解析一个 fontconfig 模式成具体字体文件。解析不到返回 `None`（不刷屏）。
    fn query(&self, pattern: &str) -> Option<FontId> {
        let out = Command::new("fc-match")
            .args(["-f", "%{file}\u{1}%{index}"])
            .arg(pattern)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut parts = text.split('\u{1}');
        let path = parts.next()?.trim();
        let index: isize = parts
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        if path.is_empty() {
            return None;
        }
        Some(FontId {
            path: PathBuf::from(path),
            index,
        })
    }

    fn face(&mut self, id: &FontId) -> Option<&mut Face> {
        if !self.faces.contains_key(id) {
            match self.lib.new_face(&id.path, id.index) {
                Ok(face) => {
                    self.faces.insert(id.clone(), face);
                }
                Err(err) => {
                    log::warn!("text: cannot open {}: {err}", id.path.display());
                    return None;
                }
            }
        }
        self.faces.get_mut(id)
    }

    /// 把字体设到指定像素高度（FreeType 的尺寸是字体状态，得每次确认）。
    fn prepare(&mut self, id: &FontId, px: f32) -> Option<&mut Face> {
        let px_u32 = (px.round() as u32).max(1);
        if self.sizes.get(id).copied() != Some(px_u32) {
            let face = self.face(id)?;
            if let Err(err) = face.set_pixel_sizes(0, px_u32) {
                log::warn!("text: set_pixel_sizes({px_u32}) failed: {err}");
                return None;
            }
            self.sizes.insert(id.clone(), px_u32);
        }
        self.face(id)
    }

    fn family_key(&self) -> Option<String> {
        match self.family.as_deref() {
            None | Some("") | Some(DEFAULT_FAMILY) => None,
            Some(name) => Some(name.to_string()),
        }
    }

    /// 某个家族（可粗体）对应的字体文件；解析过的结果会缓存。
    fn resolve(&mut self, bold: bool) -> Option<FontId> {
        let family = self.family_key();
        if let Some(hit) = self.resolved.get(&(family.clone(), bold)) {
            return hit.clone();
        }
        let mut found = None;
        // 先看皮肤/用户注册进来的字体文件（`add_font_file`），它们优先于系统同名族。
        if let Some(name) = family.as_deref() {
            for (id, face_family) in &self.extra {
                if face_family.eq_ignore_ascii_case(name) {
                    found = Some(id.clone());
                    break;
                }
            }
        }
        if found.is_none() {
            let base = family.as_deref().unwrap_or(SANS);
            let pattern = if bold {
                format!("{base}:bold")
            } else {
                base.to_string()
            };
            found = self.query(&pattern);
            // 没有该家族时 fontconfig 会给默认字体；粗体拿不到真字面时回落到常规。
            if found.is_none() && bold {
                found = self.query(base);
            }
            if found.is_none() {
                log::warn!("text: font family `{base}` not found, using {SANS}");
                found = self.query(SANS);
            }
        }
        self.resolved.insert((family, bold), found.clone());
        found
    }

    fn cjk_face(&mut self) -> Option<FontId> {
        if let Some(hit) = &self.cjk {
            return hit.clone();
        }
        let found = self.query(CJK).or_else(|| self.query(SANS));
        self.cjk = Some(found.clone());
        found
    }

    fn has_glyph(&mut self, id: &FontId, ch: char) -> bool {
        self.face(id)
            .map(|face| face.get_char_index(ch as usize).is_some())
            .unwrap_or(false)
    }

    fn text_width(&mut self, text: &str, px: f32, bold: bool) -> f32 {
        let key = ((px * 4.0).round().max(4.0) as u16, bold);
        if let Some(hit) = self.widths.get(&(text.to_string(), key.0, key.1)) {
            return *hit;
        }
        let Some(id) = self.resolve(bold) else {
            return text.chars().count() as f32 * px * 0.55;
        };
        let cjk = self.cjk_face();
        let mut width = 0.0f32;
        for ch in text.chars() {
            let use_cjk = cjk
                .clone()
                .filter(|cjk| cjk != &id && !self.has_glyph(&id, ch));
            let target = use_cjk.unwrap_or_else(|| id.clone());
            let Some(face) = self.prepare(&target, px) else {
                continue;
            };
            if face.load_char(ch as usize, LoadFlag::empty()).is_ok() {
                width += face.glyph().advance().x as f32 / 64.0;
            }
        }
        self.widths.insert((text.to_string(), key.0, key.1), width);
        width
    }

    fn vmetrics(&mut self, px: f32, bold: bool) -> Metrics {
        let fallback = Metrics {
            width: 0.0,
            ascent: px * 0.8,
            descent: px * 0.2,
            line_height: px * 1.2,
        };
        let Some(id) = self.resolve(bold) else {
            return fallback;
        };
        let Some(face) = self.prepare(&id, px) else {
            return fallback;
        };
        let Some(m) = face.size_metrics() else {
            return fallback;
        };
        if m.height <= 0 {
            return fallback;
        }
        Metrics {
            width: 0.0,
            ascent: m.ascender as f32 / 64.0,
            descent: (-m.descender) as f32 / 64.0,
            line_height: m.height as f32 / 64.0,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blend(
    out: &mut [u8],
    pm_width: usize,
    pm_height: usize,
    dst_x0: i32,
    dst_y0: i32,
    bitmap: &freetype::Bitmap,
    color: [u8; 4],
    clip: Option<(f32, f32, f32, f32)>,
) {
    let width = bitmap.width();
    let rows = bitmap.rows();
    let pitch = bitmap.pitch() as usize;
    let buffer = bitmap.buffer();
    if width <= 0 || rows <= 0 || pitch == 0 {
        return;
    }
    let (cx0, cy0, cx1, cy1) = clip.unwrap_or((0.0, 0.0, pm_width as f32, pm_height as f32));
    for row in 0..rows {
        let dst_y = dst_y0 + row;
        if dst_y < 0 || dst_y >= pm_height as i32 {
            continue;
        }
        if (dst_y as f32) < cy0 || (dst_y as f32) >= cy1 {
            continue;
        }
        let line = row as usize * pitch;
        for col in 0..width {
            let dst_x = dst_x0 + col;
            if dst_x < 0 || dst_x >= pm_width as i32 {
                continue;
            }
            if (dst_x as f32) < cx0 || (dst_x as f32) >= cx1 {
                continue;
            }
            let Some(src) = buffer.get(line + col as usize).copied() else {
                continue;
            };
            if src == 0 {
                continue;
            }
            let coverage = src as u32;
            let idx = ((dst_y as usize) * pm_width + dst_x as usize) * 4;
            let alpha = color[3] as u32 * coverage / 255;
            // 和 Windows 版同一个公式：目标与源都按预乘处理。
            for ch in 0..3 {
                let s = color[ch] as u32 * coverage / 255;
                let d = out[idx + ch] as u32;
                out[idx + ch] = (s + d * (255 - alpha) / 255).min(255) as u8;
            }
            let d = out[idx + 3] as u32;
            out[idx + 3] = (alpha + d * (255 - alpha) / 255).min(255) as u8;
        }
    }
}

/// 皮肤/用户自带字体登记进来：之后 `set_family(字体家族名)` 就能选中它。
pub fn add_font_file(path: &Path) -> bool {
    let id = FontId {
        path: path.to_path_buf(),
        index: 0,
    };
    with_engine(|engine| {
        let Some(face) = engine.face(&id) else {
            return false;
        };
        let Some(family) = face.family_name() else {
            return false;
        };
        engine.extra.push((id.clone(), family.clone()));
        engine.resolved.clear();
        engine.widths.clear();
        log::debug!("text: registered extra font {} ({family})", path.display());
        true
    })
    .unwrap_or(false)
}

pub fn family() -> Option<String> {
    with_engine(|engine| engine.family.clone()).flatten()
}

pub fn set_family(family: Option<String>) -> bool {
    with_engine(|engine| {
        engine.family = family;
        engine.resolved.clear();
        engine.widths.clear();
        true
    })
    .unwrap_or(false)
}

/// "系统的界面字体"：先问 KDE 的 `kdeglobals`（KWin 下这才是用户在用的），
/// 再问 GTK 的 `settings.ini`，最后交给 fontconfig。
pub fn os_ui_family() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            kde_font()
                .or_else(gtk_font)
                .or_else(|| fc_family(SANS))
                .filter(|name| !name.is_empty())
        })
        .clone()
}

/// 界面字体，读不到时回落到系统默认。
pub fn os_family() -> String {
    os_ui_family().unwrap_or_else(|| DEFAULT_FAMILY.to_string())
}

/// 这个家族有没有中文字形：直接问字体文件的 cmap。
pub fn family_covers_cjk(family: &str) -> bool {
    let probe = |engine: &mut Engine| {
        let id = if family.eq_ignore_ascii_case(DEFAULT_FAMILY) || family.is_empty() {
            engine.resolve(false)
        } else if let Some(found) = engine
            .extra
            .iter()
            .find(|(_, name)| name.eq_ignore_ascii_case(family))
            .map(|(id, _)| id.clone())
        {
            Some(found)
        } else {
            engine.query(family).or_else(|| engine.query(SANS))
        };
        let Some(id) = id else {
            return true;
        };
        engine
            .face(&id)
            .map(|face| face.get_char_index('中' as usize).is_some())
            .unwrap_or(true)
    };
    with_engine(probe).unwrap_or(true)
}

/// 单行文本的浮点像素宽度。
pub fn measure(text: &str, px: f32, bold: bool) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    with_engine(|engine| engine.text_width(text, px, bold)).unwrap_or_default()
}

/// 一行文本的纵向度量。
pub fn metrics(px: f32, bold: bool) -> Metrics {
    with_engine(|engine| engine.vmetrics(px, bold)).unwrap_or(Metrics {
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
    if text.is_empty() || pm_width == 0 || pm_height == 0 {
        return None;
    }
    let mut out = pixmap.to_vec();
    let mut pen = x;
    let mut drew = false;
    with_engine(|engine| {
        let Some(primary) = engine.resolve(bold) else {
            return;
        };
        // 粗体拿不到真粗字面时（fontconfig 给回了常规文件）用"描两遍"顶替，
        // 否则标题和正文看起来一样粗。
        let synthetic_bold = bold
            && engine
                .resolve(false)
                .map(|regular| regular == primary)
                .unwrap_or(false);
        let cjk = engine.cjk_face();
        let baseline = baseline.round();
        for ch in text.chars() {
            let use_cjk = cjk
                .clone()
                .filter(|cjk| cjk != &primary && !engine.has_glyph(&primary, ch));
            let target = use_cjk.unwrap_or_else(|| primary.clone());
            let Some(face) = engine.prepare(&target, px) else {
                continue;
            };
            if face
                .load_char(ch as usize, LoadFlag::RENDER | LoadFlag::TARGET_NORMAL)
                .is_err()
            {
                continue;
            }
            let slot = face.glyph();
            let advance = slot.advance().x as f32 / 64.0;
            let bitmap = slot.bitmap();
            let left = slot.bitmap_left();
            let top = slot.bitmap_top();
            let mode = bitmap.pixel_mode().ok();
            // 彩色字形（emoji 等）是 BGRA 位图，这里只处理 8 位灰度；
            // 碰到彩色字形就只推进笔位，不画（宁可空白也不画成一团色块）。
            if mode == Some(PixelMode::Gray) {
                let dst_x0 = pen.round() as i32 + left;
                let dst_y0 = baseline as i32 - top;
                blend(
                    &mut out, pm_width, pm_height, dst_x0, dst_y0, &bitmap, color, clip,
                );
                if synthetic_bold {
                    blend(
                        &mut out,
                        pm_width,
                        pm_height,
                        dst_x0 + 1,
                        dst_y0,
                        &bitmap,
                        color,
                        clip,
                    );
                }
                drew = true;
            }
            pen += advance;
        }
    });
    if drew {
        Some(out)
    } else {
        None
    }
}

fn fc_family(pattern: &str) -> Option<String> {
    let out = Command::new("fc-match")
        .args(["-f", "%{family}"])
        .arg(pattern)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // fontconfig 会用逗号列出同一字面的官方名/别名，取第一个。
    text.split(',').next().map(|s| s.trim().to_string())
}

fn config_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
}

/// 从 ini 风格的配置里取一个 `key=value`（KDE 与 GTK 都能用）。
fn ini_value(path: &Path, section: Option<&str>, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_section = section.is_none();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_section =
                section.is_some_and(|want| line[1..line.len() - 1].eq_ignore_ascii_case(want));
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// KDE 的界面字体，形如 `Noto Sans,10,-1,5,50,0,0,0,0,0`（只需第一段）。
fn kde_font() -> Option<String> {
    let home = config_home()?;
    let raw = ini_value(&home.join("kdeglobals"), Some("General"), "font")?;
    let name = raw.split(',').next()?.trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// GNOME/GTK 的界面字体，形如 `Noto Sans 10`（去掉尾部字号）。
fn gtk_font() -> Option<String> {
    let home = config_home()?;
    for file in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
        let Some(raw) = ini_value(&home.join(file), Some("Settings"), "gtk-font-name") else {
            continue;
        };
        let name = raw
            .rsplit_once(' ')
            .filter(|(_, size)| size.parse::<f32>().is_ok())
            .map(|(name, _)| name)
            .unwrap_or(&raw)
            .trim()
            .to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

/// `sysfont` 面板要用：系统里有哪些家族可选。
pub fn installed_families() -> Vec<String> {
    let Ok(out) = Command::new("fc-list").args(["-f", "%{family}\n"]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .flat_map(|line| line.split(','))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}
