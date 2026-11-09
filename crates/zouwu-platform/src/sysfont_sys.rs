//! `sysfont` 的 Windows 实现：读/写系统字体（`SystemParametersInfoW`）、枚举字体、
//! 以及 `FontSubstitutes` 的全局替换。
//!
//! 数据结构与编码在父模块 `sysfont.rs` 里；这里只有 FFI 与副作用。

use std::ffi::c_void;
use std::ptr;

use super::{
    entry_of, write_entry, Entry, LogFontW, NonClientMetricsW, Snapshot, Target, DEFAULT_CHARSET,
    DEFAULT_DPI,
};

const SPI_GETNONCLIENTMETRICS: u32 = 0x0029;
const SPI_SETNONCLIENTMETRICS: u32 = 0x002A;
const SPI_GETICONTITLELOGFONT: u32 = 0x001F;
const SPI_SETICONTITLELOGFONT: u32 = 0x0021;
/// 让系统把值写进用户配置（不带它只改内存，注销就没了）。
const SPIF_UPDATEINIFILE: u32 = 0x01;
const WM_SETTINGCHANGE: u32 = 0x001A;
const SMTO_ABORTIFHUNG: u32 = 0x0002;
const HWND_BROADCAST: isize = 0xFFFF;

type Hdc = *mut c_void;

/// `ENUMLOGFONTEXW` 的前半段：枚举时只读家族名，后面三个 64 字节的字符串不要。
#[repr(C)]
struct EnumLogFontExW {
    log_font: LogFontW,
    full_name: [u16; 64],
    style: [u16; 32],
    script: [u16; 32],
}

#[link(name = "user32")]
extern "system" {
    fn SystemParametersInfoW(action: u32, param: u32, data: *mut c_void, win_ini: u32) -> i32;
    fn SendMessageTimeoutW(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
        flags: u32,
        timeout: u32,
        result: *mut usize,
    ) -> isize;
    fn GetDC(hwnd: *mut c_void) -> Hdc;
    fn ReleaseDC(hwnd: *mut c_void, hdc: Hdc) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn EnumFontFamiliesExW(
        hdc: Hdc,
        log_font: *const LogFontW,
        callback: extern "system" fn(*const EnumLogFontExW, *const c_void, u32, isize) -> i32,
        lparam: isize,
        flags: u32,
    ) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn IsUserAnAdmin() -> i32;
}

fn read_metrics() -> Option<NonClientMetricsW> {
    let mut metrics = NonClientMetricsW::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.size,
            &mut metrics as *mut NonClientMetricsW as *mut c_void,
            0,
        )
    };
    (ok != 0).then_some(metrics)
}

fn read_icon_font() -> Option<LogFontW> {
    let mut font = LogFontW::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETICONTITLELOGFONT,
            std::mem::size_of::<LogFontW>() as u32,
            &mut font as *mut LogFontW as *mut c_void,
            0,
        )
    };
    (ok != 0).then_some(font)
}

/// 当前系统字体。拿不到就 `None`（界面显示"读不出来"，不瞎猜）。
pub fn current() -> Option<Snapshot> {
    let metrics = read_metrics()?;
    let mut items = Vec::with_capacity(Target::ALL.len());
    for target in Target::ALL {
        let entry = match target {
            Target::Menu => entry_of(&metrics.menu_font),
            Target::Message => entry_of(&metrics.message_font),
            Target::Status => entry_of(&metrics.status_font),
            Target::Palette => entry_of(&metrics.sm_caption_font),
            Target::Icon => entry_of(&read_icon_font()?),
        };
        items.push((target.id().to_string(), entry));
    }
    Some(Snapshot {
        taken: crate::logging::timestamp(),
        items,
    })
}

/// 菜单行高、调色板标题高这些是**像素**，字号变了不跟着改就会被裁字。
///
/// 按比例缩放后夹在一个合理区间：下限"字高 + 一点边距"，上限"字高的两倍再多一点"。
fn rescale_band(old: i32, old_height: i32, new_height: i32, dpi: i32) -> i32 {
    let old_height = old_height.abs().max(1);
    let new_height = new_height.abs().max(1);
    let scaled = old.max(1) * new_height / old_height;
    scaled.clamp(new_height + 4 * dpi / 96, 2 * new_height + 12 * dpi / 96)
}

/// 把一组值写进系统。
///
/// 只写传进来的那几项，其余原样回写——`SPI_SETNONCLIENTMETRICS` 是**整块替换**，
/// 少带一个字段就等于把它清零。
pub fn apply(entries: &[(Target, Entry)]) -> Result<(), String> {
    if entries.is_empty() {
        return Err("nothing to apply".to_string());
    }
    // 两遍：先按给定的写，读回来对不上就按**实测比例**修一次再写。
    // SPI 会按系统 DPI 缩放 `lfHeight`（本机 125%：写 -14 读回 -11），
    // 不修的话每点一次"应用"字体就小一圈，"还原"也救不回来。
    let desired = entries.to_vec();
    let mut attempt: Vec<(Target, Entry)> = entries.to_vec();
    for round in 0..2 {
        push(&attempt)?;
        let Some(readback) = current() else {
            return Ok(());
        };
        let mut fixed = attempt.clone();
        let mut off = false;
        for (target, wanted) in &attempt {
            // `wanted` 是最终目标；`written` 是上一次尝试的写法（用于算缩放比例）。
            let Some(got) = readback.get(*target) else {
                continue;
            };
            let written = desired
                .iter()
                .find(|(t, _)| t == target)
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| wanted.clone());
            if let Some(height) =
                crate::sysfont::calibrate(wanted.height, written.height, got.height)
            {
                log::info!(
                    "sysfont: {} height {} came back {}, retrying with {}",
                    target.id(),
                    wanted.height,
                    got.height,
                    height
                );
                if let Some(slot) = fixed.iter_mut().find(|(t, _)| t == target) {
                    slot.1.height = height;
                }
                off = true;
            }
        }
        if !off {
            return Ok(());
        }
        if round == 1 {
            log::warn!("sysfont: height still differs after calibration, leaving as is");
            return Ok(());
        }
        attempt = fixed;
    }
    Ok(())
}

/// 真正写一次 `NONCLIENTMETRICS` + `ICONTITLELOGFONT`。
fn push(entries: &[(Target, Entry)]) -> Result<(), String> {
    let mut metrics = read_metrics().ok_or("SPI_GETNONCLIENTMETRICS failed")?;
    let mut icon_font = read_icon_font().ok_or("SPI_GETICONTITLELOGFONT failed")?;
    let dpi = DEFAULT_DPI as i32;

    let mut touched_icon = false;
    for (target, entry) in entries {
        match target {
            Target::Menu => {
                let before = metrics.menu_font.height;
                write_entry(&mut metrics.menu_font, entry);
                metrics.menu_height = rescale_band(metrics.menu_height, before, entry.height, dpi);
            }
            Target::Message => write_entry(&mut metrics.message_font, entry),
            Target::Status => write_entry(&mut metrics.status_font, entry),
            Target::Palette => {
                let before = metrics.sm_caption_font.height;
                write_entry(&mut metrics.sm_caption_font, entry);
                metrics.sm_caption_height =
                    rescale_band(metrics.sm_caption_height, before, entry.height, dpi);
            }
            Target::Icon => {
                write_entry(&mut icon_font, entry);
                touched_icon = true;
            }
        }
    }

    // Windows 11 上 `iPaddedBorderWidth` 为 0 时窗口边框会贴死内容；
    // noMeiryoUI 也做这一步，代价为零。
    if metrics.padded_border_width == 0 {
        metrics.padded_border_width = 1 + dpi / 96;
    }
    metrics.size = std::mem::size_of::<NonClientMetricsW>() as u32;

    let ok = unsafe {
        SystemParametersInfoW(
            SPI_SETNONCLIENTMETRICS,
            metrics.size,
            &mut metrics as *mut NonClientMetricsW as *mut c_void,
            SPIF_UPDATEINIFILE,
        )
    };
    if ok == 0 {
        return Err("SPI_SETNONCLIENTMETRICS failed".to_string());
    }

    if touched_icon {
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_SETICONTITLELOGFONT,
                std::mem::size_of::<LogFontW>() as u32,
                &mut icon_font as *mut LogFontW as *mut c_void,
                SPIF_UPDATEINIFILE,
            )
        };
        if ok == 0 {
            return Err("SPI_SETICONTITLELOGFONT failed".to_string());
        }
    }

    broadcast_change();
    Ok(())
}

/// 告诉所有窗口"设置变了"。
///
/// 不用 `SPIF_SENDCHANGE`：那个标志会**同步**给每个顶层窗口发消息，任何一个卡住的
/// 窗口都会把调用方一起拖住（ADR-037 那次卡死就是这么来的）。这里用带超时的广播。
fn broadcast_change() {
    let mut result = 0usize;
    for action in [SPI_SETNONCLIENTMETRICS, SPI_SETICONTITLELOGFONT] {
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                action as usize,
                0,
                SMTO_ABORTIFHUNG,
                2000,
                &mut result,
            );
        }
    }
}

/// 还原一份快照。只写我们管的那几项——标题栏没动过，也就不回写。
pub fn restore(snapshot: &Snapshot) -> Result<(), String> {
    let entries: Vec<(Target, Entry)> = Target::ALL
        .into_iter()
        .filter_map(|target| snapshot.get(target).map(|e| (target, e.clone())))
        .collect();
    apply(&entries)
}

extern "system" fn collect_face(
    log_font: *const EnumLogFontExW,
    _metrics: *const c_void,
    _font_type: u32,
    lparam: isize,
) -> i32 {
    let out = unsafe { &mut *(lparam as *mut Vec<String>) };
    let face = unsafe { (*log_font).log_font.face() };
    // '@' 开头的是横排字体的"竖排别名"，选了它文字会全体躺下。
    if !face.is_empty() && !face.starts_with('@') {
        out.push(face);
    }
    1 // 非 0 = 继续枚举
}

/// 系统里装着的字体家族名（按名字排序、去重）。
pub fn installed_faces() -> Vec<String> {
    let mut faces: Vec<String> = Vec::new();
    let hdc = unsafe { GetDC(ptr::null_mut()) };
    if hdc.is_null() {
        log::warn!("sysfont: GetDC failed, cannot enumerate faces");
        return faces;
    }
    let mut query = LogFontW::default();
    query.char_set = DEFAULT_CHARSET;
    unsafe {
        EnumFontFamiliesExW(
            hdc,
            &query,
            collect_face,
            &mut faces as *mut Vec<String> as isize,
            0,
        );
        ReleaseDC(ptr::null_mut(), hdc);
    }
    faces.sort_by_key(|f| f.to_lowercase());
    faces.dedup();
    faces
}

const FONT_SUBSTITUTES: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\FontSubstitutes";
const SEGOE_UI: &str = "Segoe UI";

/// 现在 `Segoe UI` 被替换成了哪个字体（没替换过 = `None`）。
pub fn substitute_face() -> Option<String> {
    crate::reg::get_string_in(crate::reg::Root::LocalMachine, FONT_SUBSTITUTES, SEGOE_UI)
}

/// 把 `Segoe UI` 替换成 `face`；`None` = 删掉这条替换、恢复系统默认。
///
/// **需要管理员**：`HKLM` 没有普通用户的写权限，所以先自己判一次，
/// 免得把 `ERROR_ACCESS_DENIED` 变成一句看不懂的失败。
pub fn set_substitute(face: Option<&str>) -> Result<(), String> {
    if !is_admin() {
        return Err("needs administrator".to_string());
    }
    let root = crate::reg::Root::LocalMachine;
    match face {
        Some(face) => {
            if crate::reg::set_string_in(root, FONT_SUBSTITUTES, SEGOE_UI, face) {
                Ok(())
            } else {
                Err(format!("cannot write FontSubstitutes\\{SEGOE_UI}"))
            }
        }
        None => {
            if crate::reg::delete_value_in(root, FONT_SUBSTITUTES, SEGOE_UI) {
                Ok(())
            } else {
                Err(format!("cannot delete FontSubstitutes\\{SEGOE_UI}"))
            }
        }
    }
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

#[cfg(test)]
mod tests {
    use super::super::{height_for_pt, pt_for_height};
    use super::*;

    #[test]
    fn pt_and_height_agree() {
        for pt in [9.0f32, 10.0, 12.0] {
            let h = height_for_pt(pt);
            let back = pt_for_height(h);
            assert!((back - pt).abs() < 1.0, "{pt}pt -> {h} -> {back}pt");
        }
    }

    #[test]
    fn menu_height_keeps_the_font_inside_it() {
        // 19 px 的行高配 12 px 的字，换成 16 px 的字之后行高必须跟着涨。
        let grown = rescale_band(19, -12, -16, 96);
        assert!(grown >= 16 + 4, "行高 {grown} 装不下 16 px 的字");
        // 字号变小的时候行高也收，但不能收得比字还矮。
        let shrunk = rescale_band(19, -16, -11, 96);
        assert!(shrunk >= 11 + 4, "行高 {shrunk} 太窄");
    }

    #[test]
    fn enumerating_faces_never_panics() {
        // 拿不到 DC 的会话里返回空表是允许的，但不能崩。
        for face in installed_faces() {
            assert!(!face.starts_with('@'), "竖排别名不该出现: {face}");
            assert!(!face.is_empty());
        }
    }

    #[test]
    fn current_matches_what_apply_would_write() {
        // 只验证"读得出来且字段成形"，不改系统：读到的家族名不该是空串。
        let Some(snapshot) = current() else {
            return; // 无头会话
        };
        for target in Target::ALL {
            let entry = snapshot.get(target).expect("every target present");
            assert!(!entry.face.is_empty(), "{} has no face", target.id());
            assert!(entry.height < 0, "{} has non-negative height", target.id());
        }
    }
}
