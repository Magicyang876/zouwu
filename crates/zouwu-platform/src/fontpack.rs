//! 字体包：用户自备的字体（`%APPDATA%\zouwu\fonts\<id>\`）。
//!
//! 与皮肤包同构：一个目录 + 一个清单 + 若干素材文件，只存数据不存代码。
//!
//! 三个设计决定：
//!
//! 1. **家族名从字体文件里读，不让用户填**（见 `fontfile`）。让用户填"微软雅黑"
//!    这种自由文本，写错一个字节就静默回退到系统字体，且没人看得出为什么。
//! 2. **注册用 `AddFontResourceEx(FR_PRIVATE)`，不写注册表、不装进系统**。
//!    装进系统需要管理员权限，而且用户删掉字体包时我们还得负责清理 ——
//!    进程私有注册，进程退出自动消失，零残留。
//! 3. **清单里存 `family`**，即使字体文件后来被换掉，界面显示的还是用户当时看到的名字；
//!    重新注册时以文件为准（`fontfile` 再读一次）。
//!
//! 一个包可以含多个文件（比如同一家族的 Regular + Bold，或一个 `.ttc` 里多张脸）。
//! 全部注册后由系统按"家族名 + 粗体"自行挑最合适的那张。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 单个字体文件的上限。CJK 字体（尤其带全套字重的 `.ttc`）常见 20 MB 上下，
/// 32 MB 够用；再大基本是用户选错了文件（比如整个字体目录打包）。
pub const MAX_FONT_BYTES: u64 = 32 * 1024 * 1024;

pub const FONT_SCHEMA_VERSION: u32 = 1;

/// 探中文覆盖用的代表字：拉丁、汉字、全角标点。
const CJK_PROBE: [char; 3] = ['A', '永', '，'];

/// 允许的扩展名（小写比较）。
pub const FONT_EXTENSIONS: [&str; 3] = ["ttf", "otf", "ttc"];

/// 字体包清单：`fonts/<id>/font.toml`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontPack {
    #[serde(default = "default_schema")]
    pub schema_version: u32,
    pub id: String,
    /// 界面显示名（默认等于 `family`）。
    #[serde(default)]
    pub name: String,
    /// 交给系统的家族名，例如 `Consolas`、`Microsoft YaHei`。
    pub family: String,
    /// 相对 `fonts/<id>/` 的文件名列表。
    pub files: Vec<String>,
    /// 这份字体字否自带中文字形（导入时探的，老清单里没这个字段就当 `true`）。
    ///
    /// `false` 时界面仍然能显示中文，但走的是系统 font-link 回落：
    /// 字形是宋体点阵、宽度却按你选的字体算，看上去会像"字被压扁"。
    /// 面板里会提醒用户这个后果，而不是背地里换外观。
    #[serde(default = "default_cjk")]
    pub has_cjk: bool,
}

fn default_cjk() -> bool {
    true
}

fn default_schema() -> u32 {
    FONT_SCHEMA_VERSION
}

impl FontPack {
    /// 显示名：清单里写了就用，否则退回家族名。
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.family
        } else {
            &self.name
        }
    }

    fn to_toml(&self) -> String {
        // 手写而不是 toml::to_string：字段顺序固定，用户手改时好读。
        let files = self
            .files
            .iter()
            .map(|f| format!("{f:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "schema_version = {}\nid = {:?}\nname = {:?}\nfamily = {:?}\nhas_cjk = {}\nfiles = [{}]\n",
            self.schema_version, self.id, self.name, self.family, self.has_cjk, files
        )
    }

    fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str::<FontPack>(text).map_err(|e| e.to_string())
    }
}

/// `%APPDATA%\zouwu\fonts`
pub fn fonts_dir() -> PathBuf {
    crate::config_dir().join("fonts")
}

/// 扫出全部可用的字体包（按显示名排序；坏掉的清单跳过并记日志）。
pub fn list() -> Vec<FontPack> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(fonts_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest = path.join("font.toml");
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        match FontPack::from_toml(&text) {
            Ok(pack) => {
                if pack.files.is_empty() || pack.family.trim().is_empty() {
                    log::warn!("font pack {} has no usable files, ignoring", path.display());
                    continue;
                }
                out.push(pack);
            }
            Err(err) => log::warn!("bad font pack {}: {err}", manifest.display()),
        }
    }
    out.sort_by(|a, b| a.display_name().cmp(b.display_name()));
    out
}

pub fn get(id: &str) -> Option<FontPack> {
    list().into_iter().find(|p| p.id == id)
}

/// 导入一个字体文件：读家族名 → 落盘 → 写清单。
///
/// 同名家族重复导入时**原地替换**（用户改了字体文件再导一次，不该攒出一堆同名包）。
pub fn import(src: &Path) -> Result<FontPack, String> {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !FONT_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "unsupported font type `.{ext}` (expected one of {})",
            FONT_EXTENSIONS.join(", ")
        ));
    }
    let meta = fs::metadata(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", src.display()));
    }
    if meta.len() > MAX_FONT_BYTES {
        return Err(format!(
            "font is {:.1} MB, over the {} MB limit",
            meta.len() as f64 / (1024.0 * 1024.0),
            MAX_FONT_BYTES / (1024 * 1024)
        ));
    }
    let bytes = fs::read(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    let family = crate::fontfile::family_name(&bytes)?;
    let id = pack_id(&family);
    // 探中文覆盖：缺字时界面不会崩，但中文会回落到系统字体。
    // 这里只记日志 + 写进清单，由界面去提醒，**不拒绝导入** ——
    // 用户可能就是要用 Consolas 看拉丁界面。
    let has_cjk = match crate::fontfile::missing_chars(&bytes, &CJK_PROBE) {
        Ok(missing) if !missing.is_empty() => {
            log::warn!(
                "font `{family}` has no glyph for {:?}; Chinese text will fall back to the system font",
                missing
            );
            false
        }
        Ok(_) => true,
        // cmap 读不出来（少见的子集字体）：不知道就别吓人。
        Err(err) => {
            log::warn!("cannot check font coverage for `{family}`: {err}");
            true
        }
    };

    let dir = fonts_dir().join(&id);
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let file_name = format!("{id}.{ext}");
    fs::write(dir.join(&file_name), &bytes)
        .map_err(|e| format!("cannot write {}: {e}", dir.join(&file_name).display()))?;

    // 库里遗留的其它文件（换过扩展名/字重）会和新文件同时注册，导致家族里出现
    // 两张不匹配的脸。清单只留当前文件，但**不删**旧文件 —— 那是用户的素材，
    // 删了不可逆；注册时只按清单走，所以不影响结果。
    let pack = FontPack {
        schema_version: FONT_SCHEMA_VERSION,
        id,
        name: family.clone(),
        family,
        files: vec![file_name],
        has_cjk,
    };
    fs::write(dir.join("font.toml"), pack.to_toml())
        .map_err(|e| format!("cannot write manifest: {e}"))?;
    log::info!(
        "font pack imported: {} ({})",
        pack.display_name(),
        dir.display()
    );
    Ok(pack)
}

/// 把家族名变成一个安全的目录名。
///
/// 只保留 ASCII 字母数字与 `-`：家族名里有中文/空格/斜杠都会出问题
/// （中文目录在压缩包、注册表、日志里都是麻烦），而 id 只要在机器上唯一即可。
pub fn pack_id(family: &str) -> String {
    let mut id = String::with_capacity(family.len());
    for ch in family.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            id.push(ch);
        } else if (ch == '-' || ch == '_' || ch.is_whitespace())
            && !id.ends_with('-')
            && !id.is_empty()
        {
            id.push('-');
        }
    }
    let id = id.trim_matches('-').to_string();
    if id.is_empty() {
        "font".to_string()
    } else {
        id
    }
}

pub fn delete(id: &str) -> Result<(), String> {
    if id.trim().is_empty() || id.contains(['/', '\\', ':']) || id.contains("..") {
        return Err(format!("bad font pack id `{id}`"));
    }
    let dir = fonts_dir().join(id);
    if !dir.is_dir() {
        return Err(format!("font pack `{id}` is not installed"));
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("cannot delete {}: {e}", dir.display()))?;
    log::info!("font pack deleted: {id}");
    Ok(())
}

/// 启动时把所有包的字体文件注册进本进程，返回注册成功的文件数。
///
/// 必须在任何文本绘制之前调用（设置面板、气泡提示都要用）。
pub fn register_all() -> usize {
    let mut count = 0;
    for pack in list() {
        let dir = fonts_dir().join(&pack.id);
        let mut ok = false;
        for file in &pack.files {
            if zouwu_core::skin::is_unsafe_relative(file) {
                log::warn!("font pack `{}` has unsafe file path `{file}`", pack.id);
                continue;
            }
            let path = dir.join(file);
            if crate::text_gdi::add_font_file(&path) {
                ok = true;
                count += 1;
            }
        }
        if ok {
            log::info!("font pack ready: {} ({})", pack.display_name(), pack.family);
        } else {
            log::warn!("font pack `{}` could not be registered", pack.id);
        }
    }
    count
}

/// 在资源管理器里打开 `fonts/`。
pub fn reveal_dir() {
    let dir = fonts_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        log::warn!("cannot create {}: {err}", dir.display());
        return;
    }
    #[cfg(target_os = "windows")]
    crate::autostart::open_path(&dir);
    #[cfg(not(target_os = "windows"))]
    log::info!("fonts dir: {}", dir.display());
}

/// 设置面板的选择项：`("" , 系统默认)` + 每个包。
pub fn choices(i18n: &zouwu_core::i18n::I18n) -> Vec<(String, String)> {
    let mut out = vec![(String::new(), i18n.t("settings.font.system"))];
    for pack in list() {
        out.push((pack.id.clone(), pack.display_name().to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_id_is_safe_for_any_family_name() {
        assert_eq!(pack_id("Consolas"), "consolas");
        assert_eq!(pack_id("Microsoft YaHei UI"), "microsoft-yahei-ui");
        assert_eq!(pack_id("思源黑体 / Source Han"), "source-han");
        assert_eq!(pack_id("///"), "font");
        assert_eq!(pack_id("  "), "font");
        // 结果必须能直接当目录名用
        for name in ["A B", "x/y", "a\\b", "..", "C:", "汉体"] {
            let id = pack_id(name);
            assert!(!id.contains(['/', '\\', ':', '.']), "{name} -> {id}");
        }
    }

    #[test]
    fn manifest_roundtrips() {
        let pack = FontPack {
            schema_version: FONT_SCHEMA_VERSION,
            id: "consolas".into(),
            name: "Consolas".into(),
            family: "Consolas".into(),
            files: vec!["consolas.ttf".into(), "consolasb.ttf".into()],
            has_cjk: false,
        };
        let back = FontPack::from_toml(&pack.to_toml()).expect("parse");
        assert_eq!(back, pack);
        assert_eq!(back.display_name(), "Consolas");
    }

    #[test]
    fn empty_name_falls_back_to_family() {
        let mut pack = FontPack {
            schema_version: FONT_SCHEMA_VERSION,
            id: "x".into(),
            name: "  ".into(),
            family: "Noto Sans SC".into(),
            files: vec!["a.otf".into()],
            has_cjk: true,
        };
        assert_eq!(pack.display_name(), "Noto Sans SC");
        pack.family = "F".into();
        assert_eq!(pack.display_name(), "F");
    }
}
