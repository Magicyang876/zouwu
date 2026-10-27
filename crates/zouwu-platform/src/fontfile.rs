//! 从字体文件里读家族名（只读 `name` 表，不解析字形）。
//!
//! **为什么自己解析而不问系统**：导入用户丢进来的 `.ttf` 时，最直觉的做法是
//! `AddFontResourceEx` 之后枚举字体、比对"新增了哪些家族"。但这条路有两个坑：
//!
//! - 用户导的常常是**系统里已经装过**的字体（比如把微软雅黑拿出来当自定义字体包），
//!   注册后家族列表不会新增任何东西，差集为空 —— 我们就不知道家族名了；
//! - `FR_PRIVATE` 注册的字体在部分环境下**不可枚举**，那样连列表都拿不到。
//!
//! 而 `name` 表是字体文件里自带的、格式二十年没变过的东西：读它比问系统更可靠，
//! 也不依赖平台 API。整个解析是纯 Rust，可以单元测试。

/// 从 sfnt 数据里读出一组家族名（`.ttc` 集合会返回多个）。
///
/// 支持 TrueType（`\0\x01\0\0`）、CFF（`OTTO`）、Apple `true`，以及 `.ttc` 集合。
pub fn family_names(data: &[u8]) -> Result<Vec<String>, String> {
    let faces = face_offsets(data)?;
    let mut out = Vec::new();
    for offset in faces {
        if let Some(name) = best_family(data, offset)? {
            if !out.iter().any(|n| n == &name) {
                out.push(name);
            }
        }
    }
    if out.is_empty() {
        return Err("font has no family name in its `name` table".to_string());
    }
    Ok(out)
}

/// 单个字体（非集合）的家族名。
pub fn family_name(data: &[u8]) -> Result<String, String> {
    family_names(data)?
        .into_iter()
        .next()
        .ok_or_else(|| "empty".to_string())
}

/// 每个字面的偏移：普通字体是 `[0]`，`.ttc` 是集合里的每张脸。
fn face_offsets(data: &[u8]) -> Result<Vec<usize>, String> {
    if data.len() < 12 {
        return Err("file too short to be a font".to_string());
    }
    let tag = &data[0..4];
    let ok = matches!(tag, b"\0\x01\0\0" | b"OTTO" | b"true" | b"typ1");
    if ok {
        return Ok(vec![0]);
    }
    if tag != b"ttcf" {
        return Err(format!(
            "not a font file (tag {:02x?})",
            data[0..4].to_vec()
        ));
    }
    let num = be_u32(data, 8)? as usize;
    // 集合里动辄几十张脸，但真给到 4096 个以上只能是文件坏了。
    if num == 0 || num > 4096 {
        return Err(format!("bad ttc font count {num}"));
    }
    (0..num)
        .map(|i| be_u32(data, 12 + i * 4).map(|off| off as usize))
        .collect()
}

/// 在一张脸的 `name` 表里挑最合适的家族名。
///
/// 优先级：Windows 平台的**英文**记录 > 任意平台的英文记录 > 第一条家族记录。
/// 为什么要挑英文：`zh-CN` 记录的家族名是"微软雅黑"这类本地化名字，
/// 而 `CreateFontW` 用中文名也能工作，但一旦跨语言（用户切到英文界面）
/// 就找不到字体了，英文名不会有这个问题。
fn best_family(data: &[u8], face: usize) -> Result<Option<String>, String> {
    let (name_offset, name_len) = table(data, face, b"name")?;
    let name = data
        .get(name_offset..name_offset + name_len)
        .ok_or_else(|| "name table out of bounds".to_string())?;
    if name.len() < 6 {
        return Ok(None);
    }
    let count = be_u16(name, 2)? as usize;
    let string_offset = be_u16(name, 4)? as usize;
    if count > 4096 {
        return Err(format!("bad name record count {count}"));
    }

    // (优先级分数, 家族名)。分数越小越好。
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let rec = 6 + i * 12;
        let Some(r) = name.get(rec..rec + 12) else {
            break;
        };
        let platform = be_u16(r, 0)?;
        let language = be_u16(r, 4)?;
        let name_id = be_u16(r, 6)?;
        let len = be_u16(r, 8)? as usize;
        let off = be_u16(r, 10)? as usize;
        // nameID 1 = 家族名，16 = 排版家族名（有 16 时它更准，如 "思源宋体 CN / ExtraLight"）。
        if name_id != 1 && name_id != 16 {
            continue;
        }
        let start = string_offset + off;
        let Some(bytes) = name.get(start..start + len) else {
            continue;
        };
        let Some(text) = decode(platform, bytes) else {
            continue;
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        // 控制字符意味着解码或切片错了，宁可丢掉。
        if text.chars().any(|c| c.is_control()) {
            continue;
        }
        let english = platform == 3 && (language & 0x3ff) == 0x09 || platform == 1 && language == 0;
        let score = match (name_id == 16, english, platform == 3) {
            (true, true, true) => 0,
            (false, true, true) => 1,
            (true, true, false) => 2,
            (false, true, false) => 3,
            (true, false, _) => 4,
            (false, false, _) => 5,
        };
        if best.as_ref().map(|(s, _)| score < *s).unwrap_or(true) {
            best = Some((score, text));
        }
    }
    Ok(best.map(|(_, name)| name))
}

/// Windows 平台是 UTF-16BE；Mac 平台历史上是 MacRoman，实际上现在基本都是 ASCII/UTF-8。
fn decode(platform: u16, bytes: &[u8]) -> Option<String> {
    if platform == 3 || platform == 0 {
        if bytes.len() % 2 != 0 {
            return None;
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&units).ok()
    } else {
        std::str::from_utf8(bytes)
            .ok()
            .map(|s| s.to_string())
            .or_else(|| Some(bytes.iter().map(|&b| b as char).collect()))
    }
}

/// 字形覆盖探针返回的字符集是否都在这份字体里。
///
/// 返回**缺失**的字符（空 = 全覆盖）。用于导入时提醒"这个字体没有中文"：
/// GDI 遇到缺字会走系统 font-link 回落，中文会突然换成宋体（150% 下糊成一团），
/// 但宽度口径仍按你选的字体算 —— 看上去像"字被压扁"。
///
/// 只探几个代表字而不是扫全表：`永` 在几乎所有 CJK 字体里都有，`A` 覆盖拉丁，
/// `，` 覆盖全角标点。扫 2 万个码位要遍历整张 cmap，导入时不该花这个时间。
pub fn missing_chars(data: &[u8], probe: &[char]) -> Result<Vec<char>, String> {
    let faces = face_offsets(data)?;
    let face = faces[0];
    let (start, _len) = table(data, face, b"cmap")?;
    let mut out = Vec::new();
    for &ch in probe {
        if !has_glyph(data, start, ch as u32)? {
            out.push(ch);
        }
    }
    Ok(out)
}

/// 在 cmap 里查一个码位有没有字形（支持 format 4 与 12，其它格式当"没有"）。
fn has_glyph(data: &[u8], cmap: usize, cp: u32) -> Result<bool, String> {
    let count = be_u16(data, cmap + 2)? as usize;
    for i in 0..count.min(8) {
        let rec = cmap + 4 + i * 8;
        let Some(r) = data.get(rec..rec + 8) else {
            break;
        };
        let offset = be_u32(r, 4)? as usize;
        let sub = cmap + offset;
        match be_u16(data, sub)? {
            4 => {
                if cp > 0xFFFF {
                    continue;
                }
                if format4_lookup(data, sub, cp as u16)? {
                    return Ok(true);
                }
            }
            12 => {
                if format12_lookup(data, sub, cp)? {
                    return Ok(true);
                }
            }
            _ => continue,
        }
    }
    Ok(false)
}

fn format4_lookup(data: &[u8], sub: usize, c: u16) -> Result<bool, String> {
    let seg_x2 = be_u16(data, sub + 6)? as usize;
    let segs = seg_x2 / 2;
    if segs == 0 || segs > 0x4000 {
        return Ok(false);
    }
    let ends = sub + 14;
    let starts = ends + seg_x2 + 2;
    for i in 0..segs {
        let end = match be_u16(data, ends + i * 2) {
            Ok(v) => v,
            Err(_) => break,
        };
        if c > end {
            continue;
        }
        let start = be_u16(data, starts + i * 2)?;
        if c < start {
            return Ok(false);
        }
        // 区间命中：起点码位有字形就算有这个字符（不区分 .notdef 映射细节）。
        return Ok(true);
    }
    Ok(false)
}

fn format12_lookup(data: &[u8], sub: usize, cp: u32) -> Result<bool, String> {
    let groups = be_u32(data, sub + 12)? as usize;
    if groups > 0x40000 {
        return Ok(false);
    }
    for i in 0..groups {
        let at = sub + 16 + i * 12;
        let (Ok(start), Ok(end)) = (be_u32(data, at), be_u32(data, at + 4)) else {
            break;
        };
        if cp < start {
            return Ok(false);
        }
        if cp <= end {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 在表目录里找一张表，返回 (偏移, 长度)。
fn table(data: &[u8], face: usize, want: &[u8; 4]) -> Result<(usize, usize), String> {
    let num_tables = be_u16(data, face + 4)? as usize;
    if num_tables == 0 || num_tables > 512 {
        return Err(format!("bad table count {num_tables}"));
    }
    for i in 0..num_tables {
        let rec = face + 12 + i * 16;
        let Some(r) = data.get(rec..rec + 16) else {
            break;
        };
        if &r[0..4] != want {
            continue;
        }
        let offset = be_u32(r, 8)? as usize;
        let length = be_u32(r, 12)? as usize;
        if offset + length > data.len() {
            return Err(format!(
                "table {:?} out of bounds ({offset}+{length} > {})",
                want,
                data.len()
            ));
        }
        return Ok((offset, length));
    }
    Err(format!("no {:?} table (not a usable font)", want))
}

fn be_u16(data: &[u8], at: usize) -> Result<u16, String> {
    let s = data
        .get(at..at + 2)
        .ok_or_else(|| format!("read past end at {at}"))?;
    Ok(u16::from_be_bytes([s[0], s[1]]))
}

fn be_u32(data: &[u8], at: usize) -> Result<u32, String> {
    let s = data
        .get(at..at + 4)
        .ok_or_else(|| format!("read past end at {at}"))?;
    Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个真实系统字体（测试环境是 Windows；没有就跳过）。
    fn system_font(name: &str) -> Option<Vec<u8>> {
        let path = std::path::Path::new("C:/Windows/Fonts").join(name);
        std::fs::read(path).ok()
    }

    #[test]
    fn reads_family_from_a_real_ttf() {
        let Some(data) = system_font("consola.ttf") else {
            eprintln!("consola.ttf not present, skipping");
            return;
        };
        let names = family_names(&data).expect("parse");
        assert_eq!(names[0], "Consolas", "got {names:?}");
    }

    #[test]
    fn reads_family_from_a_ttc_collection() {
        let Some(data) = system_font("msyh.ttc") else {
            eprintln!("msyh.ttc not present, skipping");
            return;
        };
        let names = family_names(&data).expect("parse");
        assert!(
            names.iter().any(|n| n == "Microsoft YaHei"),
            "expected Microsoft YaHei among {names:?}"
        );
    }

    /// 字体包的“中文能不能看”必须在导入时就知道。
    ///
    /// Consolas 只有拉丁字形：导入它之后中文会回落到系统 font-link
    /// （宋体点阵，150% 下糊成一团），但宽度口径仍按 Consolas 算。
    #[test]
    fn coverage_tells_latin_fonts_apart_from_cjk_fonts() {
        let probe = ['A', '永', '，'];
        if let Some(data) = system_font("consola.ttf") {
            let missing = missing_chars(&data, &probe).expect("parse cmap");
            assert!(!missing.contains(&'A'), "Consolas 应该有拉丁字形");
            assert!(missing.contains(&'永'), "Consolas 不该有中文字形");
        }
        if let Some(data) = system_font("msyh.ttc") {
            let missing = missing_chars(&data, &probe).expect("parse cmap");
            assert!(missing.is_empty(), "雅黑应该全覆盖，缺 {missing:?}");
        }
    }

    #[test]
    fn rejects_non_font_bytes() {
        let err = family_names(b"this is definitely not a font file").unwrap_err();
        assert!(err.contains("not a font"), "{err}");
        assert!(family_names(&[]).is_err());
        assert!(family_names(&[0, 1, 0, 0]).is_err());
    }

    #[test]
    fn truncated_font_is_an_error_not_a_panic() {
        let Some(data) = system_font("consola.ttf") else {
            return;
        };
        // 每一段前缀都不能 panic —— 导入的是用户随便丢的文件。
        for cut in [4usize, 12, 20, 64, 512, 4096, data.len() / 2] {
            let _ = family_names(&data[..cut.min(data.len())]);
        }
    }
}
