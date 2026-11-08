//! Quick UFS read probe. Usage: cargo run --example probe_ufs -- <image>
//!
//! Lists every UFS partition in the image, opens the volume, walks the root
//! directory, and samples the first regular file. Mirrors the existing
//! `probe_squashfs.rs` / `probe_apfs.rs` shape.
use std::env;
use std::sync::Arc;

use lovelycarve_core::evidence::image::PartitionKind;
use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{
    children_for_image, read_file_limited, NodeKind,
};
use lovelycarve_core::filesystem::volumes::ufs::UfsVolume;

fn main() -> anyhow::Result<()> {
    let path = env::args().nth(1).expect("usage: probe_ufs <image>");
    let image = Arc::new(EvidenceSession::open(&path)?);
    let info = image.info();
    println!("image: {}", info.path);
    println!("size:  {} bytes", info.size_bytes);

    let ufs_partitions: Vec<_> = image
        .partitions
        .iter()
        .filter(|p| p.kind == PartitionKind::Ufs)
        .cloned()
        .collect();
    if ufs_partitions.is_empty() {
        eprintln!("no UFS partitions found");
        return Ok(());
    }

    for partition in ufs_partitions {
        println!(
            "\n== partition #{} {:?} @ LBA {} ({} bytes) ==",
            partition.id,
            partition.label,
            partition.start_lba,
            partition.length_lba * info.sector_size as u64,
        );
        let volume = UfsVolume::open(Arc::clone(&image), &partition)?;
        let children = volume.root_children()?;
        println!("root entries: {}", children.len());
        for entry in children.iter().take(20) {
            println!(
                "  {kind:<8} {size:>10}  {name}",
                kind = format!("{:?}", entry.kind),
                size = entry.size,
                name = entry.name,
            );
        }
        if let Some(first_file) = children.iter().find(|n| n.kind == NodeKind::File) {
            let path = format!("/{}", first_file.name);
            let bytes = volume.read_file(&path, 256)?;
            println!(
                "\nsampled {path} ({} bytes returned, first 16 hex):",
                bytes.len()
            );
            let prefix: Vec<String> = bytes
                .iter()
                .take(16)
                .map(|b| format!("{:02x}", b))
                .collect();
            println!("  {}", prefix.join(" "));
        }

        // Drive the dispatcher path: list /etc through the partition node id,
        // then read /etc/rc through the resulting `ufs:part:N:hex(/etc/rc)` id.
        let part_node_id = format!("part:{}", partition.id);
        let etc_node = children_for_image(&image, &part_node_id)?
            .into_iter()
            .find(|n| n.name == "etc" && n.kind == NodeKind::Dir);
        if let Some(etc) = etc_node {
            let etc_children = children_for_image(&image, &etc.id)?;
            println!("\n/etc via dispatcher: {} entries", etc_children.len());
            for entry in etc_children.iter().take(8) {
                println!(
                    "  {kind:<8} {size:>10}  {name}",
                    kind = format!("{:?}", entry.kind),
                    size = entry.size,
                    name = entry.name,
                );
            }
            if let Some(rc) = etc_children
                .iter()
                .find(|n| n.kind == NodeKind::File && n.name.starts_with("rc"))
            {
                let bytes = read_file_limited(&image, &rc.id, 64)?;
                let prefix: Vec<String> = bytes
                    .iter()
                    .take(64)
                    .map(|b| format!("{:02x}", b))
                    .collect();
                println!(
                    "\nread {} ({} bytes via dispatcher):\n  {}",
                    rc.name,
                    bytes.len(),
                    prefix.join(" ")
                );
            }
        }
    }
    Ok(())
}
