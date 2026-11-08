//! Metadata-less RAID reconstruction probe.
//!
//! Usage: just run-example probe_raid_nometa <member1> <member2> [<member3> ...]
//!
//! Loads each raw member image, runs the full RAID auto-detection (including
//! the metadata-less heuristic pass), then materialises every assembled array
//! as a synthetic `DiskImage` and walks its filesystem tree — exactly what the
//! GUI does after "打开重组镜像". Proves both detection and browsing on real
//! hardware-RAID dumps that carry no mdadm/LVM metadata.

use std::env;
use std::sync::Arc;

use lovelycarve_core::evidence::image::{DiskImage, Partition};
use lovelycarve_core::evidence::raid::{self, synthetic_partition_label};
use lovelycarve_core::filesystem::tree::fs_tree::{children_for_image, NodeKind};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: probe_raid_nometa <member1> <member2> [<member3> ...]");
        std::process::exit(2);
    }

    let mut images = Vec::new();
    for path in &args {
        println!("loading: {path}");
        let image = DiskImage::open(path)?;
        let info = image.info();
        println!(
            "  format={:?} size={} partitions={}",
            info.source_format,
            human(info.size_bytes),
            info.partitions.len()
        );
        images.push(Arc::new(image));
    }

    println!("\n== RAID auto-detection ==");
    let (stack, assembled) = raid::detect_with_sources(&images);

    println!("arrays: {}", stack.arrays.len());
    for a in &stack.arrays {
        println!(
            "  RAID{} {} heuristic={} disks={} chunk={}K layout={} data_offset={} size={}",
            a.level,
            if a.set_name.is_empty() {
                &a.set_uuid
            } else {
                &a.set_name
            },
            a.heuristic,
            a.raid_disks,
            a.chunksize_sectors / 2,
            a.layout,
            a.data_offset_sectors,
            human(a.assembled_bytes),
        );
        for m in &a.members {
            println!("    slot={:?} {:?}", m.slot, m.origin);
        }
    }
    println!("filesystems: {}", stack.filesystems.len());
    for fs in &stack.filesystems {
        println!(
            "  {} fsid={:?} total={:?}",
            fs.kind,
            fs.fsid,
            fs.total_bytes.map(human)
        );
    }
    println!("diagnostics:");
    for d in &stack.diagnostics {
        println!("  - {d}");
    }

    println!("\n== browsing assembled arrays ==");
    for a in &assembled {
        let label = if a.set_name.is_empty() {
            format!("md:{}", a.set_uuid)
        } else {
            format!("md:{}-{}", a.set_name, a.set_uuid)
        };
        let partitions = a.partitions.clone().unwrap_or_else(|| {
            vec![Partition {
                id: 0,
                kind: a.fs_kind,
                start_lba: 0,
                length_lba: a.size_bytes / 512,
                label: Some(synthetic_partition_label(a)),
                unique_guid: None,
                amiga_geometry: None,
            }]
        });
        let img = Arc::new(DiskImage::from_assembled(
            label.clone(),
            a.source.clone(),
            partitions,
            512,
        )?);
        let img =
            Arc::new(lovelycarve_core::filesystem::evidence_session::EvidenceSession::from(img));
        println!("\nassembled image {label} ({}):", human(img.len()));
        for p in &img.partitions {
            println!(
                "  part:{} kind={:?} start={} len={} label={:?}",
                p.id,
                p.kind,
                p.start_lba,
                human(p.length_lba * 512),
                p.label
            );
        }
        // Walk each filesystem partition's root, descend one level.
        for p in img.partitions.clone() {
            let root_id = format!("part:{}", p.id);
            let Ok(root) = children_for_image(&img, &root_id) else {
                continue;
            };
            if root.is_empty() {
                continue;
            }
            println!("  / ({:?}):", p.kind);
            let names: Vec<&str> = root.iter().map(|n| n.name.as_str()).collect();
            println!("    {}", names.join("  "));
            // Descend into the first few directories to prove dense reads.
            let mut shown = 0;
            for node in &root {
                if node.kind != NodeKind::Dir || shown >= 4 {
                    continue;
                }
                if let Ok(children) = children_for_image(&img, &node.id) {
                    if !children.is_empty() {
                        let sample: Vec<&str> =
                            children.iter().take(8).map(|n| n.name.as_str()).collect();
                        println!("    {}/  ->  {}", node.name, sample.join("  "));
                        shown += 1;
                    }
                }
            }
        }
    }

    Ok(())
}

fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}
