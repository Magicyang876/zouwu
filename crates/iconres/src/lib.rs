//! 把一个 `.ico` 变成 **COFF 资源对象**（`.obj`），好让 gnu 目标的 exe 也带上图标。
//!
//! 背景（ADR-050）：gnu 目标下 rustc 用 `rust-lld -flavor gnu`，它只认 COFF 对象、不认
//! `.res`（实测 `zouwu.res: file format not recognized`）。正经路子是 `windres`，可它属于
//! binutils，Windows 上默认没有。我们其实只差“把资源树写成 COFF”这一步，于是自己拼：
//! 解析 `.ico` → 拼 `IMAGE_RESOURCE_DIRECTORY` 树 → 套 COFF 文件头 / 段头 / 重定位表。
//!
//! 相对链接器我们只要求一件事：把 `.rsrc` 段的**段 RVA** 加到那几个“偏移”字段上。用
//! `IMAGE_REL_AMD64_ADDR32NB` 正好是这个语义（`S + A`，`A` 就是字段里已有的偏移），所以
//! 每个数据项的 RVA 字段各配一条重定位即可，不需要补丁表（同名符号 `S` 的 value 为 0）。
//!
//! 这套东西只用到“读一个 ico、写一个 obj”，所以是独立的小 crate：`build.rs` 里调它，
//! 单元测试也能直接喂假的 ico 进去验字节。

#![deny(missing_docs)]

/// 一张图标图像（`.ico` 里的一项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    /// 宽度（0 表示 256）。
    pub width: u8,
    /// 高度（0 表示 256）。
    pub height: u8,
    /// 色平面数，通常 1。
    pub planes: u16,
    /// 每像素位数，通常 32。
    pub bit_count: u16,
    /// 图像数据：DIB（不含 ICO 头）或整张 PNG。
    pub data: Vec<u8>,
}

const ICONDIR: usize = 6;
const ICONDIRENTRY: usize = 16;
const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;

/// 解析 `.ico`。只认 PNG/DIB 都行——我们不碰图像内容，原样搬进资源。
pub fn parse_ico(bytes: &[u8]) -> Result<Vec<IconImage>, String> {
    if bytes.len() < ICONDIR {
        return Err("ico: too short".into());
    }
    let reserved = u16::from_le_bytes([bytes[0], bytes[1]]);
    let kind = u16::from_le_bytes([bytes[2], bytes[3]]);
    if reserved != 0 || kind != 1 {
        return Err(format!("ico: bad header ({reserved}, {kind})"));
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if count == 0 || bytes.len() < ICONDIR + count * ICONDIRENTRY {
        return Err("ico: bad entry count".into());
    }

    let mut images = Vec::with_capacity(count);
    for i in 0..count {
        let e = &bytes[ICONDIR + i * ICONDIRENTRY..ICONDIR + (i + 1) * ICONDIRENTRY];
        let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let end = offset
            .checked_add(size)
            .ok_or("ico: image offset overflow")?;
        if size == 0 || end > bytes.len() {
            return Err(format!("ico: image {i} out of range"));
        }
        images.push(IconImage {
            width: e[0],
            height: e[1],
            planes: u16::from_le_bytes([e[4], e[5]]),
            bit_count: u16::from_le_bytes([e[6], e[7]]),
            data: bytes[offset..end].to_vec(),
        });
    }
    Ok(images)
}

fn align_up(n: usize, to: usize) -> usize {
    if to <= 1 {
        return n;
    }
    n.div_ceil(to) * to
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// 目录头：`named = 0`（我们不用字符串 ID），`ids` 是带数字 ID 的子项数。
///
/// 这里有个坑：`named`/`ids` 写反了，Windows 会把第一项当字符串 ID 去解析，资源直接不显示。
fn dir_header(out: &mut Vec<u8>, ids: u16) {
    put_u32(out, 0); // Characteristics
    put_u32(out, 0); // TimeDateStamp
    put_u16(out, 0); // MajorVersion
    put_u16(out, 0); // MinorVersion
    put_u16(out, 0); // NumberOfNamedEntries
    put_u16(out, ids); // NumberOfIdEntries
}

fn dir_entry_to_dir(out: &mut Vec<u8>, id: u32, offset: usize) {
    put_u32(out, id);
    put_u32(out, 0x8000_0000 | offset as u32);
}

fn dir_entry_to_data(out: &mut Vec<u8>, id: u32, offset: usize) {
    put_u32(out, id);
    put_u32(out, offset as u32);
}

fn data_entry(out: &mut Vec<u8>, data_offset: usize, size: usize) {
    put_u32(out, data_offset as u32); // RVA，由链接器的 ADDR32NB 补上段 RVA
    put_u32(out, size as u32);
    put_u32(out, 0); // CodePage
    put_u32(out, 0); // Reserved
}

/// `RT_GROUP_ICON` 的数据块：`GRPICONDIR` + 每张图 14 字节。
fn group_icon(images: &[IconImage]) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + images.len() * 14);
    put_u16(&mut out, 0); // 保留
    put_u16(&mut out, 1); // 类型：图标
    put_u16(&mut out, images.len() as u16);
    for (i, img) in images.iter().enumerate() {
        out.push(img.width);
        out.push(img.height);
        out.push(0); // 调色板颜色数
        out.push(0); // 保留
        put_u16(&mut out, img.planes);
        put_u16(&mut out, img.bit_count);
        put_u32(&mut out, img.data.len() as u32);
        put_u16(&mut out, (i + 1) as u16); // 引用 RT_ICON 的 ID
    }
    out
}

/// 拼出 `.rsrc` 段的内容，同时返回“需要重定位的字段偏移”列表。
pub fn rsrc(images: &[IconImage]) -> (Vec<u8>, Vec<u32>) {
    let n = images.len();
    // 偏移按“目录 → 数据项 → 数据块”的顺序一路排下去。
    let root_off = 0usize;
    let icon_type_off = root_off + 16 + 2 * 8;
    let group_type_off = icon_type_off + 16 + n * 8;
    let icon_ids_off = group_type_off + 16 + 8;
    let group_ids_off = icon_ids_off + 16 + n * 8;
    let data_entries_off = group_ids_off + 16 + 8;

    let mut cursor = data_entries_off + (n + 1) * 16;
    let mut icon_data_off = Vec::with_capacity(n);
    for img in images {
        cursor = align_up(cursor, 4);
        icon_data_off.push(cursor);
        cursor += img.data.len();
    }
    let group = group_icon(images);
    let group_data_off = align_up(cursor, 4);

    let data_entry_off = |index: usize| data_entries_off + index * 16;

    let mut out = Vec::with_capacity(group_data_off + group.len());
    // 根目录：两种类型
    dir_header(&mut out, 2);
    dir_entry_to_dir(&mut out, RT_ICON, icon_type_off);
    dir_entry_to_dir(&mut out, RT_GROUP_ICON, group_type_off);
    debug_assert_eq!(out.len(), icon_type_off);

    // RT_ICON 类型目录：n 个数字 ID
    dir_header(&mut out, n as u16);
    for i in 0..n {
        dir_entry_to_dir(&mut out, (i + 1) as u32, icon_ids_off);
    }
    debug_assert_eq!(out.len(), group_type_off);

    // RT_GROUP_ICON 类型目录：只有 1 号
    dir_header(&mut out, 1);
    dir_entry_to_dir(&mut out, 1, group_ids_off);
    debug_assert_eq!(out.len(), icon_ids_off);

    // 图标 ID 目录
    dir_header(&mut out, n as u16);
    for i in 0..n {
        dir_entry_to_data(&mut out, (i + 1) as u32, data_entry_off(i));
    }
    debug_assert_eq!(out.len(), group_ids_off);

    // 分组图标 ID 目录
    dir_header(&mut out, 1);
    dir_entry_to_data(&mut out, 1, data_entry_off(n));
    debug_assert_eq!(out.len(), data_entries_off);

    // 数据项
    for (i, img) in images.iter().enumerate() {
        data_entry(&mut out, icon_data_off[i], img.data.len());
    }
    data_entry(&mut out, group_data_off, group.len());
    debug_assert_eq!(out.len(), data_entries_off + (n + 1) * 16);

    // 数据块
    for (i, img) in images.iter().enumerate() {
        out.resize(icon_data_off[i], 0);
        out.extend_from_slice(&img.data);
    }
    out.resize(group_data_off, 0);
    out.extend_from_slice(&group);
    debug_assert_eq!(out.len(), group_data_off + group.len());

    let mut sites: Vec<u32> = (0..=n).map(|i| data_entry_off(i) as u32).collect();
    sites.sort_unstable();
    (out, sites)
}

const FILE_HEADER: usize = 20;
const SECTION_HEADER: usize = 40;
/// `IMAGE_REL_AMD64_ADDR32NB`
const REL_ADDR32NB: u16 = 3;

/// 把段内容包成一个 64 位 COFF 对象（一个 `.rsrc` 段、一个符号、n 条重定位）。
pub fn coff_object(payload: &[u8], sites: &[u32]) -> Vec<u8> {
    let raw_off = FILE_HEADER + SECTION_HEADER;
    let reloc_off = align_up(raw_off + payload.len(), 4);
    let reloc_size = sites.len() * 10;
    let sym_off = align_up(reloc_off + reloc_size, 4);
    let sym_size = 18;
    let strtab_off = sym_off + sym_size;
    let total = strtab_off + 4;

    let mut out = Vec::with_capacity(total);
    // 文件头
    put_u16(&mut out, 0x8664); // Machine: x86-64
    put_u16(&mut out, 1); // NumberOfSections
    put_u32(&mut out, 0); // TimeDateStamp: 可复现构建，别写时间
    put_u32(&mut out, sym_off as u32); // PointerToSymbolTable
    put_u32(&mut out, 1); // NumberOfSymbols
    put_u16(&mut out, 0); // SizeOfOptionalHeader
    put_u16(&mut out, 0); // Characteristics
    debug_assert_eq!(out.len(), FILE_HEADER);

    // 段头
    out.extend_from_slice(b".rsrc\0\0\0");
    put_u32(&mut out, 0); // PhysicalAddress/VirtualSize：由链接器填
    put_u32(&mut out, 0); // VirtualAddress：由链接器填
    put_u32(&mut out, payload.len() as u32); // SizeOfRawData
    put_u32(&mut out, raw_off as u32); // PointerToRawData
    put_u32(&mut out, reloc_off as u32); // PointerToRelocations
    put_u32(&mut out, 0); // PointerToLinenumbers
    put_u16(&mut out, sites.len() as u16); // NumberOfRelocations
    put_u16(&mut out, 0); // NumberOfLinenumbers
    put_u32(&mut out, 0x4000_0040); // 已初始化数据 | 可读
    debug_assert_eq!(out.len(), raw_off);

    // 段数据
    out.extend_from_slice(payload);
    out.resize(reloc_off, 0);

    // 重定位：VirtualAddress = 字段偏移，SymbolTableIndex = 0，Type = ADDR32NB
    for &site in sites {
        put_u32(&mut out, site);
        put_u32(&mut out, 0);
        put_u16(&mut out, REL_ADDR32NB);
    }
    out.resize(sym_off, 0);

    // 符号表：`.rsrc`，value 0，section 1，STATIC
    out.extend_from_slice(b".rsrc\0\0\0");
    put_u32(&mut out, 0); // Value
    put_u16(&mut out, 1); // SectionNumber
    put_u16(&mut out, 0); // Type
    out.push(3); // StorageClass: STATIC
    out.push(0); // NumberOfAuxSymbols
    debug_assert_eq!(out.len(), strtab_off);

    // 字符串表（只放最小头，符号名短到能塞进 8 字节）
    put_u32(&mut out, 4);
    debug_assert_eq!(out.len(), total);
    out
}

/// 一步到位：`.ico` 字节 → COFF 资源对象字节。
pub fn resource_object(ico: &[u8]) -> Result<Vec<u8>, String> {
    let images = parse_ico(ico)?;
    let (payload, sites) = rsrc(&images);
    Ok(coff_object(&payload, &sites))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个两图的假 ico：图片数据是任意字节，我们不解析内容。
    fn fake_ico() -> Vec<u8> {
        let a = b"AAAA".to_vec();
        let b = b"BBBBBBBB".to_vec();
        let mut out = Vec::new();
        put_u16(&mut out, 0);
        put_u16(&mut out, 1);
        put_u16(&mut out, 2);
        let header = ICONDIR + 2 * ICONDIRENTRY;
        for (i, (w, h, data)) in [(16u8, 16u8, &a), (32, 32, &b)].iter().enumerate() {
            out.push(*w);
            out.push(*h);
            out.push(0);
            out.push(0);
            put_u16(&mut out, 1);
            put_u16(&mut out, 32);
            put_u32(&mut out, data.len() as u32);
            let offset = if i == 0 { header } else { header + a.len() };
            put_u32(&mut out, offset as u32);
        }
        out.extend_from_slice(&a);
        out.extend_from_slice(&b);
        out
    }

    #[test]
    fn parsing_reads_every_image() {
        let images = parse_ico(&fake_ico()).expect("parse");
        assert_eq!(images.len(), 2);
        assert_eq!((images[0].width, images[0].height), (16, 16));
        assert_eq!((images[1].width, images[1].height), (32, 32));
        assert_eq!(images[0].data, b"AAAA");
        assert_eq!(images[1].data, b"BBBBBBBB");
        assert_eq!(images[1].bit_count, 32);
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse_ico(b"").is_err());
        assert!(parse_ico(b"\x00\x00\x01\x00\x00\x00").is_err()); // 0 张图
                                                                  // 图片跑出文件末尾
        let mut ico = fake_ico();
        ico[6 + 8] = 0xFF;
        assert!(parse_ico(&ico).is_err());
    }

    #[test]
    fn the_resource_tree_has_both_types_and_no_named_entries() {
        let images = parse_ico(&fake_ico()).expect("parse");
        let (payload, sites) = rsrc(&images);
        let u16_at = |o: usize| u16::from_le_bytes([payload[o], payload[o + 1]]);
        let u32_at = |o: usize| {
            u32::from_le_bytes([payload[o], payload[o + 1], payload[o + 2], payload[o + 3]])
        };
        // 根目录：0 个字符串 ID、2 个数字 ID
        assert_eq!(u16_at(12), 0, "NumberOfNamedEntries");
        assert_eq!(u16_at(14), 2, "NumberOfIdEntries");
        // 两个类型：RT_ICON(3) 与 RT_GROUP_ICON(14)，都指向子目录
        assert_eq!(u32_at(16), RT_ICON);
        assert_eq!(u32_at(20) & 0x8000_0000, 0x8000_0000);
        assert_eq!(u32_at(24), RT_GROUP_ICON);
        // 3 个数据项（2 张图 + 1 个分组）需要 3 条重定位
        assert_eq!(sites.len(), 3);
    }

    #[test]
    fn data_entries_point_inside_the_payload() {
        let images = parse_ico(&fake_ico()).expect("parse");
        let (payload, sites) = rsrc(&images);
        for site in &sites {
            let at = *site as usize;
            let rva = u32::from_le_bytes([
                payload[at],
                payload[at + 1],
                payload[at + 2],
                payload[at + 3],
            ]) as usize;
            let size = u32::from_le_bytes([
                payload[at + 4],
                payload[at + 5],
                payload[at + 6],
                payload[at + 7],
            ]) as usize;
            assert!(size > 0);
            assert!(
                rva + size <= payload.len(),
                "data entry at {at} runs past the payload ({rva}+{size} > {})",
                payload.len()
            );
            // 数据块要 4 字节对齐（Windows 读图标时要求）
            assert_eq!(rva % 4, 0, "image data must be 4-byte aligned");
        }
    }

    #[test]
    fn the_group_icon_lists_every_image_and_its_id() {
        let images = parse_ico(&fake_ico()).expect("parse");
        let (payload, sites) = rsrc(&images);
        let group_site = *sites.last().expect("group data entry") as usize;
        let rva = u32::from_le_bytes([
            payload[group_site],
            payload[group_site + 1],
            payload[group_site + 2],
            payload[group_site + 3],
        ]) as usize;
        let group = &payload[rva..];
        assert_eq!(&group[0..4], &[0, 0, 1, 0], "reserved/type");
        assert_eq!(u16::from_le_bytes([group[4], group[5]]), 2, "count");
        // 每项 14 字节，末尾两字节是 RT_ICON 的 ID
        assert_eq!(u16::from_le_bytes([group[6 + 12], group[6 + 13]]), 1);
        assert_eq!(u16::from_le_bytes([group[20 + 12], group[20 + 13]]), 2);
        assert_eq!(
            u32::from_le_bytes([group[6 + 8], group[6 + 9], group[6 + 10], group[6 + 11]]),
            4
        );
    }

    #[test]
    fn the_coff_object_is_well_formed() {
        let ico = fake_ico();
        let obj = resource_object(&ico).expect("object");
        let u16_at = |o: usize| u16::from_le_bytes([obj[o], obj[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes([obj[o], obj[o + 1], obj[o + 2], obj[o + 3]]);
        assert_eq!(u16_at(0), 0x8664, "machine");
        assert_eq!(u16_at(2), 1, "one section");
        assert_eq!(u32_at(12), 1, "one symbol");
        assert_eq!(&obj[20..25], b".rsrc");
        let size_of_raw = u32_at(20 + 16) as usize;
        let ptr_raw = u32_at(20 + 20) as usize;
        let ptr_reloc = u32_at(20 + 24) as usize;
        let nreloc = u16_at(20 + 32) as usize;
        assert_eq!(ptr_raw, FILE_HEADER + SECTION_HEADER);
        assert_eq!(nreloc, 3);
        assert!(ptr_reloc >= ptr_raw + size_of_raw);
        assert_eq!(ptr_reloc % 4, 0);
        let ptr_sym = u32_at(8) as usize;
        assert_eq!(ptr_sym % 4, 0);
        assert!(ptr_sym >= ptr_reloc + nreloc * 10);
        // 符号表项 + 字符串表尾巴刚好撑到文件末尾
        assert_eq!(obj.len(), ptr_sym + 18 + 4);
        assert_eq!(u32_at(obj.len() - 4), 4, "empty string table");
        // 每条重定位都指向 ADDR32NB，且落在段数据里
        for i in 0..nreloc {
            let at = ptr_reloc + i * 10;
            let site = u32_at(at) as usize;
            assert_eq!(u16_at(at + 8), REL_ADDR32NB);
            assert!(site + 4 <= size_of_raw);
        }
    }

    #[test]
    fn the_real_icon_parses() {
        // 仓库里的图标是 256/128/64/48/32/24/20/16 八张
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/zouwu.ico"
        ))
        .expect("assets/zouwu.ico");
        let images = parse_ico(&bytes).expect("parse real icon");
        assert!(
            images.len() >= 6,
            "expected many sizes, got {}",
            images.len()
        );
        let obj = resource_object(&bytes).expect("object");
        assert!(obj.len() > bytes.len() / 2);
    }
}
