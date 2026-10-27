//! `.zwskin` 皮肤包：**单文件**导入导出。
//!
//! 为什么不用 zip：皮肤包里只有「一个 toml + 几张 png」，为它引一个 zip 解码器
//! （以及 deflate、crc32、可能的 unsafe）不值得。这里定一个 20 行能读完的容器：
//!
//! ```text
//! "ZWPK"            4   magic
//! version           2   u16 LE（当前 1）
//! file_count        2   u16 LE
//! 每项：
//!   name_len        2   u16 LE
//!   name            ..  UTF-8，**只能是纯文件名**
//!   data_len        4   u32 LE
//!   data            ..  原始字节（PNG 已经是压缩格式，再压一遍没意义）
//! ```
//!
//! 定这个格式时踩过的三个坑，都写成了校验：
//!
//! 1. **名字必须是纯文件名**。`../../.ssh/authorized_keys` 这种"包内路径"
//!    是压缩包最经典的攻击面，而皮肤包只需要平铺的几个文件——不需要目录。
//! 2. **先看长度再看内容**。`data_len` 声称 4 GB 而实际只有 100 字节时，
//!    不能按声明长度去分配内存。
//! 3. **清单必须自洽**：`skin.toml` 里列了但包里没有的图层 = 坏包，直接拒。

use std::fs;
use std::path::{Path, PathBuf};

use crate::assets::{self, SkinPackage, MAX_IMAGE_BYTES};

/// 文件头魔数。选 4 字节 ASCII，`file` 命令和十六进制编辑器里一眼能认出来。
pub const MAGIC: &[u8; 4] = b"ZWPK";
pub const FORMAT_VERSION: u16 = 1;
/// 单包文件数上限（皮肤就是一两个图层，64 已经非常宽松）。
pub const MAX_FILES: usize = 64;
/// 解包后总字节上限，20 MB。
pub const MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;
/// 包内清单的固定文件名。
pub const MANIFEST_NAME: &str = "skin.toml";

const HEADER_BYTES: usize = 4 + 2 + 2;

#[derive(Debug)]
pub struct PackageFiles {
    pub manifest_toml: String,
    pub images: Vec<(String, Vec<u8>)>,
}

/// 打包：`skin.toml` + 图层字节 → `.zwskin` 内容。
pub fn encode(manifest_toml: &str, images: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let count = images.len() + 1;
    if count > MAX_FILES {
        return Err(format!("too many files: {count} > {MAX_FILES}"));
    }
    let mut out = Vec::with_capacity(
        HEADER_BYTES
            + manifest_toml.len()
            + images.iter().map(|(_, d)| d.len() + 32).sum::<usize>(),
    );
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(count as u16).to_le_bytes());

    push_entry(&mut out, MANIFEST_NAME, manifest_toml.as_bytes())?;
    for (name, data) in images {
        push_entry(&mut out, name, data)?;
    }
    Ok(out)
}

fn push_entry(out: &mut Vec<u8>, name: &str, data: &[u8]) -> Result<(), String> {
    if !is_plain_name(name) {
        return Err(format!("refusing to pack unsafe file name `{name}`"));
    }
    let name_len = u16::try_from(name.len()).map_err(|_| format!("file name too long: {name}"))?;
    let data_len = u32::try_from(data.len()).map_err(|_| format!("file too big: {name}"))?;
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(data);
    Ok(())
}

/// 解包。所有长度都先与"剩余字节数"比对，再决定要不要读。
pub fn decode(bytes: &[u8]) -> Result<PackageFiles, String> {
    if bytes.len() < HEADER_BYTES {
        return Err("not a skin package: too short".into());
    }
    if &bytes[0..4] != MAGIC {
        return Err("not a skin package: bad magic".into());
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != FORMAT_VERSION {
        return Err(format!(
            "unsupported skin package version {version} (this build reads v{FORMAT_VERSION})"
        ));
    }
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    if count == 0 || count > MAX_FILES {
        return Err(format!("bad file count {count}"));
    }

    let mut pos = HEADER_BYTES;
    let mut total = 0usize;
    let mut manifest_toml = None;
    let mut images: Vec<(String, Vec<u8>)> = Vec::with_capacity(count);

    for _ in 0..count {
        // 每个 entry 至少要有 2+4 字节的头部
        if pos + 6 > bytes.len() {
            return Err("truncated skin package: entry header".into());
        }
        let name_len = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2;
        if name_len == 0 || pos + name_len + 4 > bytes.len() {
            return Err("truncated skin package: file name".into());
        }
        let name = std::str::from_utf8(&bytes[pos..pos + name_len])
            .map_err(|_| "skin package: file name is not utf-8".to_string())?
            .to_string();
        pos += name_len;
        if !is_plain_name(&name) {
            return Err(format!("skin package contains unsafe path `{name}`"));
        }

        let data_len =
            u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                as usize;
        pos += 4;
        if data_len > MAX_IMAGE_BYTES as usize {
            return Err(format!(
                "`{name}` declares {data_len} bytes (limit {MAX_IMAGE_BYTES})"
            ));
        }
        if pos + data_len > bytes.len() {
            return Err(format!(
                "truncated skin package: `{name}` declares {data_len} bytes, {} left",
                bytes.len() - pos
            ));
        }
        total += data_len;
        if total > MAX_TOTAL_BYTES {
            return Err(format!("skin package exceeds {MAX_TOTAL_BYTES} bytes"));
        }

        let data = bytes[pos..pos + data_len].to_vec();
        pos += data_len;

        if name == MANIFEST_NAME {
            let text =
                String::from_utf8(data).map_err(|_| "skin.toml is not valid utf-8".to_string())?;
            manifest_toml = Some(text);
        } else if images.iter().any(|(existing, _)| *existing == name) {
            return Err(format!("duplicate entry `{name}`"));
        } else {
            images.push((name, data));
        }
    }

    let manifest_toml =
        manifest_toml.ok_or_else(|| format!("skin package has no `{MANIFEST_NAME}`"))?;
    Ok(PackageFiles {
        manifest_toml,
        images,
    })
}

/// 纯文件名：不含路径分隔符、不指向上级、不是绝对路径、不是 Windows 保留名。
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.starts_with('.')
        && !name.contains("..")
        // Windows 会把 "CON.png" / "aux" 之类当设备名
        && !matches!(
            name.split('.').next().unwrap_or_default().to_ascii_lowercase().as_str(),
            "con" | "prn" | "aux" | "nul"
        )
}

/// 导出：把已安装皮肤打成 `.zwskin`。
///
/// 默认落在 `<config>/exports/<id>.zwskin`，返回最终路径。
pub fn export(id: &str) -> Result<PathBuf, String> {
    let dir = assets::exports_dir();
    fs::create_dir_all(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;
    let path = dir.join(format!("{}.zwskin", id));
    export_to(id, &path)?;
    Ok(path)
}

/// 导出到指定路径（`另存为` 用）。
pub fn export_to(id: &str, path: &Path) -> Result<(), String> {
    let package = assets::load_skin(id)?;
    let manifest_toml = package.manifest.to_toml();
    let bytes = encode(&manifest_toml, &package.images)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("create {}: {err}", parent.display()))?;
        }
    }
    fs::write(path, &bytes).map_err(|err| format!("write {}: {err}", path.display()))?;
    Ok(())
}

/// 导入：解包 → 校验清单 → 落盘到 `skins/<id>/`。
///
/// 目标目录已存在时**不覆盖**：皮肤可能正在被用，静默覆盖会让人以为"导入了但没变"。
/// 想升级就先在托盘里删掉/改名，或者用 `--force`（未来设置面板里给）。
pub fn import(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let files = decode(&bytes)?;

    let manifest = zouwu_core::skin::SkinManifest::parse(&files.manifest_toml)?;
    if manifest.id == zouwu_core::skin::PROCEDURAL_SKIN_ID {
        return Err(format!(
            "`{}` is reserved for the built-in procedural skin",
            zouwu_core::skin::PROCEDURAL_SKIN_ID
        ));
    }
    if !is_plain_name(&manifest.id) {
        return Err(format!(
            "skin id `{}` is not a safe folder name",
            manifest.id
        ));
    }

    // 清单里声明的每个图层都必须真的在包里；多余的图不管（作者可能塞了备用图）。
    for layer in &manifest.layers {
        if !files.images.iter().any(|(name, _)| *name == layer.image) {
            return Err(format!(
                "skin `{}` references `{}` but the package does not contain it",
                manifest.id, layer.image
            ));
        }
    }

    let dir = assets::skins_dir().join(&manifest.id);
    if dir.exists() {
        return Err(format!(
            "skin `{}` already installed ({})",
            manifest.id,
            dir.display()
        ));
    }
    fs::create_dir_all(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;

    // 只写清单声明的图层（白名单），包里多余的图不落地。
    for layer in &manifest.layers {
        let Some((_, data)) = files.images.iter().find(|(name, _)| *name == layer.image) else {
            continue;
        };
        let target = dir.join(&layer.image);
        fs::write(&target, data).map_err(|err| format!("write {}: {err}", target.display()))?;
    }
    fs::write(dir.join(MANIFEST_NAME), manifest.to_toml())
        .map_err(|err| format!("write skin.toml: {err}"))?;

    Ok(manifest.id)
}

/// `SkinPackage` → `.zwskin` 字节（示例与导出共用）。
pub fn encode_package(package: &SkinPackage) -> Result<Vec<u8>, String> {
    encode(&package.manifest.to_toml(), &package.images)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
id = "t"
name = "测试"
[[layers]]
image = "body.png"
"#;

    fn sample() -> Vec<u8> {
        encode(MANIFEST, &[("body.png".to_string(), vec![1, 2, 3, 4])]).expect("encode")
    }

    #[test]
    fn roundtrip_keeps_manifest_and_bytes() {
        let files = decode(&sample()).expect("decode");
        assert_eq!(files.manifest_toml, MANIFEST);
        assert_eq!(files.images.len(), 1);
        assert_eq!(files.images[0].0, "body.png");
        assert_eq!(files.images[0].1, vec![1, 2, 3, 4]);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = sample();
        bytes[0] = b'X';
        assert!(decode(&bytes).unwrap_err().contains("bad magic"));
    }

    #[test]
    fn rejects_truncated_payload() {
        let bytes = sample();
        let err = decode(&bytes[..bytes.len() - 2]).unwrap_err();
        assert!(err.contains("truncated"), "unexpected: {err}");
    }

    #[test]
    fn rejects_declared_overlong_length() {
        // 手工把 data_len 改成远超实际长度的值
        let mut bytes = sample();
        let at = bytes.len() - 4 - 4; // 最后一个 u32 的位置（data 前）
        let idx = at;
        // 4000 字节：没到单文件上限，但远超包里实际剩下的内容
        bytes[idx..idx + 4].copy_from_slice(&4_000u32.to_le_bytes());
        let err = decode(&bytes).unwrap_err();
        assert!(err.contains("truncated"), "unexpected: {err}");

        // 而超过单文件上限时，要在分配内存之前就拒（报的是 limit）
        bytes[idx..idx + 4].copy_from_slice(&9_000_000u32.to_le_bytes());
        let err = decode(&bytes).unwrap_err();
        assert!(err.contains("limit"), "unexpected: {err}");
    }

    #[test]
    fn rejects_path_traversal_names() {
        assert!(!is_plain_name("../evil.png"));
        assert!(!is_plain_name("dir/evil.png"));
        assert!(!is_plain_name("C:evil.png"));
        assert!(!is_plain_name("..\\evil.png"));
        assert!(is_plain_name("body.png"));
        // 打包侧也不允许把危险名字写进包
        assert!(encode(MANIFEST, &[("../x.png".into(), vec![0])]).is_err());
    }

    #[test]
    fn rejects_missing_manifest() {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        push_entry(&mut out, "body.png", b"x").unwrap();
        assert!(decode(&out).unwrap_err().contains("skin.toml"));
    }

    #[test]
    fn rejects_duplicate_entries() {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes());
        push_entry(&mut out, MANIFEST_NAME, MANIFEST.as_bytes()).unwrap();
        push_entry(&mut out, "body.png", b"a").unwrap();
        push_entry(&mut out, "body.png", b"b").unwrap();
        assert!(decode(&out).unwrap_err().contains("duplicate"));
    }

    #[test]
    fn rejects_future_version() {
        let mut bytes = sample();
        bytes[4..6].copy_from_slice(&99u16.to_le_bytes());
        assert!(decode(&bytes)
            .unwrap_err()
            .contains("unsupported skin package version 99"));
    }
}
