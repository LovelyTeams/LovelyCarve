//! Reproduces the exact sequence the desktop app runs against a freshly-loaded
//! physical disk and prints how long each step took. Helps localise where
//! the "正在读取目录…" hang lives — `open_block_device`, partition
//! detection, the first `fs_children("")`, or the per-partition
//! `fs_children("part:N")` (which opens the NTFS / FAT / Ext volume).
//!
//! Usage (Administrator required to read raw block devices on Windows):
//!
//! ```text
//! just run-example probe_physical_disk 0                # \\.\PhysicalDrive0
//! just run-example probe_physical_disk /dev/sda         # explicit path
//! just run-example probe_physical_disk 0 --skip-volume
//! ```
//!
//! `--skip-volume` stops after the partition-level listing and does NOT
//! attempt to open each filesystem — useful when you suspect the hang is
//! inside `NtfsVolume::open`.

use std::env;
use std::sync::Arc;
use std::time::Instant;

use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let mut args = env::args().skip(1);
    let raw = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: probe_physical_disk <N | path> [--skip-volume]"))?;
    let skip_volume = args.any(|a| a == "--skip-volume");

    let path = resolve_path(&raw);
    println!("== probe_physical_disk ==");
    println!("path: {path}");
    println!();

    // 1. open + partition detection
    let t = Instant::now();
    let image = EvidenceSession::open(&path)?;
    let open_elapsed = t.elapsed();

    let info = image.info();
    println!(
        "[ {:>8.3?} ] EvidenceSession::open + partition detect → {} partitions, {} ({} sectors @ {} bytes)",
        open_elapsed,
        info.partitions.len(),
        human(info.size_bytes),
        info.size_bytes / info.sector_size as u64,
        info.sector_size,
    );
    println!("    format: {:?}", info.source_format);
    for p in &info.partitions {
        println!(
            "    #{id:<3} kind={kind:<14} start_lba={slba:<12} length_lba={llba:<12} size={size} label={label:?}",
            id = p.id,
            kind = format!("{:?}", p.kind),
            slba = p.start_lba,
            llba = p.length_lba,
            size = human(p.length_lba * info.sector_size as u64),
            label = p.label
        );
    }
    println!();

    let image = Arc::new(image);

    // 2. fs_children("") — what the UI calls first after image_load
    let t = Instant::now();
    let roots = fs_tree::children_for_image(&image, "")?;
    println!(
        "[ {:>8.3?} ] fs_children(\"\") → {} nodes",
        t.elapsed(),
        roots.len(),
    );
    for n in &roots {
        println!("    id={:<24} name={:?} kind={:?}", n.id, n.name, n.kind);
    }
    println!();

    if skip_volume {
        println!("(skipped per-partition fs_children — pass without --skip-volume to drill in)");
        return Ok(());
    }

    // 3. fs_children("part:N") for each top-level node — this is where the
    //    "正在读取目录…" hang typically lives. We open every partition
    //    sequentially so the wall-clock cost of each one is obvious.
    for n in &roots {
        if !n.id.starts_with("part:") {
            continue;
        }
        let t = Instant::now();
        let result = fs_tree::children_for_image(&image, &n.id);
        let elapsed = t.elapsed();
        match result {
            Ok(children) => println!(
                "[ {:>8.3?} ] fs_children({:?}) → {} children",
                elapsed,
                n.id,
                children.len(),
            ),
            Err(err) => println!(
                "[ {:>8.3?} ] fs_children({:?}) → ERROR: {err:#}",
                elapsed, n.id
            ),
        }
    }
    println!();
    println!("done.");
    Ok(())
}

fn resolve_path(raw: &str) -> String {
    // Accept bare drive numbers as a convenience: `0` → `\\.\PhysicalDrive0`.
    if raw.bytes().all(|b| b.is_ascii_digit()) {
        if cfg!(windows) {
            return format!(r"\\.\PhysicalDrive{raw}");
        } else {
            return format!("/dev/sd{raw}");
        }
    }
    raw.to_string()
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
