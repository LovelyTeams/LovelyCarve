//! Times a Btrfs volume-snapshot build on an mdadm-assembled array, to
//! compare the single-scan fast path against the generic per-directory
//! walker on real multi-image evidence.
//!
//! Usage:
//!   cargo run --release --example probe_btrfs_snapshot -- <image1> [<image2> ...] \
//!       [--generic-cap N] [--verify N]
//!
//! Loads every image, runs RAID auto-detection, registers each assembled
//! array as a synthetic `DiskImage` (the same path the GUI takes), then for
//! its Btrfs partition:
//!   1. builds a snapshot through `build_volume_snapshot` (fast path) with
//!      timing;
//!   2. re-walks the same volume through the generic `SnapshotWalker`, capped
//!      at `--generic-cap` entries (default 100000), and extrapolates;
//!   3. with `--verify N`, breadth-first samples N directories and asserts
//!      the snapshot listing equals the live listing JSON-byte-for-byte.
//!
//! A single image whose partition table already exposes a Btrfs partition is
//! probed directly without RAID assembly.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use lovelycarve_lib::core::catalog::snapshot::volume_snapshot::{
    build_volume_snapshot, load_snapshot, DEFAULT_MAX_SNAPSHOT_ENTRIES,
};
use lovelycarve_lib::core::evidence::image::{DiskImage, Partition, PartitionKind};
use lovelycarve_lib::core::evidence::raid;
use lovelycarve_lib::core::filesystem::tree::fs_tree::{
    children_for_image, NodeKind, SnapshotWalker,
};
use tokio_util::sync::CancellationToken;

fn main() -> anyhow::Result<()> {
    let mut paths: Vec<String> = Vec::new();
    let mut generic_cap: usize = 100_000;
    let mut reopen_cap: usize = 0;
    let mut verify: usize = 0;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--generic-cap" => {
                generic_cap = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--generic-cap N");
            }
            "--reopen-cap" => {
                reopen_cap = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--reopen-cap N");
            }
            "--verify" => {
                verify = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--verify N");
            }
            other => paths.push(other.to_string()),
        }
    }
    if paths.is_empty() {
        eprintln!(
            "usage: probe_btrfs_snapshot <image1> [<image2> ...] [--generic-cap N] [--verify N]"
        );
        std::process::exit(2);
    }

    let mut images = Vec::new();
    for path in &paths {
        println!("loading: {path}");
        images.push(Arc::new(DiskImage::open(path)?));
    }

    // Prefer a Btrfs partition directly on a loaded image; otherwise run RAID
    // assembly across the set and register the first Btrfs array.
    let target: Arc<DiskImage> = match images.iter().find(|image| {
        image
            .partitions
            .iter()
            .any(|p| p.kind == PartitionKind::Btrfs)
    }) {
        Some(image) => Arc::clone(image),
        None => {
            println!("\n== RAID auto-detection ==");
            let (stack, assembled) = raid::detect_with_sources(&images);
            for line in &stack.diagnostics {
                println!("  ! {line}");
            }
            let array = assembled
                .iter()
                .find(|a| a.fs_kind == PartitionKind::Btrfs)
                .expect("no assembled btrfs array detected");
            println!(
                "assembled: set={} ({}) {} bytes",
                array.set_uuid,
                if array.set_name.is_empty() {
                    "<unnamed>"
                } else {
                    &array.set_name
                },
                array.size_bytes
            );
            let sector_size = 512usize;
            let partitions = vec![Partition {
                id: 0,
                kind: array.fs_kind,
                start_lba: 0,
                length_lba: array.size_bytes / sector_size as u64,
                label: Some(raid::synthetic_partition_label(array)),
                unique_guid: None,
                amiga_geometry: None,
            }];
            Arc::new(DiskImage::from_assembled(
                format!("md:{}", array.set_uuid),
                array.source.clone(),
                partitions,
                sector_size,
            )?)
        }
    };

    let target = Arc::new(
        lovelycarve_lib::core::filesystem::evidence_session::EvidenceSession::from(target),
    );
    let partition = target
        .partitions
        .iter()
        .find(|p| p.kind == PartitionKind::Btrfs)
        .expect("btrfs partition")
        .clone();
    let volume_key = format!("part:{}", partition.id);
    println!("\nvolume_key: {volume_key} (image {})", target.id);

    // -- fast path ----------------------------------------------------------
    let tmp = std::env::temp_dir().join("probe_btrfs_snapshot.dirz");
    let cancel = CancellationToken::new();
    let mut last = 0u64;
    let started = Instant::now();
    let built = build_volume_snapshot(
        &target,
        &volume_key,
        &tmp,
        DEFAULT_MAX_SNAPSHOT_ENTRIES,
        "2026-06-12T00:00:00Z".to_string(),
        &cancel,
        &mut |n| {
            if n >= last + 100_000 {
                last = n;
                println!("  .. {n} entries");
            }
        },
    )?;
    let fast = started.elapsed();
    println!(
        "FAST PATH: {} entries, truncated={}, in {:.2}s ({:.0} entries/s), {} bytes",
        built.entry_count,
        built.truncated,
        fast.as_secs_f64(),
        built.entry_count as f64 / fast.as_secs_f64().max(0.001),
        std::fs::metadata(&tmp)?.len(),
    );

    // -- generic walker (capped) ---------------------------------------------
    if generic_cap > 0 {
        let started = Instant::now();
        let mut walker = SnapshotWalker::new(&target);
        let mut stack: Vec<String> = vec![volume_key.clone()];
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(volume_key.clone());
        let mut count = 0usize;
        'walk: while let Some(parent_id) = stack.pop() {
            let children = match walker.children(&parent_id) {
                Ok(children) => children,
                Err(err) => {
                    eprintln!("  generic: list {parent_id} failed: {err}");
                    continue;
                }
            };
            for child in children {
                count += 1;
                if count >= generic_cap {
                    break 'walk;
                }
                if child.kind == NodeKind::Dir
                    && !child.deleted
                    && !child.id.starts_with("orphans:")
                    && visited.insert(child.id.clone())
                {
                    stack.push(child.id);
                }
            }
        }
        let generic = started.elapsed();
        let rate = count as f64 / generic.as_secs_f64().max(0.001);
        println!(
            "GENERIC WALK: {count} entries in {:.2}s ({rate:.0} entries/s) → full volume est. {:.0}s",
            generic.as_secs_f64(),
            built.entry_count as f64 / rate.max(0.001),
        );
    }

    // -- historical behaviour: children_for_image per directory (reopens the
    // volume every listing), capped -------------------------------------------
    if reopen_cap > 0 {
        let started = Instant::now();
        let mut stack: Vec<String> = vec![volume_key.clone()];
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(volume_key.clone());
        let mut count = 0usize;
        'reopen: while let Some(parent_id) = stack.pop() {
            let children = match children_for_image(&target, &parent_id) {
                Ok(children) => children,
                Err(err) => {
                    eprintln!("  reopen: list {parent_id} failed: {err}");
                    continue;
                }
            };
            for child in children {
                count += 1;
                if count >= reopen_cap {
                    break 'reopen;
                }
                if child.kind == NodeKind::Dir
                    && !child.deleted
                    && !child.id.starts_with("orphans:")
                    && visited.insert(child.id.clone())
                {
                    stack.push(child.id);
                }
            }
        }
        let reopen = started.elapsed();
        let rate = count as f64 / reopen.as_secs_f64().max(0.001);
        println!(
            "REOPEN-PER-DIR (old): {count} entries in {:.2}s ({rate:.0} entries/s) → full volume est. {:.0}s",
            reopen.as_secs_f64(),
            built.entry_count as f64 / rate.max(0.001),
        );
    }

    // -- equivalence sample ---------------------------------------------------
    if verify > 0 {
        let snapshot = load_snapshot(&tmp)?;
        let mut queue: Vec<String> = vec![volume_key.clone()];
        let mut compared = 0usize;
        let mut mismatches = 0usize;
        while let Some(parent_id) = queue.pop() {
            if compared >= verify {
                break;
            }
            compared += 1;
            let live = children_for_image(&target, &parent_id)?;
            match snapshot.children(&parent_id) {
                Some(from_snapshot) => {
                    if serde_json::to_value(&from_snapshot)? != serde_json::to_value(&live)? {
                        mismatches += 1;
                        eprintln!("  MISMATCH at {parent_id}");
                    }
                }
                None if !live.is_empty() => {
                    mismatches += 1;
                    eprintln!("  UNCOVERED non-empty dir {parent_id}");
                }
                None => {}
            }
            for node in live {
                if node.kind == NodeKind::Dir && !node.deleted {
                    queue.push(node.id);
                }
            }
        }
        println!("VERIFY: {compared} directories sampled, {mismatches} mismatches");
        if mismatches > 0 {
            std::process::exit(1);
        }
    }

    Ok(())
}
