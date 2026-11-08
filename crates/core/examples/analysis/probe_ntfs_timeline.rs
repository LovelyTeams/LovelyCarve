//! Read-only USN/timeline probe: just run-example probe_ntfs_timeline <image>
use anyhow::Context;
use lovelycarve_core::{
    filesystem::evidence_session::EvidenceSession, filesystem::fs_all::FsAll,
    filesystem::tree::fs_tree::open_ntfs_volume_for_key,
};
use lovelycarve_core::{timeline::timeline_project, timeline::timeline_store};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: probe_ntfs_timeline <image>")?;
    let image = Arc::new(EvidenceSession::open(&path)?);
    for entry in FsAll::for_image(&image).entries() {
        println!("{} {:?}", entry.volume_key, entry.kind);
        let Some(volume) = open_ntfs_volume_for_key(&image, &entry.volume_key) else {
            continue;
        };
        let volume = volume?;
        for node in volume.children(11)? {
            if node.name == "$UsnJrnl" {
                println!(
                    "journal node={} streams={:?}",
                    node.id,
                    volume.list_data_streams(
                        node.id.rsplit(':').next().context("record")?.parse()?
                    )?
                );
            }
        }
        let records = volume.read_usn_journal(64 * 1024 * 1024)?;
        println!(
            "journal records={} first={:?} last={:?}",
            records.len(),
            records.first(),
            records.last()
        );
    }
    let case = tempfile::tempdir()?;
    let cancel = CancellationToken::new();
    let progress = |done, total, rows| {
        if rows % 100_000 == 0 {
            println!("timeline progress={done}/{total} rows={rows}");
        }
    };
    let first = timeline_project::run_rebuild_with_image(case.path(), &image, &cancel, &progress)?;
    let status = timeline_project::compute_status(case.path(), &image.id, false)?;
    println!(
        "timeline={first:?} range={:?}..{:?} records={}",
        status.start_time, status.end_time, status.record_count
    );
    let conn = timeline_store::open_reader(case.path(), &image.id)?
        .context("the rebuild created the timeline")?;
    println!("types={:?}", timeline_store::types(&conn)?);
    let second = timeline_project::run_rebuild_with_image(case.path(), &image, &cancel, &progress)?;
    anyhow::ensure!(first == second, "rebuild summary changed");
    let repeated = timeline_project::compute_status(case.path(), &image.id, false)?;
    anyhow::ensure!(
        repeated.record_count == status.record_count,
        "rebuild duplicated records"
    );
    let partial =
        timeline_project::run_rebuild_with_image(case.path(), &image, &cancel, &|_, _, rows| {
            if rows >= 2000 {
                cancel.cancel();
            }
        })?;
    anyhow::ensure!(partial.status == "cancelled", "cancellation was ignored");
    println!("idempotence verified; cancellation={partial:?}");
    Ok(())
}
