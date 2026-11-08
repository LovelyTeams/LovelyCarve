//! Times a volume-snapshot build for one volume key, to compare the generic
//! per-directory walker against any fast path.
//!
//! Usage: cargo run --example probe_snapshot -- <image> <volume_key>
//!   e.g. cargo run --example probe_snapshot -- D:\VM\api.E01 part:2

use std::sync::Arc;
use std::time::Instant;

use lovelycarve_core::catalog::snapshot::volume_snapshot::{
    build_volume_snapshot, DEFAULT_MAX_SNAPSHOT_ENTRIES,
};
use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{children_for_image, NodeKind};
use tokio_util::sync::CancellationToken;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: probe_snapshot <image> <volume_key>");
    let volume_key = args.next().unwrap_or_else(|| "part:2".to_string());

    let image = Arc::new(EvidenceSession::open(&path)?);
    println!("image: {} ({:?})", path, image.info().source_format);
    println!("volume_key: {volume_key}");

    // What does the first level look like?
    let top = children_for_image(&image, &volume_key)?;
    println!("top-level children: {}", top.len());
    for node in top.iter().take(8) {
        println!("  {:<28} {:?}", node.id, node.kind);
    }

    let tmp = std::env::temp_dir().join(format!(
        "probe_snapshot_{}.dirz",
        volume_key.replace([':'], "_")
    ));
    let cancel = CancellationToken::new();
    let mut last = 0u64;
    let step: u64 = std::env::var("PROBE_STEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50_000);
    let mut on_progress = |n: u64| {
        if n >= last + step {
            last = n;
            println!("  .. {n} entries");
        }
    };

    let started = Instant::now();
    let built = build_volume_snapshot(
        &image,
        &volume_key,
        &tmp,
        DEFAULT_MAX_SNAPSHOT_ENTRIES,
        "2026-05-29T00:00:00Z".to_string(),
        &cancel,
        &mut on_progress,
    )?;
    let elapsed = started.elapsed();

    println!(
        "\nBUILD DONE: {} entries, truncated={}, in {:.2}s ({:.0} entries/s)",
        built.entry_count,
        built.truncated,
        elapsed.as_secs_f64(),
        built.entry_count as f64 / elapsed.as_secs_f64().max(0.001),
    );
    println!("file: {} bytes", std::fs::metadata(&tmp)?.len());

    // Sanity: how many of the entries are directories vs files?
    let _ = NodeKind::Dir;
    Ok(())
}
