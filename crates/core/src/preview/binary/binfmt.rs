//! 二进制「原类型视图」的统一入口：按文件魔数把镜像内任意文件分派到 ELF /
//! Mach-O / PE / Unix ar 的结构化解析器（全部基于 [`goblin`]）。
//!
//! 分派用 goblin 的 [`peek_bytes`](goblin::peek_bytes) 取 16 字节魔数提示，再交给
//! 对应子模块。PE 走 [`binfmt_pe`](crate::preview::binary::binfmt_pe) 的 `Permissive` 路径而非
//! `Object::parse` 的 `Strict` PE 分支，以容忍截断/敌意 PE（取证场景常见）。返回值
//! 是带 `kind` 标签的联合，前端按 `kind` 选子视图渲染。

use goblin::{archive, elf, mach, Hint};
use serde::Serialize;

pub use crate::preview::binary::binfmt_archive::ArchiveReport;
pub use crate::preview::binary::binfmt_elf::ElfReport;
pub use crate::preview::binary::binfmt_macho::MachoReport;
pub use crate::preview::binary::binfmt_pe::PeReport;

/// 四类二进制的结构化报告联合。`#[serde(tag = "kind")]` 使前端可按
/// `report.kind`（`"elf"` / `"macho"` / `"pe"` / `"archive"`）分派子视图。
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum NativeReport {
    Elf(ElfReport),
    Macho(MachoReport),
    Pe(PeReport),
    Archive(ArchiveReport),
}

/// `data` 是文件前缀（可能被读取上限截断），`file_size` 是真实文件大小。无法识别
/// 为受支持的二进制格式时返回解释性 `Err`（前端直接展示）。
pub fn parse(data: &[u8], file_size: u64) -> anyhow::Result<NativeReport> {
    let hint_bytes: &[u8; 16] = data
        .get(0..16)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| anyhow::anyhow!("文件太小，无法识别二进制格式（不足 16 字节）"))?;
    let hint =
        goblin::peek_bytes(hint_bytes).map_err(|e| anyhow::anyhow!("二进制识别失败：{e}"))?;
    match hint {
        Hint::PE => Ok(NativeReport::Pe(crate::preview::binary::binfmt_pe::build(
            data, file_size,
        )?)),
        Hint::Elf(_) => {
            let elf = elf::Elf::parse(data).map_err(|e| anyhow::anyhow!("ELF 解析失败：{e}"))?;
            Ok(NativeReport::Elf(
                crate::preview::binary::binfmt_elf::build(&elf, data, file_size),
            ))
        }
        Hint::Mach(_) | Hint::MachFat(_) => {
            let mach =
                mach::Mach::parse(data).map_err(|e| anyhow::anyhow!("Mach-O 解析失败：{e}"))?;
            Ok(NativeReport::Macho(
                crate::preview::binary::binfmt_macho::build(&mach, data, file_size),
            ))
        }
        Hint::Archive => {
            let ar = archive::Archive::parse(data)
                .map_err(|e| anyhow::anyhow!("ar 归档解析失败：{e}"))?;
            Ok(NativeReport::Archive(
                crate::preview::binary::binfmt_archive::build(&ar, data, file_size),
            ))
        }
        Hint::TE => anyhow::bail!("TE（EFI Terse Executable）暂不支持结构化视图"),
        Hint::COFF => anyhow::bail!("COFF 目标文件（.obj）暂不支持结构化视图"),
        Hint::Unknown(magic) => {
            anyhow::bail!("无法识别为受支持的二进制格式（magic = 0x{magic:x}）")
        }
        // Hint 是 #[non_exhaustive]：goblin 未来新增格式时回到此处补显式分支。
        _ => anyhow::bail!("不支持的二进制格式"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_elf64() -> Vec<u8> {
        let mut d = vec![0u8; 64];
        d[0..4].copy_from_slice(b"\x7fELF");
        d[4] = 2; // ELFCLASS64
        d[5] = 1; // ELFDATA2LSB
        d[6] = 1;
        d[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
        d[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
        d[20..24].copy_from_slice(&1u32.to_le_bytes());
        d[52..54].copy_from_slice(&64u16.to_le_bytes());
        d
    }

    #[test]
    fn dispatch_rejects_too_small() {
        assert!(parse(b"MZ", 2).is_err());
    }

    #[test]
    fn dispatch_routes_elf() {
        let d = minimal_elf64();
        let report = parse(&d, d.len() as u64).expect("elf dispatch");
        assert!(matches!(report, NativeReport::Elf(_)));
    }

    #[test]
    fn dispatch_rejects_plain_text() {
        let d = b"#!/bin/sh\necho hello world, this is just a script\n";
        assert!(parse(d, d.len() as u64).is_err());
    }
}
