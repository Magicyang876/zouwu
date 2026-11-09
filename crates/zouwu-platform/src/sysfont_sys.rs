//! `sysfont` 的 Windows 实现：读/写系统字体（`SystemParametersInfoW`）、枚举字体、
//! 以及 `FontSubstitutes` 的全局替换。
//!
//! 数据结构与编码在父模块 `sysfont.rs` 里；这里只有 FFI 与副作用。

use std::ffi::c_void;
use std::ptr;

use super::{
    caption_is_broken, entry_of, write_entry, Chrome, Entry, LogFontW, NonClientMetricsW, Snapshot,
    Target, DEFAULT_CHARSET, DEFAULT_DPI, STANDARD_CHROME,
};

const SPI_GETNONCLIENTMETRICS: u32 = 0x0029;
const SPI_SETNONCLIENTMETRICS: u32 = 0x002A;
const SPI_GETICONTITLELOGFONT: u32 = 0x001F;
/// 图标字体。**别写成 `0x0021`** —— 那是 `SPI_SETMOUSEBUTTONSWAP`，我们试过：写图标字体
/// 顺手把用户的鼠标左右键对调了（`param` 传的是 `sizeof(LOGFONT) = 92`，非 0 即"交换"）。
/// 常量错了不会有任何报错，`SM_SWAPBUTTON` 却是 1——`spi_constants_are_what_we_think`
/// 这个测试就是为这条踩坑留的。
const SPI_SETICONTITLELOGFONT: u32 = 0x0022;
/// 让系统把值写进用户配置（不带它只改内存，注销就没了）。
/// 「装饰尺寸」在注册表里的家。
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
    /// Win10 1607+：同一份 metrics，按**指定 DPI** 取。用来判断 SPI 返回的
    /// `lfHeight` 到底是逻辑像素还是当前 DPI 的物理像素（诊断用）。
    fn SystemParametersInfoForDpi(
        action: u32,
        param: u32,
        data: *mut c_void,
        win_ini: u32,
        dpi: u32,
    ) -> i32;
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

#[link(name = "ntdll")]
extern "system" {
    fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
}

/// `RtlGetVersion` 要的那个结构体（和 `OSVERSIONINFOW` 同布局）。
#[repr(C)]
struct OsVersionInfoW {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service_pack: [u16; 128],
}

/// 系统版本号（`build`）。用 `RtlGetVersion` 而不是注册表：注册表里的
/// `CurrentBuild` 在部分预览版上缺项，而 `GetVersionExW` 会被应用清单骗。
pub fn os_build() -> u32 {
    let mut info = OsVersionInfoW {
        size: std::mem::size_of::<OsVersionInfoW>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        service_pack: [0; 128],
    };
    let status = unsafe { RtlGetVersion(&mut info as *mut OsVersionInfoW) };
    if status < 0 {
        // 拿不到就当作“很新的系统”：宁可少改一项，也不要把用户的标题栏弄没。
        return u32::MAX;
    }
    info.build
}

/// 能不能改标题栏字体。
///
/// Windows 11 22H2（build 22621）起，改 `lfCaptionFont` 会让标题栏文字整体消失，
/// 而且**不一定能改回来**（微软社区里有把系统改坏的记录）。所以：
/// 22621 及以上一律不提供这一项，界面上说明原因。
pub fn caption_supported() -> bool {
    os_build() < 22621
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

/// `(标题栏字体高度, 标题栏高度)`，都是 `SPI_GETNONCLIENTMETRICS` 的像素值。
///
/// 只读、只给人看的诊断用：这一对我们不让人改，所以它一旦变小就说明别处在写系统
/// 字体（或者我们自己的某次写入把它缩了），`caption_is_broken` 就是拿它判断的。
pub fn caption_metrics() -> Option<(i32, i32)> {
    read_metrics().map(|metrics| metrics.caption_pair())
}

/// 一行数字：`菜单/状态栏/消息框/调色板/图标/标题栏` 的字号 + `菜单高/小标题高/标题高`
/// + `iPaddedBorderWidth`。只读，排错用（见 `sysfont::metrics_dump`）。
pub(super) fn dump() -> Option<String> {
    let metrics = read_metrics()?;
    let icon = read_icon_font()?;
    Some(format!(
        "font menu={} status={} message={} palette={} icon={} caption={} \
         | band menu={} smcaption={} caption={} \
         | width border={} scroll={}x{} caption={} smcaption={} menu={} padded={}",
        metrics.menu_font.height,
        metrics.status_font.height,
        metrics.message_font.height,
        metrics.sm_caption_font.height,
        icon.height,
        metrics.caption_font.height,
        metrics.menu_height,
        metrics.sm_caption_height,
        metrics.caption_height,
        metrics.border_width,
        metrics.scroll_width,
        metrics.scroll_height,
        metrics.caption_width,
        metrics.sm_caption_width,
        metrics.menu_width,
        metrics.padded_border_width,
    ))
}

/// 「装饰尺寸」在注册表里的家：这些尺寸的**唯一真相**，登录时读一次。
const WINDOW_METRICS_KEY: &str = r"Control Panel\Desktop\WindowMetrics";

/// 把边框/滚动条/标题栏·菜单宽度恢复成 Windows 的标准尺寸。
///
/// **只写注册表，不碰 `SPI_SETNONCLIENTMETRICS`**。两件事都试过：
/// - 那几条尺寸 SPI 基本不认（写 17 读回还是 14，只有 `iBorderWidth` 会动），
///   而它**会把同一个结构体里的字体高度重写一遍**——一不留神就把字体又缩一圈；
/// - `HKCU\Control Panel\Desktop\WindowMetrics` 才是登录时真正读的那份，`REG_SZ`、
///   单位 twips（1 px = 15 twips）、带负号，写 `-255` 就是 17 px。
///
/// 代价：**下次登录才生效**（Windows 只在登录时把这些读进会话缓存）。
pub fn restore_chrome() -> Result<Chrome, String> {
    let before = read_metrics().map(|metrics| metrics.chrome());
    let want = STANDARD_CHROME;
    for (name, px) in [
        ("BorderWidth", want.border),
        ("ScrollWidth", want.scroll_width),
        ("ScrollHeight", want.scroll_height),
        ("CaptionWidth", want.caption_width),
        ("SmCaptionWidth", want.sm_caption_width),
        ("MenuWidth", want.menu_width),
    ] {
        if !crate::reg::set_string_in(
            crate::reg::Root::CurrentUser,
            WINDOW_METRICS_KEY,
            name,
            &(-15 * px).to_string(),
        ) {
            return Err(format!("cannot write {name}"));
        }
    }
    log::warn!(
        "sysfont: chrome metrics {before:?} -> {want:?} (registry, effective after signing in)"
    );
    Ok(want)
}

/// 「装饰尺寸」（边框 / 滚动条 / 标题栏·菜单宽度）是不是已经被写大了。
///
/// 界面用它决定要不要提示“已恢复标准尺寸、重新登录生效”。读的是
/// `SPI_GETNONCLIENTMETRICS` 的值——也就是 `push` 下一次会原样写回去的那份。
pub fn chrome_needs_repair() -> bool {
    read_metrics().is_some_and(|metrics| metrics.chrome().is_broken())
}

/// 诊断：同一组 metrics 按另一个 DPI 取回来，比较 `lfHeight` 就知道 SPI 给的
/// 值是逻辑像素（会随 dpi 变）还是当前 DPI 的物理像素（不随 dpi 变）。
pub fn probe_for_dpi(dpi: u32) -> Option<(i32, i32, i32)> {
    let mut metrics = NonClientMetricsW::default();
    let ok = unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS,
            metrics.size,
            &mut metrics as *mut NonClientMetricsW as *mut c_void,
            0,
            dpi,
        )
    };
    (ok != 0).then_some((
        metrics.menu_font.height,
        metrics.menu_height,
        metrics.caption_height,
    ))
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
            Target::Caption => entry_of(&metrics.caption_font),
            Target::Icon => entry_of(&read_icon_font()?),
        };
        items.push((target.id().to_string(), entry));
    }
    Some(Snapshot {
        taken: crate::logging::timestamp(),
        items,
        chrome: Some(metrics.chrome()),
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
    apply_with(entries, None)
}

/// `apply` 的完整版：顺便把「装饰尺寸」写成 `chrome`（`None` 就是保持系统现值）。
///
/// 两条路都走同一个 `push`，所以“写坏过一次”的值不可能从还原这条路溜进来。
pub fn apply_with(entries: &[(Target, Entry)], chrome: Option<Chrome>) -> Result<(), String> {
    if entries.is_empty() {
        return Err("nothing to apply".to_string());
    }
    // 一次写到底：`entry.height` 已经就是**当前 DPI 下的物理像素**
    // （`height_for_pt` 按 `AppliedDPI` 换算），写什么读回来的就是什么。
    //
    // 这里曾经有一段“写一次、读回来、按实测比例再修一次”的校准循环，是个大坑：
    // 它假设 `SPI_GETNONCLIENTMETRICS` 读回的值会被系统按 DPI 除一遍，可那个缩放
    // 方向取决于**调用进程的 DPI 感知**（感知进程恒等、非感知进程才 ÷1.25）。
    // 应用进程被 winit 设成 PMv2，于是校准把缩放又抵消了一次——用户点「应用」
    // 只有日志在动，屏幕上什么都没变。
    push(entries, chrome)
}

/// 注册表里那六项「装饰尺寸」，换算成 `SPI_GET` 现在的量纲（当前 DPI 的像素）。
///
/// 注册表存的是 twips（1 px ≙ 15 twips，带负号）；`SPI_GET` 在感知进程里给的是
/// 当前 DPI 下的像素。两者差一个 `dpi/96`，不换算就会每写一次涨 25 %。
fn chrome_from_registry() -> Option<Chrome> {
    let dpi = system_dpi() as f32;
    let read = |name: &str| {
        crate::reg::get_string_in(crate::reg::Root::CurrentUser, WINDOW_METRICS_KEY, name)
            .and_then(|text| text.trim().parse::<i32>().ok())
            .map(|twips| ((twips.abs() as f32 / 15.0) * dpi / 96.0).round() as i32)
    };
    Some(Chrome {
        border: read("BorderWidth")?,
        scroll_width: read("ScrollWidth")?,
        scroll_height: read("ScrollHeight")?,
        caption_width: read("CaptionWidth")?,
        sm_caption_width: read("SmCaptionWidth")?,
        menu_width: read("MenuWidth")?,
    })
}

/// 系统 DPI（写进 `NONCLIENTMETRICS` 的那套长度用的就是它）。
///
/// 来源是注册表里的 `AppliedDPI`：`GetDpiForSystem()` 在本进程里恒返回 96，
/// 拿它换算会把 125% 屏幕上的字号一律算小 20%。
pub fn system_dpi() -> u32 {
    crate::reg::get_dword_in(
        crate::reg::Root::CurrentUser,
        WINDOW_METRICS_KEY,
        "AppliedDPI",
    )
    .filter(|dpi| (72..=480).contains(dpi))
    .unwrap_or(DEFAULT_DPI)
}

/// 真正写一次 `NONCLIENTMETRICS` + `ICONTITLELOGFONT`。
///
/// 行高像素原样回写、字号写进去就是物理像素——**这里不做任何整块缩放**。
/// 曾经把整块长度乘过 `AppliedDPI/96`，结果是每写一次边框和滚动条就胖一圈。
fn push(entries: &[(Target, Entry)], chrome: Option<Chrome>) -> Result<(), String> {
    let mut metrics = read_metrics().ok_or("SPI_GETNONCLIENTMETRICS failed")?;
    let mut icon_font = read_icon_font().ok_or("SPI_GETICONTITLELOGFONT failed")?;
    let dpi = system_dpi() as i32;

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
            // 标题栏：单改字体不改高度的话，字会被切掉；按字号比例缩放行高
            // （和菜单、小标题栏同一套办法），并且只在支持的版本上做。
            Target::Caption if caption_supported() => {
                let before = metrics.caption_font.height;
                write_entry(&mut metrics.caption_font, entry);
                metrics.caption_height =
                    rescale_band(metrics.caption_height, before, entry.height, dpi);
            }
            Target::Caption => {
                log::warn!(
                    "sysfont: caption font skipped, build {} does not allow it",
                    os_build()
                );
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
    // 标题栏高度必须塞得下标题栏的字，否则整条标题栏缩成几个像素、看着就像
    // “标题栏消失了”（实测坏状态是 `CaptionFont = -2`、`iCaptionHeight = 3px`）。
    // 这一项我们**不提供修改入口**，所以它坏了一定是别处写的：碰到就按
    // “跟状态栏字体一样 + 跟小标题栏一样高”修回来，并记一条 warn。
    if caption_is_broken(
        metrics.caption_font.height,
        metrics.caption_height,
        metrics.status_font.height,
    ) {
        log::warn!(
            "sysfont: caption metrics look broken (font {}, height {}), repairing to font {} / height {}",
            metrics.caption_font.height,
            metrics.caption_height,
            metrics.status_font.height,
            metrics.sm_caption_height
        );
        metrics.caption_font = metrics.status_font.clone();
        metrics.caption_height = metrics.sm_caption_height;
    }
    // 边框/滚动条/标题栏宽度：这些**不按 DPI 换算**，写什么读回什么。曾经被
    // “整块乘系数”连累过一次（边框 36 px、滚动条 39 px），所以先看有没有要还原的
    // 快照值，再看现值是不是已经不像人调的，最后一律让它们回到标准尺寸。
    // 「装饰尺寸」：SPI 是整块替换，这几项也得跟着写一遍。**但绝不能把
    // `SPI_GET` 读到的现值原样写回去**：读回来的这套数在感知进程里是“物理像素”，
    // 而注册表存的是“96 dpi 逻辑值”，写回去就等于每写一次乘 1.25（实测边框
    // 1 px → 15 → 20，滚动条 11 px → 14）。所以一律以**注册表**那份为准：
    // 有快照就用快照的（「还原上次」要能连它们一起还原），没有就按注册表换算。
    metrics.set_chrome(
        chrome
            .or_else(|| chrome_from_registry())
            .unwrap_or_else(|| metrics.chrome()),
    );
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
    apply_with(&entries, snapshot.chrome)
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

    /// 这些数字是从 `WinUser.h` 抄来的，抄错一个就是一桩事故：
    /// `0x0021` 不是图标字体而是 `SPI_SETMOUSEBUTTONSWAP`（见常量上的注释）。
    #[test]
    fn os_build_is_a_real_windows_build_number() {
        // 无头会话也拿得到（RtlGetVersion 不依赖窗口）。
        let build = super::os_build();
        assert!(build >= 7600, "build 号看着不对：{build}");
        // 判断规则本身：22621（Win11 22H2）是分界线。
        assert!(22621 < 22621 || build >= 22621 || super::caption_supported());
    }

    #[test]
    fn caption_support_matches_the_build_cutoff() {
        // 只能验规则，不能验本机（本机是 19044，支持）。
        assert_eq!(super::caption_supported(), super::os_build() < 22621);
    }

    #[test]
    fn spi_constants_are_what_we_think() {
        assert_eq!(SPI_GETNONCLIENTMETRICS, 0x0029);
        assert_eq!(SPI_SETNONCLIENTMETRICS, 0x002A);
        assert_eq!(SPI_GETICONTITLELOGFONT, 0x001F);
        assert_eq!(SPI_SETICONTITLELOGFONT, 0x0022);
        // 邻居的值写在旁边，是为了让"抄错一行"这件事在下一次也看得见：
        // 0x0020 = SPI_GETMOUSEBUTTONSWAP、0x0021 = SPI_SETMOUSEBUTTONSWAP。
        assert_ne!(
            SPI_SETICONTITLELOGFONT, 0x0021,
            "0x0021 是 SPI_SETMOUSEBUTTONSWAP"
        );
    }

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
