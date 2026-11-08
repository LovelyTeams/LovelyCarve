//! PE 资源树中 RT_VERSION（版本信息）资源的定位与解析。
//!
//! 只为「原类型视图」抽取 VS_FIXEDFILEINFO（文件/产品版本号）和
//! StringFileInfo 键值对（CompanyName / OriginalFilename 等取证常用字段），
//! 不做完整资源枚举。容错原则：结构异常时返回已收集到的部分，所有偏移按
//! 敌意数据做边界检查。由 [`binfmt_pe`](crate::preview::binary::binfmt_pe) 从 goblin 已解析
//! 的节区表建 RVA→文件偏移映射后喂入（goblin 的资源树无一次到位的版本访问器）。

use serde::Serialize;

use crate::preview::binary::binfmt_util::{u16le, u32le};

/// StringFileInfo 键值对数量上限（跨多语言表合计）。
const MAX_STRINGS: usize = 64;
/// 资源树/版本块递归深度上限，防御自引用的损坏资源目录。
const MAX_DEPTH: usize = 8;

#[derive(Debug, Serialize)]
pub struct PeVersionInfo {
    /// VS_FIXEDFILEINFO 的二进制版本号，格式 `a.b.c.d`。
    pub file_version: Option<String>,
    pub product_version: Option<String>,
    /// StringFileInfo 键值对（CompanyName、FileDescription、OriginalFilename…）。
    pub strings: Vec<VersionString>,
    /// Translation 表（语言 ID + 代码页），如 `"0409 04b0"`。
    pub translations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct VersionString {
    pub key: String,
    pub value: String,
}

/// 在资源树中找到第一个 RT_VERSION 资源并解析。`rva_to_off` 来自调用方的
/// 节区映射；`rsrc_rva` 是 Resource 数据目录的 RVA。
pub fn version_info(
    data: &[u8],
    rva_to_off: impl Fn(u32) -> Option<usize>,
    rsrc_rva: u32,
) -> anyhow::Result<Option<PeVersionInfo>> {
    let base =
        rva_to_off(rsrc_rva).ok_or_else(|| anyhow::anyhow!("资源目录 RVA 无法定位到文件偏移"))?;

    // 三层目录：类型 → 名称/ID → 语言，之后是数据条目。前两层必须是子目录
    // （offset 高位为 1），叶子必须是数据条目（高位为 0）——标志位不符说明
    // 资源树损坏或被构造，沿错误结构走只会读出垃圾。
    let Some((type_dir, true)) = find_entry(data, base, 0, EntryMatch::Id(16)) else {
        return Ok(None); // 没有版本资源（或标志位异常），正常情况
    };
    let Some((name_dir, true)) = find_entry(data, base, type_dir, EntryMatch::First) else {
        return Ok(None);
    };
    let Some((leaf, false)) = find_entry(data, base, name_dir, EntryMatch::First) else {
        return Ok(None);
    };
    // leaf 应为数据条目偏移（相对 rsrc base）：IMAGE_RESOURCE_DATA_ENTRY
    let de = base + leaf;
    let data_rva = u32le(data, de).ok_or_else(|| anyhow::anyhow!("资源数据条目越界"))?;
    let data_size = u32le(data, de + 4).unwrap_or(0) as usize;
    let off = rva_to_off(data_rva)
        .ok_or_else(|| anyhow::anyhow!("版本资源数据 RVA 无法定位到文件偏移"))?;
    let block = data
        .get(off..off.saturating_add(data_size.min(64 * 1024)))
        .ok_or_else(|| anyhow::anyhow!("版本资源数据越过文件末尾"))?;
    Ok(parse_vs_versioninfo(block))
}

enum EntryMatch {
    /// 按资源 ID 匹配（RT_VERSION = 16）。
    Id(u32),
    /// 取第一个条目（名称或 ID 不限）。
    First,
}

/// 在 `dir_off`（相对 rsrc base）处的资源目录里找条目。返回条目 offset 字段
/// 去掉子目录标志位后的值（仍相对 rsrc base）和「是否子目录」（高位 1 =
/// 子目录，0 = 数据条目），由调用方按所在层级校验。
fn find_entry(data: &[u8], base: usize, dir_off: usize, want: EntryMatch) -> Option<(usize, bool)> {
    let dir = base + dir_off;
    let named = u16le(data, dir + 12)? as usize;
    let ids = u16le(data, dir + 14)? as usize;
    let total = (named + ids).min(1024);
    for i in 0..total {
        let e = dir + 16 + i * 8;
        let key = u32le(data, e)?;
        let offset = u32le(data, e + 4)?;
        let matched = match want {
            // ID 条目的 key 高位为 0；命名条目（高位 1）不参与 ID 匹配
            EntryMatch::Id(id) => key & 0x8000_0000 == 0 && key == id,
            EntryMatch::First => true,
        };
        if matched {
            return Some(((offset & 0x7fff_ffff) as usize, offset & 0x8000_0000 != 0));
        }
    }
    None
}

// ---- VS_VERSIONINFO ------------------------------------------------------

/// 4 字节对齐（相对版本块起始）。
fn align4(off: usize) -> usize {
    (off + 3) & !3
}

/// 读 UTF-16LE NUL 结尾字符串，返回（字符串, NUL 之后的偏移）。
fn utf16z(d: &[u8], mut off: usize) -> Option<(String, usize)> {
    let mut units = Vec::new();
    loop {
        let u = u16le(d, off)?;
        off += 2;
        if u == 0 {
            break;
        }
        if units.len() >= 512 {
            return None; // 无终止符的损坏数据
        }
        units.push(u);
    }
    Some((String::from_utf16_lossy(&units), off))
}

struct VersionBlock {
    end: usize,
    value_len: usize,
    is_text: bool,
    key: String,
    /// value（若有）的起始偏移；children 紧随 value 对齐之后。
    value_off: usize,
}

/// 读一个 {wLength, wValueLength, wType, szKey, padding} 块头。
fn block_header(d: &[u8], off: usize) -> Option<VersionBlock> {
    let w_length = u16le(d, off)? as usize;
    let value_len = u16le(d, off + 2)? as usize;
    let w_type = u16le(d, off + 4)?;
    if w_length < 8 || off + w_length > d.len() {
        return None;
    }
    let (key, after_key) = utf16z(d, off + 6)?;
    Some(VersionBlock {
        end: off + w_length,
        value_len,
        is_text: w_type == 1,
        key,
        value_off: align4(after_key),
    })
}

fn parse_vs_versioninfo(d: &[u8]) -> Option<PeVersionInfo> {
    let root = block_header(d, 0)?;
    if root.key != "VS_VERSION_INFO" {
        return None;
    }
    let mut info = PeVersionInfo {
        file_version: None,
        product_version: None,
        strings: Vec::new(),
        translations: Vec::new(),
    };

    // VS_FIXEDFILEINFO（可选，52 字节，签名 0xFEEF04BD）
    let mut child_off = root.value_off;
    if root.value_len >= 52 && u32le(d, root.value_off) == Some(0xfeef_04bd) {
        let v = root.value_off;
        let ver =
            |ms: u32, ls: u32| format!("{}.{}.{}.{}", ms >> 16, ms & 0xffff, ls >> 16, ls & 0xffff);
        if let (Some(fms), Some(fls), Some(pms), Some(pls)) = (
            u32le(d, v + 8),
            u32le(d, v + 12),
            u32le(d, v + 16),
            u32le(d, v + 20),
        ) {
            info.file_version = Some(ver(fms, fls));
            info.product_version = Some(ver(pms, pls));
        }
        child_off = align4(root.value_off + root.value_len);
    }

    walk_children(d, child_off, root.end, &mut info, 0);
    Some(info)
}

/// 递归走 StringFileInfo / VarFileInfo 子块。
fn walk_children(d: &[u8], mut off: usize, end: usize, info: &mut PeVersionInfo, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    while off + 8 <= end {
        let Some(block) = block_header(d, off) else {
            break;
        };
        if block.end <= off {
            break; // 长度不前进，防死循环
        }
        match (depth, block.key.as_str()) {
            (0, "StringFileInfo") | (0, "VarFileInfo") => {
                walk_children(d, block.value_off, block.end, info, depth + 1);
            }
            // StringFileInfo 下的 StringTable（key 是 lang+codepage 十六进制）
            (1, _) if !block.is_text || block.value_len == 0 => {
                if block.key == "Translation" {
                    // VarFileInfo/Translation：u32 数组 lang|codepage
                    let mut v = block.value_off;
                    while v + 4 <= block.end && info.translations.len() < 16 {
                        if let (Some(lang), Some(cp)) = (u16le(d, v), u16le(d, v + 2)) {
                            info.translations.push(format!("{lang:04x} {cp:04x}"));
                        }
                        v += 4;
                    }
                } else {
                    walk_children(d, block.value_off, block.end, info, depth + 1);
                }
            }
            // StringTable 下的 String：value 是 UTF-16 文本
            (2, _) if block.is_text && info.strings.len() < MAX_STRINGS => {
                let value = utf16z(d, block.value_off)
                    .map(|(s, _)| s)
                    .unwrap_or_default();
                info.strings.push(VersionString {
                    key: block.key,
                    value: value.trim_end_matches('\0').to_string(),
                });
            }
            _ => {}
        }
        off = align4(block.end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最小 VS_VERSIONINFO：fixed info + StringFileInfo/StringTable
    /// 一个键值对。
    fn push_u16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn push_utf16z(v: &mut Vec<u8>, s: &str) {
        for u in s.encode_utf16() {
            push_u16(v, u);
        }
        push_u16(v, 0);
    }
    fn pad4(v: &mut Vec<u8>) {
        while !v.len().is_multiple_of(4) {
            v.push(0);
        }
    }

    #[test]
    fn parses_minimal_versioninfo() {
        // String 块："CompanyName" = "ACME"
        let mut s = Vec::new();
        push_u16(&mut s, 0); // wLength 占位
        push_u16(&mut s, 5); // wValueLength（字符数含 NUL）
        push_u16(&mut s, 1); // 文本
        push_utf16z(&mut s, "CompanyName");
        pad4(&mut s);
        push_utf16z(&mut s, "ACME");
        let s_len = s.len() as u16;
        s[0..2].copy_from_slice(&s_len.to_le_bytes());
        pad4(&mut s);

        // StringTable "040904b0"
        let mut st = Vec::new();
        push_u16(&mut st, 0);
        push_u16(&mut st, 0);
        push_u16(&mut st, 1);
        push_utf16z(&mut st, "040904b0");
        pad4(&mut st);
        st.extend_from_slice(&s);
        let st_len = st.len() as u16;
        st[0..2].copy_from_slice(&st_len.to_le_bytes());
        pad4(&mut st);

        // StringFileInfo
        let mut sfi = Vec::new();
        push_u16(&mut sfi, 0);
        push_u16(&mut sfi, 0);
        push_u16(&mut sfi, 1);
        push_utf16z(&mut sfi, "StringFileInfo");
        pad4(&mut sfi);
        sfi.extend_from_slice(&st);
        let sfi_len = sfi.len() as u16;
        sfi[0..2].copy_from_slice(&sfi_len.to_le_bytes());

        // 根块 + VS_FIXEDFILEINFO
        let mut root = Vec::new();
        push_u16(&mut root, 0);
        push_u16(&mut root, 52);
        push_u16(&mut root, 0);
        push_utf16z(&mut root, "VS_VERSION_INFO");
        pad4(&mut root);
        let mut fixed = vec![0u8; 52];
        fixed[0..4].copy_from_slice(&0xfeef_04bdu32.to_le_bytes());
        fixed[8..12].copy_from_slice(&0x0001_0002u32.to_le_bytes()); // file 1.2
        fixed[12..16].copy_from_slice(&0x0003_0004u32.to_le_bytes()); // .3.4
        fixed[16..20].copy_from_slice(&0x0005_0000u32.to_le_bytes()); // product 5.0
        fixed[20..24].copy_from_slice(&0u32.to_le_bytes());
        root.extend_from_slice(&fixed);
        pad4(&mut root);
        root.extend_from_slice(&sfi);
        let root_len = root.len() as u16;
        root[0..2].copy_from_slice(&root_len.to_le_bytes());

        let info = parse_vs_versioninfo(&root).expect("parses");
        assert_eq!(info.file_version.as_deref(), Some("1.2.3.4"));
        assert_eq!(info.product_version.as_deref(), Some("5.0.0.0"));
        assert_eq!(info.strings.len(), 1);
        assert_eq!(info.strings[0].key, "CompanyName");
        assert_eq!(info.strings[0].value, "ACME");
    }
}
