//! 用户资源目录：外部主题、皮肤。
//!
//! 一条原则：**坏文件不能拖垮程序**。主题/皮肤都是用户手写或从网上抄来的，
//! 语法错、色值错、版本高都是常态；这里逐个 `try`，坏的记一行 warn 后跳过，
//! 界面上只是少一个选项，而不是打不开。

use std::fs;
use std::path::PathBuf;

use zouwu_core::skin::{is_unsafe_relative, SkinManifest, PROCEDURAL_SKIN_ID};
use zouwu_core::theme::Theme;

/// 单张图层 PNG 的字节上限。
///
/// 解码前的第一道闸：尺寸闸在解码后（见 `zouwu_ui::skin::MAX_LAYER_SIDE`），
/// 但那时内存已经吃掉了。8 MB 足够一张 2048² 的插画（压缩后通常只有几百 KB）。
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

/// 一份**还没解码**的皮肤：清单 + 原始 PNG 字节。
///
/// 为什么在这里只到字节为止：解码需要 tiny-skia（渲染层），而资源层刻意不依赖
/// 渲染。这样"读盘/校验"和"解码/绘制"可以各自测试，坏图也不会把资源层拖下水。
#[derive(Clone, Debug)]
pub struct SkinPackage {
    pub manifest: SkinManifest,
    /// 皮肤目录（报错信息里指回用户自己的文件）。
    pub dir: PathBuf,
    /// `(清单里写的相对路径, 文件字节)`，顺序与 `manifest.layers` 一致。
    pub images: Vec<(String, Vec<u8>)>,
}

/// 读 `skins/<id>/skin.toml` 与它引用的所有 PNG。
///
/// 失败一律返回带上下文的字符串：皮肤是从网上抄来的东西，出问题时用户需要知道
/// 是哪一行/哪一张图坏了，而不是"加载失败"。
pub fn load_skin(id: &str) -> Result<SkinPackage, String> {
    if id == PROCEDURAL_SKIN_ID {
        return Err("procedural skin has no files on disk".into());
    }
    if id.trim().is_empty() || id.contains(['/', '\\', ':']) || id.contains("..") {
        return Err(format!("skin id `{id}` is not a plain directory name"));
    }
    let dir = skins_dir().join(id);
    let manifest_path = dir.join("skin.toml");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest = SkinManifest::parse(&text)?;
    if manifest.id != id {
        return Err(format!(
            "skin.toml in `{id}` declares id `{}` — folder and id must match",
            manifest.id
        ));
    }

    let mut images = Vec::with_capacity(manifest.layers.len());
    for layer in &manifest.layers {
        if is_unsafe_relative(&layer.image) {
            return Err(format!("layer `{}` escapes the skin folder", layer.image));
        }
        let path = dir.join(&layer.image);
        let meta = fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.len() > MAX_IMAGE_BYTES {
            return Err(format!(
                "{} is {} bytes, over the {MAX_IMAGE_BYTES} byte limit",
                path.display(),
                meta.len()
            ));
        }
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        images.push((layer.image.clone(), bytes));
    }

    Ok(SkinPackage {
        manifest,
        dir,
        images,
    })
}

/// `%APPDATA%\zouwu\themes`
pub fn themes_dir() -> PathBuf {
    crate::config_dir().join("themes")
}

/// `%APPDATA%\zouwu\skins`
pub fn skins_dir() -> PathBuf {
    crate::config_dir().join("skins")
}

/// `%APPDATA%\zouwu\exports` —— 导出皮肤包的默认目录。
///
/// 不用系统下载目录：那里什么都有，多一个 `.zwskin` 谁也不会注意；
/// 放在自己的目录里，`open_config_dir` 一点就能找到。
pub fn exports_dir() -> PathBuf {
    crate::config_dir().join("exports")
}

/// 在资源管理器里打开 `skins/`。目录不存在时先建一个空的。
pub fn reveal_skins_dir() {
    let dir = skins_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        log::warn!("cannot create {}: {err}", dir.display());
        return;
    }
    #[cfg(target_os = "windows")]
    crate::autostart::open_path(&dir);
    #[cfg(not(target_os = "windows"))]
    log::info!("skins dir: {}", dir.display());
}

/// 随程序发布的官方皮肤（内嵌进二进制）。
///
/// 为什么内嵌而不直接读 `assets/`：发布版是单个 exe，没有 `assets` 目录；
/// 但内嵌后又要能被用户改，所以**首次启动时落到磁盘**，之后就以磁盘为准。
/// 删掉会被下次启动补回来（自愈），但要改配色/换图，改磁盘上那份就够了。
struct BuiltinSkin {
    id: &'static str,
    manifest: &'static str,
    layers: &'static [(&'static str, &'static [u8])],
}

const BUILTIN_SKINS: &[BuiltinSkin] = &[BuiltinSkin {
    id: "zouwu-ink",
    manifest: include_str!("../../../assets/skins/zouwu-ink/skin.toml"),
    layers: &[
        (
            "body.png",
            include_bytes!("../../../assets/skins/zouwu-ink/body.png"),
        ),
        (
            "blink.png",
            include_bytes!("../../../assets/skins/zouwu-ink/blink.png"),
        ),
        (
            "seal.png",
            include_bytes!("../../../assets/skins/zouwu-ink/seal.png"),
        ),
    ],
}];

/// 官方皮肤 id 列表（托盘里可以标出来，未来也可做"修复"按钮）。
pub fn builtin_skin_ids() -> Vec<&'static str> {
    BUILTIN_SKINS.iter().map(|s| s.id).collect()
}

/// 内置素材的版本戳。
///
/// **只要改了 `assets/` 下内置皮肤的字节（哪怕是同一版程序），就要把它 +1。**
/// 否则老用户机器上那份永远不会被替换——我们改了构图，他们看到的还是旧的那只。
/// 这条规则是踩出来的：水墨驺吾重新居中之后，本机的 `%APPDATA%` 里仍是旧图，
/// 看起来像"根本没修好"。
pub const BUILTIN_ASSET_REVISION: u32 = 3;

/// 版本戳文件名（写在每个内置皮肤目录里）。
const STAMP_FILE: &str = ".zouwu-builtin";

/// 把官方皮肤写到 `skins/`，返回这次实际写入了几个。
///
/// 幂等有两个含义：
/// - 已经有**同一版**的：一律不动（用户在改我们发布的皮肤，不能被启动覆盖）；
/// - 是旧版的（版本戳不同）：整目录重建。用户对内置皮肤的手改会丢，
///   但"更新之后看到的还是旧图"更难解释——导入的皮肤有自己的 id，不受影响。
pub fn ensure_builtin_skins() -> usize {
    let root = skins_dir();
    let mut installed = 0;
    for skin in BUILTIN_SKINS {
        let dir = root.join(skin.id);
        let up_to_date = fs::read_to_string(dir.join(STAMP_FILE))
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .is_some_and(|rev| rev == BUILTIN_ASSET_REVISION);
        if dir.is_dir() && up_to_date {
            continue;
        }
        if dir.is_dir() {
            // 旧版本（或没有戳的历史安装）：整目录重建，避免残留旧图层文件。
            if let Err(err) = fs::remove_dir_all(&dir) {
                log::warn!("cannot refresh {}: {err}", dir.display());
                continue;
            }
        }
        if let Err(err) = fs::create_dir_all(&dir) {
            log::warn!("cannot create {}: {err}", dir.display());
            continue;
        }
        let mut ok = true;
        for (name, bytes) in skin.layers {
            let path = dir.join(name);
            if let Err(err) = fs::write(&path, bytes) {
                log::warn!("cannot write {}: {err}", path.display());
                ok = false;
            }
        }
        let manifest = dir.join("skin.toml");
        if let Err(err) = fs::write(&manifest, skin.manifest) {
            log::warn!("cannot write {}: {err}", manifest.display());
            ok = false;
        }
        if ok {
            if let Err(err) = fs::write(dir.join(STAMP_FILE), BUILTIN_ASSET_REVISION.to_string()) {
                log::warn!("cannot write stamp in {}: {err}", dir.display());
                ok = false;
            }
        }
        if ok {
            installed += 1;
            log::info!(
                "installed built-in skin `{}` into {}",
                skin.id,
                dir.display()
            );
        }
    }
    installed
}

/// 全部可用主题：内置在前，外部在后（同名 id 时外部覆盖内置）。
///
/// 外部主题按 `*.toml` 文件名排序，保证每次启动菜单顺序一致——
/// 设置面板里选项跳来跳去是很难用的一件事。
pub fn load_themes() -> Vec<Theme> {
    let mut out: Vec<Theme> = Theme::builtin_ids()
        .into_iter()
        .map(|(id, _)| Theme::by_name(id))
        .collect();

    let dir = themes_dir();
    let mut files: Vec<PathBuf> = match fs::read_dir(&dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();

    for path in files {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                log::warn!("theme {} unreadable: {err}", path.display());
                continue;
            }
        };
        match Theme::from_toml(&text) {
            Ok(theme) => {
                if let Some(slot) = out.iter_mut().find(|t| t.id == theme.id) {
                    *slot = theme;
                } else {
                    out.push(theme);
                }
            }
            Err(err) => log::warn!("theme {} rejected: {err}", path.display()),
        }
    }
    out
}

/// 托盘/设置面板里一个主题选项：`id` 用来落盘，`name` 用来给人看。
///
/// 为什么单独一个类型而不是直接传 `&Theme`：托盘只关心"id + 显示名"，
/// 让它持有整份调色板既浪费，也会把 UI 层和渲染类型绑在一起。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeEntry {
    pub id: String,
    pub name: String,
}

impl From<&Theme> for ThemeEntry {
    fn from(theme: &Theme) -> Self {
        Self {
            id: theme.id.clone(),
            name: theme.name.clone(),
        }
    }
}

/// 托盘/设置面板里一个皮肤选项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkinEntry {
    pub id: String,
    pub name: String,
}

/// 全部可用皮肤：`procedural` 永远排第一（兜底），其后是磁盘上的 `skins/*`。
///
/// 清单读不出来时用目录名顶上：**坏掉的皮肤也要能在菜单里被看见**，
/// 否则用户只会觉得"我放进去了但没出现"，连删都不知道删哪个。
pub fn skin_entries() -> Vec<SkinEntry> {
    let mut out = vec![SkinEntry {
        id: PROCEDURAL_SKIN_ID.to_string(),
        name: SkinManifest::procedural().display_name().to_string(),
    }];
    for id in skin_ids() {
        let name = fs::read_to_string(skins_dir().join(&id).join("skin.toml"))
            .ok()
            .and_then(|text| SkinManifest::parse(&text).ok())
            .map(|m| m.display_name().to_string())
            .unwrap_or_else(|| id.clone());
        out.push(SkinEntry { id, name });
    }
    out
}

/// 按 id 取主题（找不到 → 兜底主题）。
pub fn theme_by_id(themes: &[Theme], id: &str) -> Theme {
    themes
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .unwrap_or_else(Theme::fallback)
}

/// 把内置主题导出成 `themes/<id>.toml` 模板，给"复制一份改改"用。
///
/// 只在该文件不存在时写：用户改过的文件不能被我们覆盖。
pub fn export_theme_template(id: &str) -> Option<PathBuf> {
    let theme = Theme::builtin(id)?;
    let dir = themes_dir();
    let path = dir.join(format!("{id}.example.toml"));
    if path.exists() {
        return Some(path);
    }
    if let Err(err) = fs::create_dir_all(&dir) {
        log::warn!("cannot create themes dir: {err}");
        return None;
    }
    match fs::write(&path, theme.to_toml()) {
        Ok(()) => Some(path),
        Err(err) => {
            log::warn!("cannot write theme template: {err}");
            None
        }
    }
}

/// 皮肤目录列表（`skins/<id>/skin.toml` 的父目录顺序，供设置面板展示）。
pub fn skin_ids() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(skins_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join("skin.toml").is_file() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    out.push(name.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// 皮肤菜单项：`(id, 展示名)`，`procedural` 恒定排第一。
///
/// 展示名优先用 `skin.toml` 里的 `name`，没写就用目录名；
/// 内置的「代码绘制」这个说法要翻译，所以走 i18n。
pub fn skin_choices(i18n: &zouwu_core::i18n::I18n) -> Vec<(String, String)> {
    let mut out = vec![(
        PROCEDURAL_SKIN_ID.to_string(),
        i18n.t("settings.skin.procedural"),
    )];
    for id in skin_ids() {
        // 扫目录时已经确认过 `skin.toml` 存在；读坏了就用目录名兜底。
        let name = fs::read_to_string(skins_dir().join(&id).join("skin.toml"))
            .ok()
            .and_then(|text| SkinManifest::parse(&text).ok())
            .and_then(|manifest| manifest.name)
            .unwrap_or_else(|| id.clone());
        out.push((id, name));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_lookup_falls_back() {
        let themes = load_themes();
        assert!(themes.iter().any(|t| t.id == "zouwu-warm"));
        assert_eq!(theme_by_id(&themes, "nope").id, "zouwu-warm");
    }
}
