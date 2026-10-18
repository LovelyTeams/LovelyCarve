//! Mach-O（macOS / iOS 可执行格式）→ 「原类型视图」结构化报告，基于 [`goblin`]。
//!
//! 取证向：CPU 架构、文件类型、PIE/两级命名空间等标志、入口、LC_UUID、最低系统
//! 版本、依赖 dylib、加载命令分布、段/节布局（含每节 Shannon 熵）、符号摘要、
//! 代码签名（LC_CODE_SIGNATURE）与加密（LC_ENCRYPTION_INFO）存在性。通用二进制
//! （Fat）列出全部架构，仅深解析第一个。

use goblin::mach::{
    constants::cputype,
    header,
    load_command::{self, CommandVariant},
    Mach, MachO, MultiArch, SingleArch,
};
use serde::Serialize;

use crate::core::preview::binary::binfmt_util::EntropyBudget;

const MAX_SEGMENTS: usize = 128;
const MAX_SECTIONS: usize = 512;
const MAX_LIBS: usize = 256;
const MAX_ARCHES: usize = 16;
/// 符号计数迭代上限，防御损坏的符号表无限迭代。
const MAX_SYM_SCAN: usize = 200_000;

#[derive(Debug, Serialize)]
pub struct MachoReport {
    pub format: String,
    pub is_fat: bool,
    /// 通用二进制的全部架构（含未深解析的）。
    pub arches: Vec<MachoArch>,
    pub cpu: u32,
    pub cpu_label: String,
    pub filetype: u32,
    pub filetype_label: String,
    pub flags: Vec<String>,
    /// 入口地址（十六进制字符串）。
    pub entry: String,
    pub uuid: Option<String>,
    pub min_os: Option<String>,
    pub libs: Vec<String>,
    pub load_commands: Vec<MachoLoadCmd>,
    pub segments: Vec<MachoSegment>,
    pub symbol_summary: MachoSymbolSummary,
    pub code_signature: bool,
    pub encrypted: bool,
    pub file_size: u64,
    pub truncated: bool,
    pub parse_warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct MachoArch {
    pub cpu_label: String,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct MachoLoadCmd {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct MachoSegment {
    pub name: String,
    /// 虚拟地址（十六进制字符串）。
    pub vmaddr: String,
    pub vmsize: u64,
    pub fileoff: u64,
    pub filesize: u64,
    pub initprot: String,
    pub maxprot: String,
    pub nsects: u32,
    pub sections: Vec<MachoSection>,
}

#[derive(Debug, Serialize)]
pub struct MachoSection {
    pub name: String,
    pub segname: String,
    /// 节地址（十六进制字符串）。
    pub addr: String,
    pub size: u64,
    pub offset: u64,
    pub entropy: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct MachoSymbolSummary {
    pub total: usize,
    pub defined: usize,
    pub undefined: usize,
    pub stripped: bool,
    pub truncated: bool,
}

fn vm_perms(prot: u32) -> String {
    format!(
        "{}{}{}",
        if prot & 1 != 0 { 'r' } else { '-' },
        if prot & 2 != 0 { 'w' } else { '-' },
        if prot & 4 != 0 { 'x' } else { '-' },
    )
}

/// (cputype, cpusubtype) → 架构名。先按原值匹配，未命中再掩掉能力位（高 8 位，
/// 如 arm64e / x86_64h 的 CPU_SUBTYPE_MASK）重试，最后退回十六进制。
fn arch_label(cpu: u32, subtype: u32) -> String {
    cputype::get_arch_name_from_types(cpu, subtype)
        .or_else(|| cputype::get_arch_name_from_types(cpu, subtype & 0x00ff_ffff))
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("cpu 0x{cpu:x}"))
}

fn header_flags(flags: u32) -> Vec<String> {
    const F: &[(u32, &str)] = &[
        (header::MH_PIE, "PIE"),
        (header::MH_TWOLEVEL, "TWO_LEVEL"),
        (header::MH_DYLDLINK, "DYLDLINK"),
        (header::MH_NO_HEAP_EXECUTION, "NO_HEAP_EXEC"),
    ];
    F.iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, n)| (*n).to_string())
        .collect()
}

fn fmt_packed_version(v: u32) -> String {
    format!("{}.{}.{}", (v >> 16) & 0xffff, (v >> 8) & 0xff, v & 0xff)
}

fn platform_label(platform: u32) -> &'static str {
    match platform {
        1 => "macOS",
        2 => "iOS",
        3 => "tvOS",
        4 => "watchOS",
        6 => "Mac Catalyst",
        7 => "iOS 模拟器",
        other => {
            let _ = other;
            "未知平台"
        }
    }
}

pub fn build(mach: &Mach, data: &[u8], file_size: u64) -> MachoReport {
    match mach {
        Mach::Binary(m) => macho_report(m, data, file_size, false, Vec::new(), Vec::new()),
        Mach::Fat(multi) => {
            let mut warnings = Vec::new();
            let arches = collect_arches(multi, &mut warnings);
            match multi.get(0) {
                Ok(SingleArch::MachO(m)) => {
                    let mut r = macho_report(&m, data, file_size, true, arches, warnings);
                    r.format = "Mach-O 通用二进制（Fat）".to_string();
                    r
                }
                Ok(SingleArch::Archive(_)) => {
                    warnings.push("通用二进制首架构为静态库归档，未展开 Mach-O 字段".into());
                    fat_stub(arches, file_size, warnings)
                }
                Err(e) => {
                    warnings.push(format!("展开通用二进制首架构失败：{e}"));
                    fat_stub(arches, file_size, warnings)
                }
            }
        }
    }
}

fn collect_arches(multi: &MultiArch, warnings: &mut Vec<String>) -> Vec<MachoArch> {
    let arches = match multi.arches() {
        Ok(a) => a,
        Err(e) => {
            warnings.push(format!("读取通用二进制架构表失败：{e}"));
            return Vec::new();
        }
    };
    if arches.len() > MAX_ARCHES {
        warnings.push(format!(
            "架构数 {} 超过上限 {MAX_ARCHES}，仅展示前 {MAX_ARCHES} 个",
            arches.len()
        ));
    }
    let total = arches.len();
    let out: Vec<MachoArch> = arches
        .into_iter()
        .take(MAX_ARCHES)
        .map(|a| MachoArch {
            cpu_label: arch_label(a.cputype, a.cpusubtype),
            offset: u64::from(a.offset),
            size: u64::from(a.size),
        })
        .collect();
    if total > 1 {
        warnings.push(format!(
            "通用二进制含 {total} 个架构，仅展开第 1 个的详细结构"
        ));
    }
    out
}

fn fat_stub(arches: Vec<MachoArch>, file_size: u64, warnings: Vec<String>) -> MachoReport {
    MachoReport {
        format: "Mach-O 通用二进制（Fat）".to_string(),
        is_fat: true,
        arches,
        cpu: 0,
        cpu_label: "—".to_string(),
        filetype: 0,
        filetype_label: "—".to_string(),
        flags: Vec::new(),
        entry: "0x0".to_string(),
        uuid: None,
        min_os: None,
        libs: Vec::new(),
        load_commands: Vec::new(),
        segments: Vec::new(),
        symbol_summary: MachoSymbolSummary {
            total: 0,
            defined: 0,
            undefined: 0,
            stripped: true,
            truncated: false,
        },
        code_signature: false,
        encrypted: false,
        file_size,
        // 未深解析首架构（归档或出错），但这不是「读取被上限截断」——保持 false。
        truncated: false,
        parse_warnings: warnings,
    }
}

fn macho_report(
    m: &MachO,
    data: &[u8],
    file_size: u64,
    is_fat: bool,
    arches: Vec<MachoArch>,
    mut warnings: Vec<String>,
) -> MachoReport {
    let h = &m.header;

    // 加载命令：分组计数 + 扫描感兴趣的命令
    let mut lc_order: Vec<&'static str> = Vec::new();
    let mut lc_count: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
    let mut uuid = None;
    let mut min_os = None;
    let mut code_signature = false;
    let mut encrypted = false;
    for lc in &m.load_commands {
        let name = load_command::cmd_to_str(lc.command.cmd());
        if !lc_count.contains_key(name) {
            lc_order.push(name);
        }
        *lc_count.entry(name).or_insert(0) += 1;
        match &lc.command {
            CommandVariant::Uuid(u) => {
                uuid = Some(
                    u.uuid
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(""),
                );
            }
            CommandVariant::CodeSignature(_) => code_signature = true,
            CommandVariant::EncryptionInfo32(e) => encrypted = encrypted || e.cryptid != 0,
            CommandVariant::EncryptionInfo64(e) => encrypted = encrypted || e.cryptid != 0,
            CommandVariant::VersionMinMacosx(v)
            | CommandVariant::VersionMinIphoneos(v)
            | CommandVariant::VersionMinTvos(v)
            | CommandVariant::VersionMinWatchos(v) => {
                if min_os.is_none() {
                    min_os = Some(fmt_packed_version(v.version));
                }
            }
            CommandVariant::BuildVersion(b) => {
                min_os = Some(format!(
                    "{} {}",
                    platform_label(b.platform),
                    fmt_packed_version(b.minos)
                ));
            }
            _ => {}
        }
    }
    let load_commands = lc_order
        .into_iter()
        .map(|name| MachoLoadCmd {
            name: name.to_string(),
            count: lc_count.get(name).copied().unwrap_or(0),
        })
        .collect();

    // 段与节（节熵直接取 goblin 给出的节数据切片）
    let mut budget = EntropyBudget::new();
    let mut segments = Vec::new();
    for seg in m.segments.iter().take(MAX_SEGMENTS) {
        let seg_name = seg
            .name()
            .map(|s| s.to_string())
            .unwrap_or_else(|_| lossy_fixed(&seg.segname));
        let mut sections = Vec::new();
        match seg.sections() {
            Ok(secs) => {
                for (sect, sdata) in secs.into_iter().take(MAX_SECTIONS) {
                    let name = sect
                        .name()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|_| lossy_fixed(&sect.sectname));
                    sections.push(MachoSection {
                        name,
                        segname: lossy_fixed(&sect.segname),
                        addr: format!("0x{:x}", sect.addr),
                        size: sect.size,
                        offset: u64::from(sect.offset),
                        entropy: budget.entropy(sdata),
                    });
                }
            }
            Err(e) => warnings.push(format!("段 {seg_name} 的节解析失败：{e}")),
        }
        segments.push(MachoSegment {
            name: seg_name,
            vmaddr: format!("0x{:x}", seg.vmaddr),
            vmsize: seg.vmsize,
            fileoff: seg.fileoff,
            filesize: seg.filesize,
            initprot: vm_perms(seg.initprot),
            maxprot: vm_perms(seg.maxprot),
            nsects: seg.nsects,
            sections,
        });
    }
    if m.segments.len() > MAX_SEGMENTS {
        warnings.push(format!(
            "段数 {} 超过上限 {MAX_SEGMENTS}，仅展示前 {MAX_SEGMENTS} 个",
            m.segments.len()
        ));
    }
    if budget.exhausted() {
        warnings.push("节总数据量超过熵计算预算，部分节未计算熵".into());
    }

    let symbol_summary = build_symbol_summary(m, &mut warnings);

    let libs: Vec<String> = m
        .libs
        .iter()
        .filter(|l| **l != "self")
        .take(MAX_LIBS)
        .map(|s| s.to_string())
        .collect();

    let truncated = file_size > data.len() as u64;
    if truncated {
        warnings.push(format!(
            "仅读取了文件前 {} 字节（共 {file_size} 字节），节熵等统计基于已读部分",
            data.len(),
        ));
    }

    MachoReport {
        format: if h.magic == header::MH_MAGIC_64 {
            "Mach-O 64".into()
        } else {
            "Mach-O 32".into()
        },
        is_fat,
        arches,
        cpu: h.cputype,
        cpu_label: arch_label(h.cputype(), h.cpusubtype()),
        filetype: h.filetype,
        filetype_label: header::filetype_to_str(h.filetype).to_string(),
        flags: header_flags(h.flags),
        entry: format!("0x{:x}", m.entry),
        uuid,
        min_os,
        libs,
        load_commands,
        segments,
        symbol_summary,
        code_signature,
        encrypted,
        file_size,
        truncated,
        parse_warnings: warnings,
    }
}

fn build_symbol_summary(m: &MachO, warnings: &mut Vec<String>) -> MachoSymbolSummary {
    let (mut total, mut defined, mut undefined) = (0usize, 0usize, 0usize);
    let mut sym_truncated = false;
    for sym in m.symbols() {
        match sym {
            Ok((_, nlist)) => {
                if nlist.is_stab() {
                    continue;
                }
                total += 1;
                if nlist.is_undefined() {
                    undefined += 1;
                } else {
                    defined += 1;
                }
                if total >= MAX_SYM_SCAN {
                    sym_truncated = true;
                    break;
                }
            }
            Err(e) => {
                warnings.push(format!("符号表解析中断：{e}"));
                break;
            }
        }
    }
    MachoSymbolSummary {
        total,
        defined,
        undefined,
        stripped: total == 0,
        truncated: sym_truncated,
    }
}

/// 16 字节定长名（段/节名）→ 去尾零的 lossy UTF-8。
fn lossy_fixed(raw: &[u8; 16]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use goblin::Object;

    /// 手工构造一个最小 Mach-O 64（x86-64，EXECUTE），仅头部、零加载命令。
    fn minimal_macho64() -> Vec<u8> {
        let mut d = vec![0u8; 32];
        d[0..4].copy_from_slice(&header::MH_MAGIC_64.to_le_bytes());
        d[4..8].copy_from_slice(&cputype::CPU_TYPE_X86_64.to_le_bytes()); // cputype
        d[8..12].copy_from_slice(&3u32.to_le_bytes()); // cpusubtype (CPU_SUBTYPE_X86_64_ALL)
        d[12..16].copy_from_slice(&header::MH_EXECUTE.to_le_bytes()); // filetype
        d[16..20].copy_from_slice(&0u32.to_le_bytes()); // ncmds
        d[20..24].copy_from_slice(&0u32.to_le_bytes()); // sizeofcmds
        d[24..28].copy_from_slice(&header::MH_PIE.to_le_bytes()); // flags
        d
    }

    #[test]
    fn parses_minimal_macho64() {
        let d = minimal_macho64();
        let Object::Mach(mach) = Object::parse(&d).expect("parse") else {
            panic!("not mach");
        };
        let report = build(&mach, &d, d.len() as u64);
        assert!(!report.is_fat);
        assert_eq!(report.format, "Mach-O 64");
        assert_eq!(report.cpu_label, "x86_64");
        assert_eq!(report.filetype_label, "EXECUTE");
        assert!(report.flags.contains(&"PIE".to_string()));
        assert!(report.symbol_summary.stripped);
    }
}
