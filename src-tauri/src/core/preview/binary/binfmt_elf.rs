//! ELF（可执行与可链接格式）→ 「原类型视图」结构化报告，基于 [`goblin`] 解析。
//!
//! Linux 镜像里占主导的可执行格式。取证向：机器/ABI、文件类型、PIE、解释器、
//! soname、依赖库（DT_NEEDED）、rpath/runpath（常硬编码私有路径）、GNU build-id
//! （PT_NOTE）、安全缓解（RELRO / NX 栈 / PIE / BIND_NOW / 符号剥离）、段/节
//! 布局（含每节 Shannon 熵）、符号摘要（导入/导出/是否 stripped）。
//!
//! 大地址（入口、段虚拟地址、节地址）序列化为十六进制字符串，避免超出 JS 安全
//! 整数；文件偏移/大小为数值。

use goblin::elf::{dynamic, header, note, program_header as ph, section_header as sh, sym, Elf};
use serde::Serialize;

use crate::core::preview::binary::binfmt_util::EntropyBudget;

const MAX_SECTIONS: usize = 256;
const MAX_PHDRS: usize = 128;
const MAX_NEEDED: usize = 256;
/// 导入/导出符号各自的列举上限（超出截断并置 *_truncated）。
const MAX_LISTED_SYMS: usize = 512;

#[derive(Debug, Serialize)]
pub struct ElfReport {
    /// `"ELF32"` / `"ELF64"`。
    pub format: String,
    pub endianness: String,
    pub machine: u16,
    pub machine_label: String,
    pub os_abi: u8,
    pub os_abi_label: String,
    pub elf_type: String,
    pub is_pie: bool,
    /// 入口地址（十六进制字符串）。
    pub entry: String,
    pub interpreter: Option<String>,
    pub soname: Option<String>,
    /// DT_NEEDED 依赖库。
    pub needed: Vec<String>,
    pub rpaths: Vec<String>,
    pub runpaths: Vec<String>,
    /// GNU build-id（十六进制），来自 PT_NOTE / .note.gnu.build-id。
    pub build_id: Option<String>,
    /// 安全缓解标签（RELRO / NX / PIE / BIND_NOW / stripped）。
    pub security_flags: Vec<String>,
    pub program_headers: Vec<ElfSegment>,
    pub sections: Vec<ElfSection>,
    pub symbol_summary: ElfSymbolSummary,
    pub file_size: u64,
    pub truncated: bool,
    pub parse_warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ElfSegment {
    pub ptype: String,
    pub perms: String,
    pub offset: u64,
    /// 虚拟地址（十六进制字符串）。
    pub vaddr: String,
    pub filesz: u64,
    pub memsz: u64,
}

#[derive(Debug, Serialize)]
pub struct ElfSection {
    pub name: String,
    pub sh_type: String,
    pub flags: String,
    /// 节地址（十六进制字符串）。
    pub addr: String,
    pub offset: u64,
    pub size: u64,
    pub entropy: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ElfSymbolSummary {
    pub dynsym_count: usize,
    pub symtab_count: usize,
    pub stripped: bool,
    pub exported: Vec<String>,
    pub imported: Vec<String>,
    pub exported_truncated: bool,
    pub imported_truncated: bool,
}

fn machine_label(machine: u16) -> String {
    match machine {
        header::EM_NONE => "未知（0）".into(),
        header::EM_386 => "x86 (i386)".into(),
        header::EM_X86_64 => "x64 (AMD64)".into(),
        header::EM_ARM => "ARM".into(),
        header::EM_AARCH64 => "ARM64 (AArch64)".into(),
        header::EM_RISCV => "RISC-V".into(),
        header::EM_PPC => "PowerPC".into(),
        header::EM_PPC64 => "PowerPC64".into(),
        header::EM_MIPS => "MIPS".into(),
        header::EM_S390 => "S/390".into(),
        header::EM_SPARC => "SPARC".into(),
        header::EM_IA_64 => "Itanium (IA64)".into(),
        header::EM_LOONGARCH => "LoongArch".into(),
        other => format!("0x{other:04x}"),
    }
}

fn os_abi_label(abi: u8) -> String {
    match abi {
        header::ELFOSABI_SYSV => "System V".into(),
        header::ELFOSABI_HPUX => "HP-UX".into(),
        header::ELFOSABI_NETBSD => "NetBSD".into(),
        header::ELFOSABI_GNU => "Linux / GNU".into(),
        header::ELFOSABI_SOLARIS => "Solaris".into(),
        header::ELFOSABI_AIX => "AIX".into(),
        header::ELFOSABI_FREEBSD => "FreeBSD".into(),
        header::ELFOSABI_OPENBSD => "OpenBSD".into(),
        header::ELFOSABI_ARM => "ARM".into(),
        header::ELFOSABI_STANDALONE => "Standalone / 嵌入式".into(),
        other => format!("0x{other:02x}"),
    }
}

fn elf_type_label(e_type: u16) -> String {
    match e_type {
        header::ET_REL => "可重定位目标（REL）".into(),
        header::ET_EXEC => "可执行（EXEC）".into(),
        header::ET_DYN => "共享对象 / PIE（DYN）".into(),
        header::ET_CORE => "核心转储（CORE）".into(),
        header::ET_NONE => "无（NONE）".into(),
        other => format!("{} (0x{other:x})", header::et_to_str(other)),
    }
}

pub fn build(elf: &Elf, data: &[u8], file_size: u64) -> ElfReport {
    let mut warnings: Vec<String> = Vec::new();
    let h = &elf.header;

    let is_pie = h.e_type == header::ET_DYN
        && (elf.interpreter.is_some()
            || elf
                .dynamic
                .as_ref()
                .is_some_and(|d| d.info.flags_1 & dynamic::DF_1_PIE != 0));

    // 安全缓解
    let mut security_flags = Vec::new();
    let mut has_gnu_stack = false;
    let mut nx_stack = true;
    let mut relro = false;
    for p in &elf.program_headers {
        match p.p_type {
            ph::PT_GNU_STACK => {
                has_gnu_stack = true;
                if p.is_executable() {
                    nx_stack = false;
                }
            }
            ph::PT_GNU_RELRO => relro = true,
            _ => {}
        }
    }
    let bind_now = elf.dynamic.as_ref().is_some_and(|d| {
        d.info.flags & dynamic::DF_BIND_NOW != 0 || d.info.flags_1 & dynamic::DF_1_NOW != 0
    });
    if is_pie {
        security_flags.push("PIE".into());
    }
    if has_gnu_stack {
        security_flags.push(if nx_stack {
            "NX（不可执行栈）".into()
        } else {
            "可执行栈（危险）".into()
        });
    }
    if relro {
        security_flags.push(if bind_now {
            "Full RELRO".into()
        } else {
            "Partial RELRO".into()
        });
    } else if bind_now {
        security_flags.push("BIND_NOW".into());
    }
    let stripped = elf.syms.is_empty();
    if stripped {
        security_flags.push("已剥离符号（stripped）".into());
    }

    // build-id（GNU note）
    let build_id = extract_build_id(elf, data, &mut warnings);

    // 段
    let mut program_headers = Vec::new();
    for p in elf.program_headers.iter().take(MAX_PHDRS) {
        let perms = format!(
            "{}{}{}",
            if p.is_read() { 'r' } else { '-' },
            if p.is_write() { 'w' } else { '-' },
            if p.is_executable() { 'x' } else { '-' },
        );
        program_headers.push(ElfSegment {
            ptype: ph::pt_to_str(p.p_type).to_string(),
            perms,
            offset: p.p_offset,
            vaddr: format!("0x{:x}", p.p_vaddr),
            filesz: p.p_filesz,
            memsz: p.p_memsz,
        });
    }
    if elf.program_headers.len() > MAX_PHDRS {
        warnings.push(format!(
            "段数 {} 超过上限 {MAX_PHDRS}，仅展示前 {MAX_PHDRS} 个",
            elf.program_headers.len()
        ));
    }

    // 节（含熵）
    let mut sections = Vec::new();
    let mut budget = EntropyBudget::new();
    for s in elf.section_headers.iter().take(MAX_SECTIONS) {
        let name = elf.shdr_strtab.get_at(s.sh_name).unwrap_or("").to_string();
        let flags = format!(
            "{}{}{}",
            if s.sh_flags & u64::from(sh::SHF_ALLOC) != 0 {
                'a'
            } else {
                '-'
            },
            if s.sh_flags & u64::from(sh::SHF_WRITE) != 0 {
                'w'
            } else {
                '-'
            },
            if s.sh_flags & u64::from(sh::SHF_EXECINSTR) != 0 {
                'x'
            } else {
                '-'
            },
        );
        let entropy = if s.sh_type == sh::SHT_NOBITS {
            None
        } else {
            data.get(
                s.sh_offset as usize..(s.sh_offset as usize).saturating_add(s.sh_size as usize),
            )
            .and_then(|slice| budget.entropy(slice))
        };
        sections.push(ElfSection {
            name,
            sh_type: sh::sht_to_str(s.sh_type).to_string(),
            flags,
            addr: format!("0x{:x}", s.sh_addr),
            offset: s.sh_offset,
            size: s.sh_size,
            entropy,
        });
    }
    if elf.section_headers.len() > MAX_SECTIONS {
        warnings.push(format!(
            "节数 {} 超过上限 {MAX_SECTIONS}，仅展示前 {MAX_SECTIONS} 个",
            elf.section_headers.len()
        ));
    }
    if budget.exhausted() {
        warnings.push("节总数据量超过熵计算预算，部分节未计算熵".into());
    }

    let symbol_summary = build_symbol_summary(elf, stripped);

    let needed: Vec<String> = elf
        .libraries
        .iter()
        .take(MAX_NEEDED)
        .map(|s| s.to_string())
        .collect();

    let truncated = file_size > data.len() as u64;
    if truncated {
        warnings.push(format!(
            "仅读取了文件前 {} 字节（共 {file_size} 字节），节熵等统计基于已读部分",
            data.len(),
        ));
    }

    ElfReport {
        format: if elf.is_64 { "ELF64" } else { "ELF32" }.to_string(),
        endianness: if elf.little_endian {
            "小端（LSB）".into()
        } else {
            "大端（MSB）".into()
        },
        machine: h.e_machine,
        machine_label: machine_label(h.e_machine),
        os_abi: h.e_ident[header::EI_OSABI],
        os_abi_label: os_abi_label(h.e_ident[header::EI_OSABI]),
        elf_type: elf_type_label(h.e_type),
        is_pie,
        entry: format!("0x{:x}", elf.entry),
        interpreter: elf.interpreter.map(|s| s.to_string()),
        soname: elf.soname.map(|s| s.to_string()),
        needed,
        rpaths: elf.rpaths.iter().map(|s| s.to_string()).collect(),
        runpaths: elf.runpaths.iter().map(|s| s.to_string()).collect(),
        build_id,
        security_flags,
        program_headers,
        sections,
        symbol_summary,
        file_size,
        truncated,
        parse_warnings: warnings,
    }
}

fn extract_build_id(elf: &Elf, data: &[u8], warnings: &mut Vec<String>) -> Option<String> {
    let iter = elf.iter_note_headers(data)?;
    for note in iter {
        match note {
            Ok(n)
                if n.n_type == note::NT_GNU_BUILD_ID && n.name.trim_end_matches('\0') == "GNU" =>
            {
                return Some(hex::encode(n.desc));
            }
            Ok(_) => {}
            Err(e) => {
                warnings.push(format!("note 段解析中断：{e}"));
                break;
            }
        }
    }
    None
}

fn build_symbol_summary(elf: &Elf, stripped: bool) -> ElfSymbolSummary {
    let mut exported = Vec::new();
    let mut imported = Vec::new();
    let mut exported_truncated = false;
    let mut imported_truncated = false;
    for s in elf.dynsyms.iter() {
        let Some(name) = elf.dynstrtab.get_at(s.st_name) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        if s.is_import() {
            if imported.len() < MAX_LISTED_SYMS {
                imported.push(name.to_string());
            } else {
                imported_truncated = true;
            }
        } else if s.st_shndx != 0 && s.st_bind() == sym::STB_GLOBAL {
            if exported.len() < MAX_LISTED_SYMS {
                exported.push(name.to_string());
            } else {
                exported_truncated = true;
            }
        }
    }
    ElfSymbolSummary {
        dynsym_count: elf.dynsyms.len(),
        symtab_count: elf.syms.len(),
        stripped,
        exported,
        imported,
        exported_truncated,
        imported_truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goblin::Object;

    /// 手工构造一个最小 ELF64（小端，x86-64，DYN/PIE），只含头部。
    fn minimal_elf64() -> Vec<u8> {
        let mut d = vec![0u8; 64];
        d[0..4].copy_from_slice(b"\x7fELF");
        d[4] = header::ELFCLASS64;
        d[5] = header::ELFDATA2LSB;
        d[6] = 1; // EV_CURRENT
        d[7] = header::ELFOSABI_GNU;
        d[16..18].copy_from_slice(&header::ET_DYN.to_le_bytes()); // e_type
        d[18..20].copy_from_slice(&header::EM_X86_64.to_le_bytes()); // e_machine
        d[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
        d[24..32].copy_from_slice(&0x1040u64.to_le_bytes()); // e_entry
        d[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
        d
    }

    #[test]
    fn parses_minimal_elf64() {
        let d = minimal_elf64();
        let Object::Elf(elf) = Object::parse(&d).expect("parse") else {
            panic!("not elf");
        };
        let report = build(&elf, &d, d.len() as u64);
        assert_eq!(report.format, "ELF64");
        assert_eq!(report.machine_label, "x64 (AMD64)");
        assert_eq!(report.os_abi_label, "Linux / GNU");
        assert_eq!(report.entry, "0x1040");
        assert!(report.endianness.contains("小端"));
        assert!(report.symbol_summary.stripped);
    }
}
