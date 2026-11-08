//! HFS+ / DMG end-to-end probe. Verifies the full pipeline:
//!   1. Open a `.dmg` → udif decompresses the main partition into a tempfile
//!      → mmap'd as `ImageSource::Dmg` → partition detection sees a single
//!      whole-device HFS+ / HFSX volume.
//!   2. Walk the volume root via the lovelycarve dispatcher
//!      (`children_for_image`) and read the first reasonably-sized file.
//!
//! Use this to sanity-check HFS+ support after touching either the
//! vendored `udif` / `hfsplus` crates or the dispatcher wiring.
//!
//! Usage: cargo run --example probe_hfsplus -- <image.dmg>
use std::env;
use std::sync::Arc;

use lovelycarve_core::evidence::image::{ImageFormat, PartitionKind};
use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{
    children_for_image, file_stat, read_file_limited, NodeKind,
};

fn main() -> anyhow::Result<()> {
    let path = env::args()
        .nth(1)
        .expect("usage: probe_hfsplus <image.dmg>");
    let image = Arc::new(EvidenceSession::open(&path)?);
    let info = image.info();
    println!("image:        {}", info.path);
    println!("source:       {:?}", info.source_format);
    println!(
        "size:         {} bytes ({:.2} MiB)",
        info.size_bytes,
        info.size_bytes as f64 / (1024.0 * 1024.0)
    );
    println!("sector_size:  {}", info.sector_size);

    if !matches!(info.source_format, ImageFormat::Dmg | ImageFormat::Raw) {
        eprintln!(
            "WARN: probe expects a .dmg or raw HFS+ image, got {:?}",
            info.source_format
        );
    }

    println!("\npartitions ({}):", info.partitions.len());
    for partition in &info.partitions {
        println!(
            "  #{} {:?} start={} length_lba={} label={:?}",
            partition.id,
            partition.kind,
            partition.start_lba,
            partition.length_lba,
            partition.label
        );
    }

    let hfs_part = info
        .partitions
        .iter()
        .find(|p| matches!(p.kind, PartitionKind::HfsPlus | PartitionKind::HfsX))
        .expect("no HFS+ / HFSX partition found inside DMG");
    println!(
        "\nselected partition: #{} kind={:?}",
        hfs_part.id, hfs_part.kind
    );

    let part_id = format!("part:{}", hfs_part.id);
    let root = children_for_image(&image, &part_id)?;
    println!("\nroot listing ({} entries):", root.len());
    for node in &root {
        println!(
            "  {:?} {:>10} {} (id={})",
            node.kind, node.size, node.name, node.id
        );
    }

    // Pick the first small-ish file we can find at the root, read it and
    // call file_stat — exercises parse_hfsplus_id end-to-end. Prefer files
    // whose name doesn't start with `.` so the probe shows actual content
    // instead of journaling/metadata zeros.
    let probe_target = root
        .iter()
        .find(|node| {
            node.kind == NodeKind::File
                && node.size > 0
                && node.size <= 64 * 1024
                && !node.name.starts_with('.')
        })
        .or_else(|| {
            root.iter()
                .find(|node| node.kind == NodeKind::File && node.size > 0 && node.size <= 64 * 1024)
        })
        .cloned();

    if let Some(file) = probe_target {
        let bytes = read_file_limited(&image, &file.id, 64 * 1024)?;
        println!(
            "\nread {} ({} bytes requested, got {} bytes); first 32 bytes: {}",
            file.name,
            file.size,
            bytes.len(),
            bytes
                .iter()
                .take(32)
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        if let Ok(text) = std::str::from_utf8(&bytes) {
            println!("text: {text:?}");
        }
        let stat = file_stat(&image, &file.id)?;
        println!(
            "stat: size={} mime={} mode={:?} entropy={:?}",
            stat.size, stat.mime, stat.mode, stat.entropy
        );
    } else {
        println!("\n(no small file at root for read+stat probe)");
    }

    // Walk the first "normal" directory (skip the canonical HFS+ private
    // data directories whose names contain embedded NULs and would need
    // special-case path escaping).
    if let Some(dir) = root.iter().find(|node| {
        node.kind == NodeKind::Dir && !node.name.contains('\0') && !node.name.starts_with(' ')
    }) {
        let dir_children = children_for_image(&image, &dir.id)?;
        println!("\n{} ({} entries):", dir.name, dir_children.len());
        for node in dir_children.iter().take(12) {
            println!("  {:?} {:>10} {}", node.kind, node.size, node.name);
        }
    }

    Ok(())
}
