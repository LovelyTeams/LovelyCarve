//! Portable Executable (PE) → 「原类型视图」结构化报告，基于 [`goblin`] 解析。
//!
//! goblin 负责 DOS/COFF/可选头、数据目录、节区、导入/导出、调试目录（CodeView
//! PDB）、Authenticode 证书、CLR/.NET、资源树解析；本模块在其之上补齐 goblin 不
//! 计算的取证字段：**每节区 Shannon 熵**、**overlay**（末节区后的附加数据）、
//! **延迟导入**（goblin 的扁平 `imports` 不含）。版本资源沿用自带的 [`pe_rsrc`]
//! 解析器（goblin 的资源树无一次到位的版本访问器），由本模块从 goblin 已解析的
//! 节区表建 RVA→文件偏移映射喂入。
//!
//! 解析对取证场景保持容错：goblin 用 `Permissive` 模式，单个结构损坏只记入
//! `parse_warnings`，不影响其余字段；只有「根本不是 PE」才返回 `Err`。`PeReport`
//! 的字段与旧手写解析器逐一对齐，前端 `PeViewer` 无需改动。

use goblin::pe::{self, options::ParseMode};
use serde::Serialize;

use crate::core::preview::binary::binfmt_util::{cstr, ts_utc, u16le, u32le, u64le, EntropyBudget};
use crate::core::preview::binary::pe_rsrc;

/// 单个 DLL 的导入函数数量上限；导出表条目同限。超出截断并标记 total。
const MAX_FUNCS: usize = 1024;
/// 延迟导入 DLL 数量上限（防御损坏数据死循环）。
const MAX_DELAY_DLLS: usize = 256;
/// 调试目录条目上限。
const MAX_DEBUG_ENTRIES: usize = 16;

#[derive(Debug, Serialize)]
pub struct PeReport {
    /// `"PE32"` / `"PE32+"`。
    pub format: String,
    pub machine: u16,
    pub machine_label: String,
    pub timestamp: u32,
    pub timestamp_utc: Option<String>,
    pub characteristics: u16,
    pub characteristic_flags: Vec<String>,
    pub subsystem: u16,
    pub subsystem_label: String,
    pub dll_characteristics: u16,
    pub dll_characteristic_flags: Vec<String>,
    pub entry_point: u32,
    pub entry_section: Option<String>,
    /// 十六进制字符串（PE32+ 的 u64 会超出 JS 安全整数范围）。
    pub image_base: String,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub checksum: u32,
    pub linker_version: String,
    pub os_version: String,
    pub subsystem_version: String,
    pub is_dll: bool,
    pub is_dotnet: bool,
    /// Security 数据目录非空（存在 Authenticode 签名块；未验证签名本身）。
    pub is_signed: bool,
    pub data_directories: Vec<DataDirEntry>,
    pub sections: Vec<PeSection>,
    pub imports: Vec<PeImport>,
    pub exports: Option<PeExports>,
    pub debug: Vec<PeDebugEntry>,
    pub version_info: Option<pe_rsrc::PeVersionInfo>,
    pub overlay: Option<PeOverlay>,
    pub parse_warnings: Vec<String>,
    pub file_size: u64,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct DataDirEntry {
    pub name: String,
    pub rva: u32,
    pub size: u32,
}

#[derive(Debug, Serialize)]
pub struct PeSection {
    pub name: String,
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub characteristics: u32,
    /// 形如 `"r-x"`。
    pub perms: String,
    pub is_code: bool,
    /// Shannon 熵（0..8）。>7.2 通常意味着压缩/加壳/加密。
    pub entropy: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct PeImport {
    pub dll: String,
    /// 延迟导入（Delay-load）—— 恶意样本常用来隐藏依赖。
    pub delayed: bool,
    pub functions: Vec<String>,
    /// 实际函数总数；大于 `functions.len()` 表示已截断。
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct PeExports {
    pub dll_name: Option<String>,
    pub total: usize,
    pub entries: Vec<PeExportEntry>,
}

#[derive(Debug, Serialize)]
pub struct PeExportEntry {
    pub ordinal: u32,
    pub name: Option<String>,
    pub rva: u32,
    pub forwarder: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PeDebugEntry {
    pub kind: String,
    pub timestamp_utc: Option<String>,
    /// CodeView (RSDS) 才有：原始 PDB 路径，常泄露开发者用户名 / 工程路径。
    pub pdb_path: Option<String>,
    pub guid: Option<String>,
    pub age: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct PeOverlay {
    pub offset: u64,
    pub size: u64,
}

// ---- RVA → 文件偏移（从 goblin 节区表重建，供延迟导入 / 版本资源定位）-----

struct SectionMap {
    /// (virtual_address, virtual_size, raw_offset, raw_size)
    sections: Vec<(u32, u32, u32, u32)>,
    /// 头部区（RVA == 文件偏移）恒等映射的上限，截到最小节区文件偏移与文件长度。
    header_limit: u32,
}

impl SectionMap {
    fn rva_to_off(&self, rva: u32) -> Option<usize> {
        if rva == 0 {
            return None; // RVA 0 是「无」哨兵，绝不映射到 MZ 头
        }
        if rva < self.header_limit {
            return Some(rva as usize);
        }
        for &(va, vsize, raw_off, raw_size) in &self.sections {
            let span = vsize.max(raw_size);
            if rva >= va && rva.checked_sub(va)? < span {
                let delta = rva - va;
                if delta < raw_size {
                    return Some(raw_off.checked_add(delta)? as usize);
                }
                return None; // 落在未映射到文件的虚拟尾部（bss 等）
            }
        }
        None
    }
}

// ---- 标签映射（与旧手写解析器输出一致）----------------------------------

fn machine_label(machine: u16) -> String {
    match machine {
        0x014c => "x86 (i386)".into(),
        0x8664 => "x64 (AMD64)".into(),
        0xaa64 => "ARM64".into(),
        0x01c0 => "ARM".into(),
        0x01c4 => "ARM Thumb-2 (ARMNT)".into(),
        0x0200 => "Itanium (IA64)".into(),
        0x5064 => "RISC-V 64".into(),
        0x6264 => "LoongArch64".into(),
        0x0000 => "未知（0x0000）".into(),
        other => format!("0x{other:04x}"),
    }
}

fn subsystem_label(subsystem: u16) -> String {
    match subsystem {
        1 => "Native（驱动 / 原生进程）".into(),
        2 => "Windows GUI".into(),
        3 => "Windows 控制台".into(),
        5 => "OS/2 控制台".into(),
        7 => "POSIX 控制台".into(),
        9 => "Windows CE GUI".into(),
        10 => "EFI 应用".into(),
        11 => "EFI 引导服务驱动".into(),
        12 => "EFI 运行时驱动".into(),
        13 => "EFI ROM".into(),
        14 => "Xbox".into(),
        16 => "Windows 引导应用".into(),
        other => format!("0x{other:04x}"),
    }
}

fn characteristic_flags(c: u16) -> Vec<String> {
    const FLAGS: &[(u16, &str)] = &[
        (0x0001, "RELOCS_STRIPPED"),
        (0x0002, "EXECUTABLE_IMAGE"),
        (0x0020, "LARGE_ADDRESS_AWARE"),
        (0x0100, "32BIT_MACHINE"),
        (0x0200, "DEBUG_STRIPPED"),
        (0x0400, "REMOVABLE_RUN_FROM_SWAP"),
        (0x0800, "NET_RUN_FROM_SWAP"),
        (0x1000, "SYSTEM"),
        (0x2000, "DLL"),
        (0x4000, "UP_SYSTEM_ONLY"),
    ];
    FLAGS
        .iter()
        .filter(|(bit, _)| c & bit != 0)
        .map(|(_, name)| (*name).to_string())
        .collect()
}

fn dll_characteristic_flags(c: u16) -> Vec<String> {
    const FLAGS: &[(u16, &str)] = &[
        (0x0020, "HIGH_ENTROPY_VA"),
        (0x0040, "DYNAMIC_BASE (ASLR)"),
        (0x0080, "FORCE_INTEGRITY"),
        (0x0100, "NX_COMPAT (DEP)"),
        (0x0200, "NO_ISOLATION"),
        (0x0400, "NO_SEH"),
        (0x0800, "NO_BIND"),
        (0x1000, "APPCONTAINER"),
        (0x2000, "WDM_DRIVER"),
        (0x4000, "GUARD_CF"),
        (0x8000, "TERMINAL_SERVER_AWARE"),
    ];
    FLAGS
        .iter()
        .filter(|(bit, _)| c & bit != 0)
        .map(|(_, name)| (*name).to_string())
        .collect()
}

fn guid_string(g: &[u8; 16]) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        u32::from_le_bytes([g[0], g[1], g[2], g[3]]),
        u16::from_le_bytes([g[4], g[5]]),
        u16::from_le_bytes([g[6], g[7]]),
        g[8],
        g[9],
        g[10],
        g[11],
        g[12],
        g[13],
        g[14],
        g[15],
    )
}

// ---- 主解析入口 -----------------------------------------------------------

/// `data` 是文件前缀（可能被读取上限截断），`file_size` 是真实文件大小。
pub fn build(data: &[u8], file_size: u64) -> anyhow::Result<PeReport> {
    let mut warnings: Vec<String> = Vec::new();

    let opts = pe::options::ParseOptions::default()
        .with_parse_mode(ParseMode::Permissive)
        .with_parse_tls_data(false);
    let pe =
        pe::PE::parse_with_opts(data, &opts).map_err(|e| anyhow::anyhow!("不是有效的 PE：{e}"))?;

    let coff = &pe.header.coff_header;
    let opt = pe
        .header
        .optional_header
        .ok_or_else(|| anyhow::anyhow!("可选头缺失，不是可执行镜像"))?;
    let std_fields = &opt.standard_fields;
    let win = &opt.windows_fields;

    let is_pe64 = std_fields.magic == pe::optional_header::MAGIC_64;
    let format = if is_pe64 { "PE32+" } else { "PE32" }.to_string();

    // 节区表 + RVA 映射
    let mut sections: Vec<PeSection> = Vec::new();
    let mut map = SectionMap {
        sections: Vec::new(),
        header_limit: 0,
    };
    let mut budget = EntropyBudget::new();
    for s in &pe.sections {
        let name = s.name().map(|n| n.to_string()).unwrap_or_else(|_| {
            String::from_utf8_lossy(&s.name)
                .trim_end_matches('\0')
                .to_string()
        });
        let raw_off = s.pointer_to_raw_data;
        let raw_size = s.size_of_raw_data;
        let chars = s.characteristics;
        let perms = format!(
            "{}{}{}",
            if chars & 0x4000_0000 != 0 { 'r' } else { '-' },
            if chars & 0x8000_0000 != 0 { 'w' } else { '-' },
            if chars & 0x2000_0000 != 0 { 'x' } else { '-' },
        );
        let entropy = data
            .get(raw_off as usize..(raw_off as usize).saturating_add(raw_size as usize))
            .and_then(|slice| budget.entropy(slice));
        map.sections
            .push((s.virtual_address, s.virtual_size, raw_off, raw_size));
        sections.push(PeSection {
            name,
            virtual_address: s.virtual_address,
            virtual_size: s.virtual_size,
            raw_offset: raw_off,
            raw_size,
            characteristics: chars,
            perms,
            is_code: chars & 0x0000_0020 != 0,
            entropy,
        });
    }
    if budget.exhausted() {
        warnings.push("节区总数据量超过熵计算预算，部分节区未计算熵".into());
    }
    map.header_limit = map
        .sections
        .iter()
        .filter(|&&(_, _, _, raw_size)| raw_size > 0)
        .map(|&(_, _, raw_off, _)| raw_off)
        .min()
        .unwrap_or(win.size_of_headers)
        .min(win.size_of_headers)
        .min(u32::try_from(data.len()).unwrap_or(u32::MAX));

    let entry_section = pe
        .sections
        .iter()
        .find(|s| {
            pe.entry >= s.virtual_address
                && pe.entry - s.virtual_address < s.virtual_size.max(s.size_of_raw_data).max(1)
        })
        .and_then(|s| s.name().ok().map(|n| n.to_string()));

    // 导入表：goblin 的扁平表按 DLL 分组（保持首见顺序）
    let mut imports = group_imports(&pe.imports);
    // 延迟导入：goblin 不解析，按数据目录手工补
    if let Some(dd) = opt.data_directories.get_delay_import_descriptor() {
        imports.extend(parse_delay_imports(
            data,
            &map,
            (dd.virtual_address, dd.size),
            is_pe64,
            pe.image_base,
            &mut warnings,
        ));
    }

    // 导出表 / 调试目录：goblin 丢弃导出序号与每条调试目录项的时间戳/类型，故
    // 自原数据目录解析（取证需要正确序号、纯序号导出与调试时间戳）。
    let exp_dir = opt
        .data_directories
        .get_export_table()
        .map(|d| (d.virtual_address, d.size))
        .unwrap_or((0, 0));
    let exports = parse_exports(data, &map, exp_dir, &mut warnings);
    let dbg_dir = opt
        .data_directories
        .get_debug_table()
        .map(|d| (d.virtual_address, d.size))
        .unwrap_or((0, 0));
    let debug = parse_debug(data, &map, dbg_dir, &mut warnings);

    let version_info = match opt.data_directories.get_resource_table() {
        Some(dd) if dd.virtual_address != 0 => {
            match pe_rsrc::version_info(data, |rva| map.rva_to_off(rva), dd.virtual_address) {
                Ok(v) => v,
                Err(err) => {
                    warnings.push(format!("版本资源解析失败：{err}"));
                    None
                }
            }
        }
        _ => None,
    };

    // overlay：最后一个节区（按文件偏移）之后的数据
    let data_end: u64 = map
        .sections
        .iter()
        .map(|&(_, _, raw_off, raw_size)| u64::from(raw_off) + u64::from(raw_size))
        .max()
        .unwrap_or(u64::from(win.size_of_headers))
        .max(u64::from(win.size_of_headers));
    let overlay = (file_size > data_end && data_end > 0).then(|| PeOverlay {
        offset: data_end,
        size: file_size - data_end,
    });

    // 数据目录（保持 16 行命名输出）
    let dd = &opt.data_directories;
    let dir_table: [(&str, Option<&pe::data_directories::DataDirectory>); 16] = [
        ("Export", dd.get_export_table()),
        ("Import", dd.get_import_table()),
        ("Resource", dd.get_resource_table()),
        ("Exception", dd.get_exception_table()),
        ("Security (Authenticode)", dd.get_certificate_table()),
        ("Base Relocation", dd.get_base_relocation_table()),
        ("Debug", dd.get_debug_table()),
        ("Architecture", dd.get_architecture()),
        ("Global Ptr", dd.get_global_ptr()),
        ("TLS", dd.get_tls_table()),
        ("Load Config", dd.get_load_config_table()),
        ("Bound Import", dd.get_bound_import_table()),
        ("IAT", dd.get_import_address_table()),
        ("Delay Import", dd.get_delay_import_descriptor()),
        ("CLR (.NET)", dd.get_clr_runtime_header()),
        ("Reserved", None),
    ];
    let data_directories = dir_table
        .iter()
        .map(|(name, d)| DataDirEntry {
            name: (*name).to_string(),
            rva: d.map(|x| x.virtual_address).unwrap_or(0),
            size: d.map(|x| x.size).unwrap_or(0),
        })
        .collect();

    let truncated = file_size > data.len() as u64;
    if truncated {
        warnings.push(format!(
            "仅读取了文件前 {} 字节（共 {file_size} 字节），熵 / overlay 等统计基于已读部分",
            data.len(),
        ));
    }

    Ok(PeReport {
        format,
        machine: coff.machine,
        machine_label: machine_label(coff.machine),
        timestamp: coff.time_date_stamp,
        timestamp_utc: ts_utc(coff.time_date_stamp),
        characteristics: coff.characteristics,
        characteristic_flags: characteristic_flags(coff.characteristics),
        subsystem: win.subsystem,
        subsystem_label: subsystem_label(win.subsystem),
        dll_characteristics: win.dll_characteristics,
        dll_characteristic_flags: dll_characteristic_flags(win.dll_characteristics),
        entry_point: pe.entry,
        entry_section,
        image_base: format!("0x{:x}", pe.image_base),
        size_of_image: win.size_of_image,
        size_of_headers: win.size_of_headers,
        checksum: win.check_sum,
        linker_version: format!(
            "{}.{}",
            std_fields.major_linker_version, std_fields.minor_linker_version
        ),
        os_version: format!(
            "{}.{}",
            win.major_operating_system_version, win.minor_operating_system_version
        ),
        subsystem_version: format!(
            "{}.{}",
            win.major_subsystem_version, win.minor_subsystem_version
        ),
        is_dll: coff.characteristics & 0x2000 != 0,
        // 取数据目录的「存在性」而非 goblin 解析成功：Authenticode 证书块可能位于
        // 64 MiB 读取上限之外或被损坏（恶意样本常见），但目录大小已是确定信号；
        // CLR 目录同理。沿用旧解析器语义，避免大文件/损坏块上的假阴性。
        is_dotnet: opt
            .data_directories
            .get_clr_runtime_header()
            .is_some_and(|d| d.virtual_address != 0 && d.size != 0),
        is_signed: opt
            .data_directories
            .get_certificate_table()
            .is_some_and(|d| d.size != 0),
        data_directories,
        sections,
        imports,
        exports,
        debug,
        version_info,
        overlay,
        parse_warnings: warnings,
        file_size,
        truncated,
    })
}

/// goblin 的扁平导入表按 DLL 分组，保持首见顺序，每 DLL 函数截到 `MAX_FUNCS`。
fn group_imports(imports: &[pe::import::Import]) -> Vec<PeImport> {
    let mut order: Vec<String> = Vec::new();
    let mut by_dll: std::collections::HashMap<String, PeImport> = std::collections::HashMap::new();
    for imp in imports {
        let dll = imp.dll.to_string();
        let entry = by_dll.entry(dll.clone()).or_insert_with(|| {
            order.push(dll.clone());
            PeImport {
                dll,
                delayed: false,
                functions: Vec::new(),
                total: 0,
            }
        });
        entry.total += 1;
        if entry.functions.len() < MAX_FUNCS {
            let name = if imp.name.is_empty() {
                format!("Ordinal#{}", imp.ordinal)
            } else {
                imp.name.to_string()
            };
            entry.functions.push(name);
        }
    }
    order
        .into_iter()
        .filter_map(|dll| by_dll.remove(&dll))
        .collect()
}

// ---- 导出表（自原数据目录解析，保正确序号 + 纯序号导出，移植自旧手写解析器）----

fn parse_exports(
    data: &[u8],
    map: &SectionMap,
    (dir_rva, dir_size): (u32, u32),
    warnings: &mut Vec<String>,
) -> Option<PeExports> {
    if dir_rva == 0 || dir_size == 0 {
        return None;
    }
    let base = match map.rva_to_off(dir_rva) {
        Some(b) => b,
        None => {
            warnings.push("导出表的 RVA 无法定位到文件偏移".into());
            return None;
        }
    };
    let header = (|| {
        Some((
            u32le(data, base + 12)?,          // Name RVA
            u32le(data, base + 16)?,          // OrdinalBase
            u32le(data, base + 20)? as usize, // NumberOfFunctions
            u32le(data, base + 24)? as usize, // NumberOfNames
            u32le(data, base + 28)?,          // AddressOfFunctions
        ))
    })();
    let Some((name_rva, ordinal_base, num_funcs, num_names, af_rva)) = header else {
        warnings.push("导出目录头越过文件末尾，导出表不可读".into());
        return None;
    };
    let Some(addr_funcs) = map.rva_to_off(af_rva) else {
        warnings.push("导出函数地址表的 RVA 无法定位到文件偏移".into());
        return None;
    };
    let addr_names = u32le(data, base + 32).and_then(|r| map.rva_to_off(r));
    let addr_ords = u32le(data, base + 36).and_then(|r| map.rva_to_off(r));

    // 先按名字表建 ordinal → name 映射，再走完整函数表（含纯序号导出）。
    let mut names_by_index: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();
    if let (Some(an), Some(ao)) = (addr_names, addr_ords) {
        for i in 0..num_names.min(MAX_FUNCS * 4) {
            let Some(nrva) = u32le(data, an + i * 4) else {
                break;
            };
            let Some(ord_index) = u16le(data, ao + i * 2) else {
                break;
            };
            if let Some(name) = map.rva_to_off(nrva).and_then(|o| cstr(data, o, 512)) {
                names_by_index.insert(ord_index as usize, name);
            }
        }
    }

    let dir_end = dir_rva.saturating_add(dir_size);
    let mut entries = Vec::new();
    for i in 0..num_funcs.min(MAX_FUNCS * 4) {
        let Some(func_rva) = u32le(data, addr_funcs + i * 4) else {
            break;
        };
        if func_rva == 0 {
            continue; // 空槽位（ordinal 间隙）
        }
        if entries.len() >= MAX_FUNCS {
            break;
        }
        // 函数 RVA 落在导出目录范围内 → forwarder 字符串而非代码
        let forwarder = (func_rva >= dir_rva && func_rva < dir_end)
            .then(|| map.rva_to_off(func_rva).and_then(|o| cstr(data, o, 512)))
            .flatten();
        entries.push(PeExportEntry {
            ordinal: ordinal_base.saturating_add(i as u32),
            name: names_by_index.get(&i).cloned(),
            rva: func_rva,
            forwarder,
        });
    }

    Some(PeExports {
        dll_name: map.rva_to_off(name_rva).and_then(|o| cstr(data, o, 256)),
        total: num_funcs,
        entries,
    })
}

// ---- 调试目录（逐项解析，保每项时间戳 + 全类型 + CodeView PDB，移植自旧实现）----

fn parse_debug(
    data: &[u8],
    map: &SectionMap,
    (dir_rva, dir_size): (u32, u32),
    warnings: &mut Vec<String>,
) -> Vec<PeDebugEntry> {
    let mut out = Vec::new();
    if dir_rva == 0 || dir_size == 0 {
        return out;
    }
    let Some(base) = map.rva_to_off(dir_rva) else {
        warnings.push("调试目录的 RVA 无法定位到文件偏移".into());
        return out;
    };
    let count = (dir_size as usize / 28).min(MAX_DEBUG_ENTRIES);
    for i in 0..count {
        let e = base + i * 28;
        let Some(type_id) = u32le(data, e + 12) else {
            break;
        };
        let ts = u32le(data, e + 4).unwrap_or(0);
        let size_of_data = u32le(data, e + 16).unwrap_or(0) as usize;
        let ptr = u32le(data, e + 24).unwrap_or(0) as usize;
        let kind = match type_id {
            1 => "COFF".to_string(),
            2 => "CodeView".to_string(),
            3 => "FPO".to_string(),
            4 => "Misc".to_string(),
            9 => "Borland".to_string(),
            16 => "Repro（可重现构建）".to_string(),
            17 => "Embedded PDB".to_string(),
            19 => "PDB Checksum".to_string(),
            20 => "Ex.DllCharacteristics".to_string(),
            other => format!("类型 {other}"),
        };
        let (mut pdb_path, mut guid, mut age) = (None, None, None);
        if type_id == 2 && size_of_data >= 24 {
            if let Some(cv) = data.get(ptr..ptr + size_of_data.min(1024)) {
                if cv.get(..4) == Some(b"RSDS".as_slice()) {
                    if let Ok(g) = <&[u8; 16]>::try_from(&cv[4..20]) {
                        guid = Some(guid_string(g));
                    }
                    age = u32le(cv, 20);
                    pdb_path = cstr(cv, 24, 512).filter(|s| !s.is_empty());
                }
            }
        }
        out.push(PeDebugEntry {
            kind,
            timestamp_utc: ts_utc(ts),
            pdb_path,
            guid,
            age,
        });
    }
    out
}

/// 延迟导入描述符解析（从旧手写解析器移植）。goblin 的 `pe.imports` 不含延迟导入。
/// IMAGE_DELAYLOAD_DESCRIPTOR { Attributes, DllNameRVA, ModuleHandleRVA,
/// ImportAddressTableRVA, ImportNameTableRVA, ... }（32 字节）。
fn parse_delay_imports(
    data: &[u8],
    map: &SectionMap,
    (dir_rva, dir_size): (u32, u32),
    is_pe64: bool,
    image_base: u64,
    warnings: &mut Vec<String>,
) -> Vec<PeImport> {
    let mut out = Vec::new();
    if dir_rva == 0 || dir_size == 0 {
        return out;
    }
    let Some(base) = map.rva_to_off(dir_rva) else {
        warnings.push("延迟导入表的 RVA 无法定位到文件偏移".into());
        return out;
    };
    let mut legacy_va_warned = false;
    let mut terminated = false;
    for i in 0..MAX_DELAY_DLLS {
        let d = base + i * 32;
        let Some(raw_name) = u32le(data, d + 4) else {
            warnings.push("延迟导入表越过文件末尾，已截断".into());
            terminated = true;
            break;
        };
        if raw_name == 0 {
            terminated = true;
            break; // 全零结束符
        }
        // Attributes 位 0 为 0 时各字段是绝对 VA 而非 RVA（旧式链接器），需减镜像基址。
        let rva_based = (u32le(data, d).unwrap_or(1) & 1) != 0;
        if !rva_based && !legacy_va_warned {
            legacy_va_warned = true;
            warnings.push("延迟导入表使用 VA 寻址（旧式链接器），已按镜像基址换算".into());
        }
        let fix_addr = |v: u32| -> u32 {
            if rva_based {
                v
            } else {
                u64::from(v)
                    .checked_sub(image_base)
                    .and_then(|r| u32::try_from(r).ok())
                    .unwrap_or(0)
            }
        };
        let name_rva = fix_addr(raw_name);
        let dll = map
            .rva_to_off(name_rva)
            .and_then(|off| cstr(data, off, 256))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("<无效名称 rva=0x{name_rva:x}>"));

        let thunk_rva = fix_addr(u32le(data, d + 16).unwrap_or(0));
        let mut functions = Vec::new();
        let mut total = 0usize;
        if let Some(mut t) = map.rva_to_off(thunk_rva) {
            let step = if is_pe64 { 8 } else { 4 };
            loop {
                let val = if is_pe64 {
                    u64le(data, t)
                } else {
                    u32le(data, t).map(u64::from)
                };
                let Some(val) = val else { break };
                if val == 0 {
                    break;
                }
                total += 1;
                if total > MAX_FUNCS * 4 {
                    warnings.push(format!("{dll} 的延迟导入 thunk 数组异常长，已放弃计数"));
                    break;
                }
                if functions.len() < MAX_FUNCS {
                    let ordinal_bit = if is_pe64 { 1u64 << 63 } else { 1u64 << 31 };
                    if val & ordinal_bit != 0 {
                        functions.push(format!("Ordinal#{}", val & 0xffff));
                    } else {
                        let hint_rva = fix_addr((val & 0x7fff_ffff) as u32);
                        let name = map
                            .rva_to_off(hint_rva)
                            .and_then(|off| cstr(data, off + 2, 512))
                            .filter(|s| !s.is_empty());
                        functions.push(name.unwrap_or_else(|| format!("<rva=0x{hint_rva:x}>")));
                    }
                }
                t += step;
            }
        }
        out.push(PeImport {
            dll,
            delayed: true,
            functions,
            total,
        });
    }
    if !terminated {
        warnings.push(format!(
            "延迟导入表超过 {MAX_DELAY_DLLS} 项上限仍无结束符，已截断"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_pe() {
        assert!(build(b"not a pe file at all, padding padding padding padding", 50).is_err());
        let mut mz_only = vec![0u8; 128];
        mz_only[0] = b'M';
        mz_only[1] = b'Z';
        assert!(build(&mz_only, 128).is_err());
    }

    /// 手工构造一个最小 PE32+：MZ stub + PE 头 + 一个节区。
    #[test]
    fn parses_minimal_pe64() {
        let mut d = vec![0u8; 0x400];
        d[0] = b'M';
        d[1] = b'Z';
        d[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        d[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        d[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes()); // x64
        d[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // 1 section
        d[coff + 4..coff + 8].copy_from_slice(&0x6000_0000u32.to_le_bytes()); // timestamp
        d[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes()); // opt size
        d[coff + 18..coff + 20].copy_from_slice(&0x2002u16.to_le_bytes()); // EXE | DLL
        let opt = coff + 20;
        d[opt..opt + 2].copy_from_slice(&0x020bu16.to_le_bytes()); // PE32+
        d[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes()); // entry
        d[opt + 24..opt + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes()); // base
        d[opt + 60..opt + 64].copy_from_slice(&0x200u32.to_le_bytes()); // hdr size
        d[opt + 68..opt + 70].copy_from_slice(&3u16.to_le_bytes()); // console
        d[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes()); // num rva
        let sec = opt + 240;
        d[sec..sec + 6].copy_from_slice(b".text\0");
        d[sec + 8..sec + 12].copy_from_slice(&0x100u32.to_le_bytes()); // vsize
        d[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // va
        d[sec + 16..sec + 20].copy_from_slice(&0x100u32.to_le_bytes()); // raw size
        d[sec + 20..sec + 24].copy_from_slice(&0x300u32.to_le_bytes()); // raw off
        d[sec + 36..sec + 40].copy_from_slice(&0x6000_0020u32.to_le_bytes()); // code r-x

        let report = build(&d, d.len() as u64).expect("minimal PE parses");
        assert_eq!(report.format, "PE32+");
        assert_eq!(report.machine_label, "x64 (AMD64)");
        assert!(report.is_dll);
        assert_eq!(report.sections.len(), 1);
        assert_eq!(report.sections[0].name, ".text");
        assert_eq!(report.sections[0].perms, "r-x");
        assert_eq!(report.entry_section.as_deref(), Some(".text"));
        assert_eq!(report.image_base, "0x140000000");
        assert_eq!(report.subsystem_label, "Windows 控制台");
        assert!(report.imports.is_empty());
    }
}
