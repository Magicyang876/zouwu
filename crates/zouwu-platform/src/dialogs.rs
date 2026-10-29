//! 系统对话框：选文件、另存为、说一句人话。
//!
//! **为什么允许阻塞主线程**：这三个对话框都是**模态**的——用户点开之后，
//! 除了回应它什么也做不了，此时继续跑渲染循环没有意义。rfd 的同步 API
//! 内部会自己 pump 系统消息，所以窗口不会假死。
//!
//! **为什么错误要用弹窗而不是只写日志**：zouwu 是常驻托盘的小工具，
//! 用户不会为了"导入失败"去翻日志文件。失败必须在他眼前说清楚哪一步错了。

use std::path::PathBuf;

/// 选一个程序文件（`.exe` 或任何快捷方式），用来新增一颗“打开它”的球。
pub fn pick_program() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("zouwu — choose a program")
        .add_filter("programs", &["exe", "com", "bat", "cmd", "lnk", "ps1"])
        .add_filter("all files", &["*"])
        .pick_file()
}

/// 选一张 PNG 当气泡球的图标。
pub fn pick_icon_image() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("zouwu — choose a bubble icon")
        .add_filter("png image", &["png"])
        .pick_file()
}

/// 选一个 `.zwskin` 皮肤包。
pub fn pick_skin_package() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("zouwu — open skin package")
        .add_filter("zouwu skin package", &["zwskin"])
        .add_filter("all files", &["*"])
        .pick_file()
}

/// 选一个字体文件（`.ttf` / `.otf` / `.ttc`）。
pub fn pick_font_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("zouwu — open font file")
        .add_filter("font", &["ttf", "otf", "ttc"])
        .add_filter("all files", &["*"])
        .pick_file()
}

/// 「另存为」皮肤包。`default_name` 建议是 `<皮肤 id>.zwskin`。
pub fn pick_export_path(default_name: &str) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title("zouwu — export skin package")
        .add_filter("zouwu skin package", &["zwskin"])
        .set_file_name(default_name);
    if let Some(dir) = super::assets::exports_dir().to_str() {
        // 目录不存在时 `set_directory` 会被系统忽略，所以先建。
        let _ = std::fs::create_dir_all(super::assets::exports_dir());
        dialog = dialog.set_directory(dir);
    }
    dialog.save_file()
}

/// 一个是/否框。默认"否"（MB_DEFBUTTON2）：危险操作不该被回车顺手确认掉。
pub fn confirm(title: &str, text: &str) -> bool {
    log::info!("confirm: {title} — {text}");
    message_box_ex(title, text, true)
}

/// 一个警告框。失败路径专用，不要拿它做日常提示。
pub fn warn(title: &str, text: &str) {
    log::warn!("{title}: {text}");
    message_box_ex(title, text, false);
}

#[cfg(target_os = "windows")]
fn message_box_ex(title: &str, text: &str, question: bool) -> bool {
    use std::ffi::CString;

    const MB_OK: u32 = 0x0000_0000;
    const MB_ICONWARNING: u32 = 0x0000_0030;
    const MB_SETFOREGROUND: u32 = 0x0001_0000;
    const MB_YESNO: u32 = 0x0000_0004;
    const MB_DEFBUTTON2: u32 = 0x0000_0100;
    const IDYES: i32 = 6;
    // 顶层窗口是分层窗口 + NOACTIVATE，正常弹窗会被别的前台窗口压住。
    const MB_TOPMOST: u32 = 0x0004_0000;

    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, ty: u32) -> i32;
    }

    let Ok(title) = CString::new(title.replace('\0', "")) else {
        return false;
    };
    let Ok(text) = CString::new(text.replace('\0', "")) else {
        return false;
    };
    let wide = |s: &CString| -> Vec<u16> {
        s.to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    };
    let (t, b) = (wide(&text), wide(&title));
    let flags = if question {
        MB_YESNO | MB_DEFBUTTON2
    } else {
        MB_OK | MB_ICONWARNING
    } | MB_SETFOREGROUND
        | MB_TOPMOST;
    let ret = unsafe { MessageBoxW(0, t.as_ptr(), b.as_ptr(), flags) };
    ret == IDYES
}

#[cfg(not(target_os = "windows"))]
fn message_box_ex(_title: &str, _text: &str, question: bool) -> bool {
    // 非 Windows 后端还没接：不做假设，一律当"否"。
    let _ = question;
    false
}
