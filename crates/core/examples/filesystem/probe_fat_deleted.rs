//! FAT deleted-file recovery probe. Opens an image, finds the FAT12/16/32
//! volume, prints the geometry the recovery path resolves (proving the
//! sector-6 backup BPB is used when the primary boot sector is wiped), and
//! counts recovered deleted entries from every surfacing path:
//!   * per-directory inline `0xE5` entries (`children_with_deleted`),
//!   * the synthetic `$OrphanFiles` orphan scan (dispatcher).
//!
//! Usage: cargo run --no-default-features --example probe_fat_deleted -- <image>

use std::env;
use std::sync::Arc;

use lovelycarve_core::evidence::image::PartitionKind;
use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{
    children_for_image, read_file_limited, NodeKind,
};
use lovelycarve_core::filesystem::volumes::fat::FatVolume;
use lovelycarve_core::filesystem::volumes::fat_deleted;

const MAX_DEPTH: usize = 6;
const MAX_PRINT: usize = 60;

fn main() -> anyhow::Result<()> {
    let path = env::args()
        .nth(1)
        .expect("usage: probe_fat_deleted <image>");
    let image = Arc::new(EvidenceSession::open(&path)?);
    let info = image.info();
    println!("image:       {}", info.path);
    println!("sector_size: {}", info.sector_size);
    println!("partitions:");
    for partition in &info.partitions {
        println!(
            "  #{} kind={:?} start_lba={} length_lba={}",
            partition.id, partition.kind, partition.start_lba, partition.length_lba
        );
    }

    let partition = image
        .partitions
        .iter()
        .find(|p| {
            matches!(
                p.kind,
                PartitionKind::Fat32 | PartitionKind::Fat12 | PartitionKind::Fat16
            )
        })
        .expect("no FAT12/16/32 partition found in this image");
    let base_offset = partition.start_lba * image.sector_size as u64;
    println!(
        "\nselected FAT partition #{} (kind={:?}) base_offset=0x{:x}",
        partition.id, partition.kind, base_offset
    );

    // Prove which boot sector the recovery geometry resolved from.
    let geom = fat_deleted::open_geom(&image, base_offset)?;
    println!(
        "resolved geom: is_fat32={} bytes/sec={} sec/clus={} reserved={} num_fats={} \
         sectors_per_fat={} root_cluster={} total_sectors={} cluster_count={}",
        geom.is_fat32,
        geom.bytes_per_sector,
        geom.sectors_per_cluster,
        geom.reserved_sectors,
        geom.num_fats,
        geom.sectors_per_fat,
        geom.root_cluster,
        geom.total_sectors,
        geom.cluster_count(),
    );

    let volume = FatVolume::open(Arc::clone(&image), partition)?;
    let live = volume.root_children()?;
    println!("\nroot: {} live entries", live.len());

    // Walk live directories, counting inline deleted entries at every level.
    let mut total_deleted = 0usize;
    let mut printed = 0usize;
    walk(&volume, "", 0, &mut total_deleted, &mut printed);
    println!("\ninline deleted entries recovered across the tree: {total_deleted}");

    // Expand the synthetic $OrphanFiles node through the real dispatcher path.
    let part_children = children_for_image(&image, &format!("part:{}", partition.id))?;
    println!(
        "\n=== GUI root view: children_for_image(\"part:{}\") -> {} entries ===",
        partition.id,
        part_children.len()
    );
    for n in &part_children {
        let flag = if n.deleted { " <DELETED>" } else { "" };
        println!(
            "  {:?} {} {} bytes deleted={} id={}{}",
            n.kind, n.name, n.size, n.deleted, n.id, flag
        );
    }

    // Confirm the FAT metadata virtual files are present and readable.
    for name in ["$DBR", "$FAT1", "$FAT2"] {
        if let Some(node) = part_children.iter().find(|n| n.name == name) {
            match read_file_limited(&image, &node.id, 16) {
                Ok(bytes) => {
                    let hex = bytes
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    println!("  read {name} ({} bytes) first16: {hex}", node.size);
                }
                Err(err) => println!("  read {name} FAILED: {err}"),
            }
        } else {
            println!("  ({name} node missing!)");
        }
    }

    if let Some(orphan_node) = part_children.iter().find(|n| n.id.starts_with("orphans")) {
        println!(
            "\nexpanding {} (id={}) ...",
            orphan_node.name, orphan_node.id
        );
        let orphans = children_for_image(&image, &orphan_node.id)?;
        println!("$OrphanFiles recovered: {} entries", orphans.len());
        for n in orphans.iter().take(MAX_PRINT) {
            println!(
                "  [ORPHAN] {:?} {} {} bytes id={}",
                n.kind, n.name, n.size, n.id
            );
        }
    } else {
        println!(
            "\n(no $OrphanFiles node present in part:{} listing)",
            partition.id
        );
    }

    Ok(())
}

fn walk(
    volume: &FatVolume,
    path: &str,
    depth: usize,
    total_deleted: &mut usize,
    printed: &mut usize,
) {
    let entries = match volume.children_with_deleted(path) {
        Ok(entries) => entries,
        Err(err) => {
            println!("  (children_with_deleted {path} failed: {err})");
            return;
        }
    };
    for node in &entries {
        if node.deleted {
            *total_deleted += 1;
            if *printed < MAX_PRINT {
                let shown = if path.is_empty() { "/" } else { path };
                println!(
                    "  [DEL] {shown} :: {:?} {} {} bytes id={}",
                    node.kind, node.name, node.size, node.id
                );
                *printed += 1;
            }
        }
    }
    if depth >= MAX_DEPTH {
        return;
    }
    for node in &entries {
        if node.kind == NodeKind::Dir && !node.deleted {
            let child = if path.is_empty() {
                format!("/{}", node.name)
            } else {
                format!("{path}/{}", node.name)
            };
            walk(volume, &child, depth + 1, total_deleted, printed);
        }
    }
}
