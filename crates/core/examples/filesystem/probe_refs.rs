//! Quick ReFS-volume probe via the librefs-13.dll FFI shim.
//!
//! Usage:
//!   cargo run --example probe_refs -- <image> [path]
//!
//! The probe loads `<image>` (raw / E01 / VMDK — anything `DiskImage`
//! understands), finds the first `PartitionKind::Refs` partition,
//! opens it through `core::filesystem::volumes::refs::RefsVolume` (which mounts via
//! `sys_device_open_callbacks` — no temp file, no Dokan), lists either
//! the volume root or `<path>` if supplied, and reads the first 256
//! bytes of any regular file encountered for a sanity check.
//!
//! Bring the matching DLL on the loader path (the repo root or
//! `target/<profile>/examples/` are picked up automatically by the
//! `dll_search_path` helper in `core::platform::ffi::common`).

use std::env;
use std::sync::Arc;

use lovelycarve_core::evidence::image::{DiskImage, PartitionKind};
use lovelycarve_core::filesystem::tree::fs_tree::NodeKind;
use lovelycarve_core::filesystem::volumes::refs::{open_refs_source, RefsSource, RefsVolume};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut args = env::args().skip(1);
    let image_path = args.next().expect("usage: probe_refs <image> [path]");
    let probe_path = args.next();

    let image = Arc::new(DiskImage::open(&image_path)?);
    println!("image: {image_path}");
    println!("partitions: {}", image.partitions.len());
    for p in &image.partitions {
        println!(
            "  #{} kind={:?} start_lba={} length_lba={}",
            p.id, p.kind, p.start_lba, p.length_lba
        );
    }
    let part = image
        .partitions
        .iter()
        .find(|p| p.kind == PartitionKind::Refs)
        .expect("no ReFS partition detected");
    println!(
        "opening ReFS on partition #{} (offset=0x{:x}, length={} bytes)",
        part.id,
        part.start_lba * image.sector_size as u64,
        part.length_lba * image.sector_size as u64
    );

    let volume: RefsVolume = open_refs_source(&image, RefsSource::Partition(part.id))?;

    let list_path = probe_path.as_deref().unwrap_or("");
    let entries = volume.children(list_path)?;
    println!(
        "listing `{}`: {} entries",
        if list_path.is_empty() { "/" } else { list_path },
        entries.len()
    );
    for entry in entries.iter().take(40) {
        let kind = match entry.kind {
            NodeKind::Dir => "DIR ",
            NodeKind::File => "FILE",
            NodeKind::Symlink => "LINK",
            NodeKind::Unallocated => "UNL ",
        };
        let mtime = entry.timestamps.modified.clone().unwrap_or_default();
        println!("  {kind} {:>12} {:<24} {}", entry.size, mtime, entry.name);
    }

    // Read up to 256 bytes of the first regular file in the listing to
    // verify that fsapi_node_read + the buffer iohandler are wired up.
    if let Some(file_entry) = entries.iter().find(|e| e.kind == NodeKind::File) {
        let file_path = if list_path.is_empty() {
            format!("/{}", file_entry.name)
        } else {
            format!("{list_path}/{}", file_entry.name)
        };
        match volume.read_file(&file_path, 256) {
            Ok(bytes) => {
                println!("read `{file_path}` -> {} bytes, first 64 hex:", bytes.len());
                let head = &bytes[..bytes.len().min(64)];
                let hex = head
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("    {hex}");
            }
            Err(err) => println!("read `{file_path}` FAILED: {err:#}"),
        }
    }

    Ok(())
}
