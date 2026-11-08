//! 端到端 PE 解析探针：打开镜像 → 枚举分区 → 走文件树找 PE 文件 → 解析并
//! 打印报告摘要。用于在不起 UI 的情况下验证 `fs_pe_preview` 的完整链路。
//!
//! ```text
//! cargo run --release --example probe_pe -- <镜像路径> [最多解析个数，默认 5]
//! ```

use std::sync::Arc;

use lovelycarve_core::{
    filesystem::evidence_session::EvidenceSession,
    filesystem::tree::fs_tree,
    preview::binary::binfmt::{self, NativeReport},
};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("用法: probe_pe <镜像路径> [最多解析个数]");
        std::process::exit(2);
    };
    let max_files: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);

    let image = Arc::new(EvidenceSession::open(&path)?);
    println!("镜像: {path}");
    println!("分区数: {}", image.partitions.len());

    let roots = fs_tree::children_for_image(&image, "root")?;
    let mut found: Vec<(String, String)> = Vec::new(); // (node_id, path)
    for root in roots {
        if found.len() >= max_files {
            break;
        }
        if root.kind != fs_tree::NodeKind::Dir {
            continue;
        }
        println!("-- 遍历 {} ({}) --", root.name, root.id);
        let mut walker = fs_tree::SnapshotWalker::new(&image);
        let walk = walker.walk_visit_from(&root.id, &mut |entry: &fs_tree::FsWalkEntry| {
            let name = entry.node.name.to_ascii_lowercase();
            if entry.node.kind == fs_tree::NodeKind::File
                && !entry.node.deleted
                && entry.node.size > 0
                && (name.ends_with(".exe") || name.ends_with(".dll"))
            {
                found.push((entry.node.id.clone(), entry.path.clone()));
            }
            Ok(found.len() < max_files)
        });
        if let Err(err) = walk {
            eprintln!("   遍历中止: {err:#}");
        }
    }

    if found.is_empty() {
        println!("未找到 .exe / .dll 文件");
        return Ok(());
    }

    for (node_id, file_path) in found {
        println!("\n=== {file_path} ===");
        let data = match fs_tree::read_file_limited(&image, &node_id, 64 * 1024 * 1024) {
            Ok(d) => d,
            Err(err) => {
                println!("  读取失败: {err:#}");
                continue;
            }
        };
        let stat_size = fs_tree::file_stat(&image, &node_id)
            .map(|s| s.size)
            .unwrap_or(data.len() as u64);
        match binfmt::parse(&data, stat_size.max(data.len() as u64)) {
            Ok(NativeReport::Pe(r)) => {
                println!(
                    "  {} | {} | {} | 编译时间 {}",
                    r.format,
                    r.machine_label,
                    r.subsystem_label,
                    r.timestamp_utc.as_deref().unwrap_or("—"),
                );
                println!(
                    "  节区 {} 个, 导入 {} DLL, 导出 {} 项, 签名块: {}, .NET: {}",
                    r.sections.len(),
                    r.imports.len(),
                    r.exports.as_ref().map(|e| e.total).unwrap_or(0),
                    r.is_signed,
                    r.is_dotnet,
                );
                for s in &r.sections {
                    println!(
                        "    {:8} va=0x{:08x} raw=0x{:08x}+0x{:x} {} 熵={}",
                        s.name,
                        s.virtual_address,
                        s.raw_offset,
                        s.raw_size,
                        s.perms,
                        s.entropy
                            .map(|e| format!("{e:.2}"))
                            .unwrap_or_else(|| "—".into()),
                    );
                }
                for imp in r.imports.iter().take(6) {
                    println!(
                        "    导入 {}{} ({} 个函数)",
                        imp.dll,
                        if imp.delayed { " [延迟]" } else { "" },
                        imp.total,
                    );
                }
                if let Some(v) = &r.version_info {
                    println!(
                        "    版本 {} | {}",
                        v.file_version.as_deref().unwrap_or("—"),
                        v.strings
                            .iter()
                            .filter(|s| {
                                matches!(
                                    s.key.as_str(),
                                    "CompanyName" | "OriginalFilename" | "FileDescription"
                                )
                            })
                            .map(|s| format!("{}={}", s.key, s.value))
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
                for d in &r.debug {
                    if let Some(p) = &d.pdb_path {
                        println!("    PDB: {p}");
                    }
                }
                if let Some(o) = &r.overlay {
                    println!("    overlay: 偏移 0x{:x}, {} 字节", o.offset, o.size);
                }
                for w in &r.parse_warnings {
                    println!("    警告: {w}");
                }
            }
            Ok(_) => println!("  二进制解析结果不是 PE"),
            Err(err) => println!("  PE 解析失败: {err:#}"),
        }
    }
    Ok(())
}
