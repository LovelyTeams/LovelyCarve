//! CDFS / ISO 9660 end-to-end probe. Verifies the lovelycarve dispatcher path
//! (`fs_tree::children_for_image` / `read_file_limited` / `file_stat`) against
//! a real image.
//!
//! Usage: cargo run --example probe_cdfs -- <image.iso>

use std::env;
use std::sync::Arc;

use anyhow::{anyhow, bail};
use lovelycarve_core::evidence::image::PartitionKind;
use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{
    children_for_image, file_stat, read_file_limited, NodeKind,
};

fn main() -> anyhow::Result<()> {
    let Some(path) = env::args().nth(1) else {
        bail!("usage: probe_cdfs <image.iso>");
    };
    let image = Arc::new(EvidenceSession::open(&path)?);
    let info = image.info();
    println!("image: {}", info.path);
    println!("sector_size: {}", info.sector_size);
    println!("partitions:");
    for partition in &info.partitions {
        println!(
            "  #{} kind={:?} start_lba={} length_lba={}",
            partition.id, partition.kind, partition.start_lba, partition.length_lba
        );
    }

    let cdfs_partition = info
        .partitions
        .iter()
        .find(|partition| partition.kind == PartitionKind::Cdfs)
        .ok_or_else(|| anyhow!("no CDFS partition found in this image"))?;
    println!(
        "\nselected CDFS partition #{} at LBA {} ({} sectors)",
        cdfs_partition.id, cdfs_partition.start_lba, cdfs_partition.length_lba
    );

    let part_id = format!("part:{}", cdfs_partition.id);
    let dispatched = children_for_image(&image, &part_id)?;
    println!(
        "\ndispatcher children for {part_id}: {} entries",
        dispatched.len()
    );
    for node in dispatched.iter().take(20) {
        println!(
            "  {:?} {} {} bytes id={}",
            node.kind, node.name, node.size, node.id
        );
    }

    let mut sample = None;
    let mut frontier = dispatched.to_vec();
    'outer: while let Some(node) = frontier.pop() {
        if node.kind == NodeKind::File && node.size > 0 && node.size < 64 * 1024 {
            sample = Some(node);
            break;
        }
        if node.kind == NodeKind::Dir {
            for child in children_for_image(&image, &node.id)? {
                if child.kind == NodeKind::File && child.size > 0 && child.size < 64 * 1024 {
                    sample = Some(child);
                    break 'outer;
                }
                if child.kind == NodeKind::Dir {
                    frontier.push(child);
                }
            }
        }
    }

    if let Some(file) = sample {
        let bytes = read_file_limited(&image, &file.id, 64 * 1024)?;
        println!(
            "\nread {} ({} bytes requested, got {} bytes); first 16 bytes: {}",
            file.name,
            file.size,
            bytes.len(),
            bytes
                .iter()
                .take(16)
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let stat = file_stat(&image, &file.id)?;
        println!(
            "stat: size={} mime={} entropy={:?} mode={:?}",
            stat.size, stat.mime, stat.entropy, stat.mode
        );
    } else {
        println!("\n(no small file found in the tree for read/stat probe)");
    }

    Ok(())
}
