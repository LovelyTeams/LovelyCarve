//! Imports a published volume snapshot into its index file and publishes the
//! index, reporting the real outcome to the task center. When the import
//! fails the snapshot body is loaded into memory instead, so browsing keeps a
//! fast path.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::host::AppHandle;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::app::workflows::snapshot::case_snapshot::{
    emit_progress, remove_staged_file, report, SnapshotProgressPayload, PROGRESS_THROTTLE,
};
use crate::core::casework::case::CaseState;
use crate::core::catalog::filedb::filedb_store::{self, IndexSource};
use crate::core::catalog::snapshot::volume_snapshot;
use crate::task_manager::{TaskManager, TaskResourcePermit};

/// One index import of a published snapshot.
pub(crate) struct IndexJob {
    pub(crate) task_id: String,
    pub(crate) case_dir: PathBuf,
    pub(crate) image_id: String,
    pub(crate) volume_key: String,
    /// The published `.dirz` in the volume directory.
    pub(crate) snapshot_path: PathBuf,
    pub(crate) staging_dir: PathBuf,
    /// `built_at` of the published snapshot; the index is published only
    /// while that snapshot is still the current one.
    pub(crate) built_at: String,
    pub(crate) entry_count: u64,
    pub(crate) truncated: bool,
    pub(crate) vol_id: i64,
    pub(crate) fingerprint: String,
    pub(crate) cancel: CancellationToken,
}

impl IndexJob {
    fn payload(
        &self,
        phase: &str,
        entries: u64,
        message: Option<String>,
    ) -> SnapshotProgressPayload {
        let mut payload =
            SnapshotProgressPayload::new(&self.task_id, &self.image_id, &self.volume_key, phase);
        payload.entries = entries;
        payload.truncated = self.truncated;
        payload.message = message;
        payload
    }

    fn reused(&self) -> SnapshotProgressPayload {
        let mut payload =
            SnapshotProgressPayload::new("", &self.image_id, &self.volume_key, "reused");
        payload.entries = self.entry_count;
        payload
    }
}

/// Builds the index of `job.snapshot_path` in the staging directory (outside
/// the case lock) and publishes it. The resource permit is held until the
/// import finishes.
pub(crate) async fn index_published_snapshot(
    app: &AppHandle,
    active_case: &Arc<Mutex<Option<CaseState>>>,
    tasks: &TaskManager,
    job: IndexJob,
    resource_permit: TaskResourcePermit,
) {
    report(
        app,
        tasks,
        job.payload("indexing", 0, Some("正在写入索引".into())),
    );
    let staged_index = filedb_store::staged_index_path(&job.staging_dir, &job.volume_key);
    let built = {
        let app = app.clone();
        let tasks = tasks.clone();
        let progress = job.payload("indexing", 0, None);
        let snapshot_path = job.snapshot_path.clone();
        let staged_index = staged_index.clone();
        let image_id = job.image_id.clone();
        let volume_key = job.volume_key.clone();
        let built_at = job.built_at.clone();
        let fingerprint = job.fingerprint.clone();
        let (vol_id, truncated) = (job.vol_id, job.truncated);
        let cancel = job.cancel.clone();
        tokio::task::spawn_blocking(move || {
            let _resource_permit = resource_permit;
            let mut last = Instant::now()
                .checked_sub(PROGRESS_THROTTLE)
                .unwrap_or_else(Instant::now);
            let mut on_progress = |entries: u64| {
                if last.elapsed() >= PROGRESS_THROTTLE {
                    last = Instant::now();
                    let mut payload = progress.clone();
                    payload.entries = entries;
                    report(&app, &tasks, payload);
                }
            };
            filedb_store::build_index(
                &snapshot_path,
                &staged_index,
                &IndexSource {
                    image_id: &image_id,
                    volume_key: &volume_key,
                    vol_id,
                    built_at: &built_at,
                    fingerprint: &fingerprint,
                    truncated,
                },
                Some(&mut on_progress),
                Some(&cancel),
            )
        })
        .await
    };
    let failure = match built {
        Ok(Ok(rows)) => {
            tracing::debug!(
                "case: indexed snapshot image={} volume={} rows={rows}",
                job.image_id,
                job.volume_key
            );
            match publish_staged_index(active_case, &job, &staged_index).await {
                Ok(true) => {
                    report(app, tasks, job.payload("done", job.entry_count, None));
                    emit_progress(app, job.reused());
                    return;
                }
                Ok(false) => {
                    report(
                        app,
                        tasks,
                        job.payload(
                            "cancelled",
                            job.entry_count,
                            Some("案件或检材已切换".into()),
                        ),
                    );
                    return;
                }
                Err(err) => {
                    remove_staged_file(&staged_index);
                    format!("{err:#}")
                }
            }
        }
        Ok(Err(_)) if job.cancel.is_cancelled() => {
            // Cancelled mid-import: stop — do NOT fall back to the in-memory
            // body (millions of nodes into RAM defeats the cancel). The
            // published `.dirz` stays for a later rebuild to reuse.
            report(app, tasks, job.payload("cancelled", 0, None));
            return;
        }
        Ok(Err(err)) => format!("{err:#}"),
        Err(join_err) => format!("index task crashed: {join_err}"),
    };
    tracing::warn!(
        "case: index snapshot image={} volume={} failed: {failure}",
        job.image_id,
        job.volume_key
    );
    report(
        app,
        tasks,
        job.payload("error", 0, Some(format!("建立索引失败：{failure}"))),
    );
    install_snapshot_body(app, active_case, &job).await;
}

/// Publishes the staged index on a blocking thread. `Ok(false)` when the case
/// or evidence changed or the snapshot was replaced meanwhile; the staged
/// index is removed then.
async fn publish_staged_index(
    active_case: &Arc<Mutex<Option<CaseState>>>,
    job: &IndexJob,
    staged_index: &Path,
) -> anyhow::Result<bool> {
    let active_case = Arc::clone(active_case);
    let case_dir = job.case_dir.clone();
    let image_id = job.image_id.clone();
    let volume_key = job.volume_key.clone();
    let built_at = job.built_at.clone();
    let staged_index = staged_index.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut guard = active_case.blocking_lock();
        let Some(case) = guard
            .as_mut()
            .filter(|case| case.dir == case_dir && case.has_evidence(&image_id))
        else {
            remove_staged_file(&staged_index);
            return Ok(false);
        };
        case.publish_index(&image_id, &volume_key, &built_at, &staged_index)
    })
    .await
    .map_err(|join_err| anyhow::anyhow!("publish index task crashed: {join_err}"))?
}

/// Fallback when the index cannot be built: keep the snapshot body in memory.
async fn install_snapshot_body(
    app: &AppHandle,
    active_case: &Arc<Mutex<Option<CaseState>>>,
    job: &IndexJob,
) {
    let path = job.snapshot_path.clone();
    let loaded = tokio::task::spawn_blocking(move || volume_snapshot::load_snapshot(&path)).await;
    let snapshot = match loaded {
        Ok(Ok(snapshot)) => Arc::new(snapshot),
        Ok(Err(err)) => {
            tracing::warn!("case: load snapshot {} body failed: {err}", job.volume_key);
            return;
        }
        Err(join_err) => {
            tracing::warn!("case: load snapshot task crashed: {join_err}");
            return;
        }
    };
    let mut guard = active_case.lock().await;
    if let Some(case) = guard.as_mut() {
        if case.dir == job.case_dir && case.has_evidence(&job.image_id) {
            case.install_snapshot(snapshot);
            emit_progress(app, job.reused());
        }
    }
}
