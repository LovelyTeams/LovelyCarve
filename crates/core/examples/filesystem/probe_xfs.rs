//! Probe XFS partitions in an image: detect them, list root, sample a file.
//!
//! Usage: cargo run --release --example probe_xfs -- <image>
use std::env;
use std::sync::Arc;

use lovelycarve_core::evidence::partitions::PartitionKind;
use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{
    children_for_image, read_file_limited, NodeKind,
};

fn main() -> anyhow::Result<()> {
    let path = env::args().nth(1).expect("usage: probe_xfs <image>");
    let image = Arc::new(EvidenceSession::open(&path)?);
    let info = image.info();
    println!("image: {}", info.path);
    println!(
        "size:  {} bytes, partitions: {}",
        info.size_bytes,
        info.partitions.len()
    );

    let xfs_partitions: Vec<_> = info
        .partitions
        .iter()
        .filter(|p| p.kind == PartitionKind::Xfs)
        .cloned()
        .collect();
    if xfs_partitions.is_empty() {
        println!("no XFS partitions detected");
        return Ok(());
    }

    for p in &xfs_partitions {
        println!();
        println!(
            "== XFS partition #{} (LBA {} .. {}) ==",
            p.id,
            p.start_lba,
            p.start_lba + p.length_lba
        );
        let parent_id = format!("part:{}", p.id);
        let children = children_for_image(&image, &parent_id)?;
        println!("root: {} entries", children.len());
        for child in children.iter().take(40) {
            println!(
                "  {kind:1} {name} ({size} bytes) ino={inode}",
                kind = match child.kind {
                    NodeKind::Dir => "d",
                    NodeKind::File => "-",
                    NodeKind::Symlink => "l",
                    NodeKind::Unallocated => "?",
                },
                name = child.name,
                size = child.size,
                inode = child.inode.unwrap_or(0)
            );
        }
        if children.len() > 40 {
            println!("  ... ({} more)", children.len() - 40);
        }

        // Walk one subdirectory to exercise directory recursion.
        if let Some(subdir) = children.iter().find(|c| c.kind == NodeKind::Dir) {
            println!();
            println!("== children of {}/ ==", subdir.name);
            match children_for_image(&image, &subdir.id) {
                Ok(grandchildren) => {
                    for g in grandchildren.iter().take(20) {
                        println!(
                            "  {kind:1} {name} ({size} bytes)",
                            kind = match g.kind {
                                NodeKind::Dir => "d",
                                NodeKind::File => "-",
                                NodeKind::Symlink => "l",
                                NodeKind::Unallocated => "?",
                            },
                            name = g.name,
                            size = g.size
                        );
                    }
                }
                Err(e) => println!("subdir walk failed: {e}"),
            }
        }

        if let Some(file) = children
            .iter()
            .find(|c| c.kind == NodeKind::File && c.size < 200_000)
        {
            println!();
            println!(
                "== sampling first 256 bytes of {} (id={}) ==",
                file.name, file.id
            );
            match read_file_limited(&image, &file.id, 256) {
                Ok(bytes) => {
                    let preview: String = bytes
                        .iter()
                        .take(128)
                        .map(|b| {
                            if (32..127).contains(b) {
                                *b as char
                            } else {
                                '.'
                            }
                        })
                        .collect();
                    println!("read {} bytes", bytes.len());
                    println!("preview: {}", preview);
                }
                Err(e) => println!("read failed: {e}"),
            }
        }
    }
    Ok(())
}
