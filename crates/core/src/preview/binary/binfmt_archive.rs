//! Unix `ar` 归档（`.a` 静态库 / MSVC 导入库 / `.deb` 控制档）→ 「原类型视图」
//! 结构化报告，基于 [`goblin`]。列出成员（名称 / 大小 / 数据偏移 / 导出符号数 /
//! 按魔数猜测的成员格式）与全局符号总数。
//!
//! Magika 无 ar 标签，归类靠扩展名 + `!<arch>\n` 字节门（见前端 fileCategory）。

use goblin::archive::Archive;
use serde::Serialize;

/// 成员数上限（防御损坏归档）。
const MAX_MEMBERS: usize = 4096;

#[derive(Debug, Serialize)]
pub struct ArchiveReport {
    pub format: String,
    pub member_count: usize,
    pub total_symbols: usize,
    pub members: Vec<ArchiveMember>,
    pub file_size: u64,
    pub truncated: bool,
    pub parse_warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ArchiveMember {
    pub name: String,
    pub size: u64,
    /// 成员数据在归档内的文件偏移（十六进制字符串）。
    pub data_offset: String,
    /// 该成员导出的符号数（来自归档符号索引）。
    pub symbol_count: usize,
    /// 按成员首字节魔数猜测的格式（ELF / Mach-O / PE / 其它）。
    pub member_format: String,
}

/// 按成员首字节魔数粗略分类（ar 成员通常是目标文件）。
fn classify_member(data: &[u8], offset: u64) -> String {
    let Some(head) = data.get(offset as usize..) else {
        return "—".into();
    };
    if head.starts_with(b"\x7fELF") {
        "ELF 目标".into()
    } else if head.starts_with(&[0xfe, 0xed, 0xfa, 0xce])
        || head.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])
        || head.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || head.starts_with(&[0xce, 0xfa, 0xed, 0xfe])
    {
        "Mach-O 目标".into()
    } else if head.starts_with(&[0xca, 0xfe, 0xba, 0xbe]) {
        "Mach-O Fat".into()
    } else if head.starts_with(b"MZ") {
        "PE".into()
    } else if head.len() >= 2
        && matches!(
            u16::from_le_bytes([head[0], head[1]]),
            0x014c | 0x8664 | 0xaa64
        )
    {
        "COFF 目标".into()
    } else {
        "目标文件 / 数据".into()
    }
}

pub fn build(archive: &Archive, data: &[u8], file_size: u64) -> ArchiveReport {
    let mut warnings = Vec::new();
    let summary = archive.summarize();
    let member_count = summary.len();
    let total_symbols: usize = summary.iter().map(|(_, _, syms)| syms.len()).sum();

    if member_count > MAX_MEMBERS {
        warnings.push(format!(
            "成员数 {member_count} 超过上限 {MAX_MEMBERS}，仅展示前 {MAX_MEMBERS} 个"
        ));
    }

    let members = summary
        .into_iter()
        .take(MAX_MEMBERS)
        .map(|(name, member, syms)| ArchiveMember {
            name: name.to_string(),
            size: member.size() as u64,
            data_offset: format!("0x{:x}", member.offset),
            symbol_count: syms.len(),
            member_format: classify_member(data, member.offset),
        })
        .collect();

    let truncated = file_size > data.len() as u64;
    if truncated {
        warnings.push(format!(
            "仅读取了文件前 {} 字节（共 {file_size} 字节），成员格式判定基于已读部分",
            data.len(),
        ));
    }

    ArchiveReport {
        format: "Unix ar 归档".into(),
        member_count,
        total_symbols,
        members,
        file_size,
        truncated,
        parse_warnings: warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goblin::Object;

    /// 手工构造一个最小 ar 归档：magic + 一个空成员（名为 `foo.txt`）。
    /// 成员头 60 字节：name(16) mtime(12) uid(6) gid(6) mode(8) size(10) 终止符(2)。
    fn minimal_ar() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"!<arch>\n");
        let mut hdr = [b' '; 60];
        hdr[..8].copy_from_slice(b"foo.txt/"); // SysV/GNU 短名以 `/` 结尾
        hdr[16] = b'0'; // mtime
        hdr[28] = b'0'; // uid
        hdr[34] = b'0'; // gid
        hdr[40] = b'0'; // mode
        hdr[48] = b'0'; // size = 0
        hdr[58..60].copy_from_slice(b"`\n"); // 0x60 0x0a 终止符
        d.extend_from_slice(&hdr);
        d
    }

    #[test]
    fn parses_minimal_ar() {
        let d = minimal_ar();
        let Object::Archive(ar) = Object::parse(&d).expect("parse") else {
            panic!("not archive");
        };
        let report = build(&ar, &d, d.len() as u64);
        assert_eq!(report.format, "Unix ar 归档");
        assert_eq!(report.member_count, 1);
        assert_eq!(report.members[0].name, "foo.txt");
    }
}
