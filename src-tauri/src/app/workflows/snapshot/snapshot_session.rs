//! Caseless volume-snapshot orchestration.
//!
//! When an image is loaded with NO case open, [`spawn_session_snapshot`] builds
//! the same directory-tree snapshot the case path would (`app::workflows::snapshot::case_snapshot`), but
//! installs a process-local SQLite index in
//! [`crate::state::AppState::session_snapshots`] instead of a case manifest — so
//! browsing / 全显 / child-counts are served from a snapshot even with no case,
//! and the directory structure is ready in the background right after load
//! instead of on first click.
//!
//! The `.dirz` is built to a temp scratch file, imported into a transient
//! index file (the same format as a case volume index), then deleted: the
//! snapshot lives only as long as the process. If indexing fails, the full-body
//! in-memory load is used as a fallback. Builds are cancellable background tasks registered in `AppState::tasks` under
//! `snapshot-session:<image_id>:<volume_key>` (distinct from the case path's
//! `snapshot:...`) and emit the shared [`crate::app::workflows::snapshot::case_snapshot::SNAPSHOT_PROGRESS_EVENT`]
//! events, so the existing frontend progress chip works unchanged.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::host::AppHandle;
use tokio::sync::Mutex;

use crate::app::workflows::snapshot::case_snapshot::{
    emit_progress, emit_unified_snapshot_progress, SnapshotProgressPayload, PROGRESS_THROTTLE,
};
use crate::core::casework::case::CaseState;
use crate::core::casework::case_paths;
use crate::core::catalog::filedb::filedb_store::{self, IndexSource};
use crate::core::catalog::snapshot::volume_snapshot::{
    self, snapshot_file_name, volume_size_for_key, VolumeSnapshot, DEFAULT_MAX_SNAPSHOT_ENTRIES,
};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::fs_all;
use crate::state::{SessionIndex, SessionSnapshot, SessionSnapshots};
use crate::task_manager::{TaskFlag, TaskKind, TaskManager};

type ImageStore = Arc<Mutex<HashMap<String, Arc<EvidenceSession>>>>;
type TaskStore = TaskManager;
type ActiveCase = Arc<Mutex<Option<CaseState>>>;

/// Scratch directory for transient `.dirz` and index files of caseless session
/// snapshots. A `.dirz` is deleted right after indexing, an index when its
/// session snapshot is dropped; a dedicated subdir keeps any crash-orphaned
/// file collectable and out of the temp root.
fn session_scratch_dir() -> PathBuf {
    std::env::temp_dir().join("lovelycarve-snapshots")
}

fn session_index_path(image_id: &str, volume_key: &str) -> PathBuf {
    session_scratch_dir().join(format!(
        "index-{}-{}-{}.db",
        case_paths::path_segment(image_id, "image"),
        case_paths::volume_segment(volume_key),
        uuid::Uuid::new_v4().simple()
    ))
}

fn remove_scratch_file(path: &std::path::Path) {
    if let Err(error) = case_paths::remove_database_files(path) {
        tracing::debug!(
            "session snapshot scratch {} not removed: {error}",
            path.display()
        );
    }
}

/// Loads a scratch `.dirz` as the in-memory fallback body.
async fn load_fallback_body(path: &std::path::Path) -> Result<VolumeSnapshot, String> {
    let load_path = path.to_path_buf();
    match tokio::task::spawn_blocking(move || volume_snapshot::load_snapshot(&load_path)).await {
        Ok(Ok(body)) => Ok(body),
        Ok(Err(err)) => Err(format!("load snapshot: {err}")),
        Err(join_err) => Err(format!("load task crashed: {join_err}")),
    }
}

/// Cancel any in-flight caseless session-snapshot build for `image_id` and drop
/// any session snapshot already installed for it. Used when a case takes over
/// the image (the persisted case store supersedes) and on image unload.
pub(crate) async fn purge_session_snapshots(
    session_snapshots: &SessionSnapshots,
    tasks: &TaskStore,
    image_id: &str,
) {
    tasks.cancel_image_kind(image_id, TaskKind::SessionSnapshot);
    session_snapshots
        .lock()
        .await
        .retain(|(img, _), _| img != image_id);
}

/// The session snapshot covering `parent_id` for `image_id`, if any. Mirrors
/// `CaseState::covering_snapshot` over the caseless store; the caller already
/// holds the lock guard.
pub(crate) fn covering_session_snapshot(
    store: &HashMap<(String, String), Arc<SessionSnapshot>>,
    image_id: &str,
    parent_id: &str,
) -> Option<Arc<SessionSnapshot>> {
    store
        .iter()
        .find(|((img, _), snap)| {
            img == image_id
                && snap
                    .body
                    .as_ref()
                    .is_some_and(|body| body.covers(parent_id))
        })
        .map(|(_, snap)| Arc::clone(snap))
}

/// The recorded child count of `parent_id` for `image_id` across this image's
/// session snapshots (`Some(0)` for a walked-but-empty dir, `None` when no
/// session snapshot has any record). Mirrors `CaseState::snapshot_child_count`.
pub(crate) fn session_child_count(
    store: &HashMap<(String, String), Arc<SessionSnapshot>>,
    image_id: &str,
    parent_id: &str,
) -> Option<usize> {
    store
        .iter()
        .filter(|((img, _), _)| img == image_id)
        .find_map(|(_, snap)| {
            snap.body
                .as_ref()
                .and_then(|body| body.child_count(parent_id))
        })
}

/// Whether `parent_id`'s subtree is empty of files across this image's session
/// snapshots (`Some(true)` = only empty dirs beneath, `Some(false)` = a file
/// exists somewhere, `None` = no session snapshot can answer). Mirrors
/// `CaseState::snapshot_subtree_empty` over the caseless store.
pub(crate) fn session_subtree_empty(
    store: &HashMap<(String, String), Arc<SessionSnapshot>>,
    image_id: &str,
    parent_id: &str,
) -> Option<bool> {
    store
        .iter()
        .filter(|((img, _), _)| img == image_id)
        .find_map(|(_, snap)| {
            snap.body
                .as_ref()
                .and_then(|body| body.subtree_has_file(parent_id))
        })
        .map(|has_file| !has_file)
}

/// Background build (or reuse) of one volume's snapshot into the caseless
/// session store. Fire-and-forget; mirrors `case::spawn_volume_snapshot` but
/// without any case manifest or on-disk persistence.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_session_snapshot(
    app: AppHandle,
    session_snapshots: SessionSnapshots,
    images: ImageStore,
    tasks: TaskStore,
    active_case: ActiveCase,
    image: Arc<EvidenceSession>,
    volume_key: String,
    token_id: String,
) {
    // Registration must precede spawn: a caller may immediately wait or unload.
    let Some(task) = tasks.register_if_absent(
        token_id.clone(),
        TaskKind::SessionSnapshot,
        Some(image.id.clone()),
    ) else {
        return;
    };
    task.set_automatic();
    tokio::spawn(async move {
        let key = (image.id.clone(), volume_key.clone());
        let current_size = volume_size_for_key(&image, &volume_key);

        // Reuse a fresh in-memory snapshot instead of re-walking (covers the
        // `image_load` early-return path that re-registers an already-loaded
        // image while still caseless). Re-emit `done` so the frontend chip stays
        // consistent — the case path likewise emits on reuse (`install_loaded_snapshot`).
        {
            let reuse = {
                let store = session_snapshots.lock().await;
                store.get(&key).and_then(|existing| {
                    (existing.source_volume_size == current_size).then_some((
                        existing.entry_count,
                        existing.truncated,
                        existing.index.is_some(),
                    ))
                })
            };
            if let Some((entries, truncated, indexed)) = reuse {
                task.update(|view| {
                    view.flags.insert(TaskFlag::Reused);
                });
                // The directory is available even when a previous index build failed.
                let mut payload =
                    SnapshotProgressPayload::new(&token_id, &image.id, &volume_key, "indexing");
                payload.entries = entries;
                payload.truncated = truncated;
                emit_unified_snapshot_progress(&tasks, &payload);
                emit_progress(&app, payload);
                let mut payload = SnapshotProgressPayload::new(
                    &token_id,
                    &image.id,
                    &volume_key,
                    if indexed { "done" } else { "error" },
                );
                payload.message = (!indexed).then(|| "索引未建立，当前使用内存目录".into());
                payload.entries = entries;
                payload.truncated = truncated;
                emit_unified_snapshot_progress(&tasks, &payload);
                emit_progress(&app, payload);
                return;
            }
        }

        let resource_permit = match tasks.acquire_resources(&task).await {
            Ok(permit) => permit,
            Err(err) => {
                let mut payload =
                    SnapshotProgressPayload::new(&token_id, &image.id, &volume_key, "cancelled");
                payload.message = Some(err.message());
                emit_unified_snapshot_progress(&tasks, &payload);
                emit_progress(&app, payload);
                return;
            }
        };
        let cancel = task.token();

        let payload = SnapshotProgressPayload::new(&token_id, &image.id, &volume_key, "started");
        emit_unified_snapshot_progress(&tasks, &payload);
        emit_progress(&app, payload);

        let scratch_dir = session_scratch_dir();
        if let Err(err) = std::fs::create_dir_all(&scratch_dir) {
            let mut payload =
                SnapshotProgressPayload::new(&token_id, &image.id, &volume_key, "error");
            payload.message = Some(format!("scratch dir: {err}"));
            emit_unified_snapshot_progress(&tasks, &payload);
            emit_progress(&app, payload);
            return;
        }
        let out_path = scratch_dir.join(snapshot_file_name(&image.id, &volume_key));

        let build = {
            let app = app.clone();
            let tasks = tasks.clone();
            let token_id = token_id.clone();
            let image_id = image.id.clone();
            let volume_key = volume_key.clone();
            let image = Arc::clone(&image);
            let out_path = out_path.clone();
            let cancel = cancel.clone();
            let built_at = crate::core::casework::case_ledger::now();
            tokio::task::spawn_blocking(move || {
                let _resource_permit = resource_permit;
                let mut last = Instant::now()
                    .checked_sub(PROGRESS_THROTTLE)
                    .unwrap_or_else(Instant::now);
                let mut on_progress = |entries: u64| {
                    if last.elapsed() >= PROGRESS_THROTTLE {
                        last = Instant::now();
                        let mut payload = SnapshotProgressPayload::new(
                            &token_id,
                            &image_id,
                            &volume_key,
                            "walking",
                        );
                        payload.entries = entries;
                        emit_unified_snapshot_progress(&tasks, &payload);
                        emit_progress(&app, payload);
                    }
                };
                volume_snapshot::build_volume_snapshot(
                    &image,
                    &volume_key,
                    &out_path,
                    DEFAULT_MAX_SNAPSHOT_ENTRIES,
                    built_at,
                    &cancel,
                    &mut on_progress,
                )
            })
            .await
        };

        let built = match build {
            Ok(Ok(built)) => built,
            Ok(Err(err)) => {
                let cancelled = cancel.is_cancelled();
                remove_scratch_file(&out_path);
                let mut payload = SnapshotProgressPayload::new(
                    &token_id,
                    &image.id,
                    &volume_key,
                    if cancelled { "cancelled" } else { "error" },
                );
                payload.message = Some(err.to_string());
                emit_unified_snapshot_progress(&tasks, &payload);
                emit_progress(&app, payload);
                return;
            }
            Err(join_err) => {
                remove_scratch_file(&out_path);
                let mut payload =
                    SnapshotProgressPayload::new(&token_id, &image.id, &volume_key, "error");
                payload.message = Some(format!("snapshot task crashed: {join_err}"));
                emit_unified_snapshot_progress(&tasks, &payload);
                emit_progress(&app, payload);
                return;
            }
        };

        // Import the freshly written `.dirz` into a transient index file. On
        // success, caseless browsing uses the same page-oriented store as a
        // case volume without retaining the full body in memory.
        let payload = SnapshotProgressPayload::new(&token_id, &image.id, &volume_key, "indexing");
        emit_unified_snapshot_progress(&tasks, &payload);
        emit_progress(&app, payload);
        let index_path = session_index_path(&image.id, &volume_key);
        let indexed = {
            let snapshot_path = out_path.clone();
            let index_path = index_path.clone();
            let image = Arc::clone(&image);
            let volume_key = volume_key.clone();
            let built_at = built.header.built_at.clone();
            let truncated = built.truncated;
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                let vol_id = fs_all::vol_id_for_key(&image, &volume_key)?;
                let fingerprint = image.evidence_fingerprint();
                filedb_store::build_index(
                    &snapshot_path,
                    &index_path,
                    &IndexSource {
                        image_id: &image.id,
                        volume_key: &volume_key,
                        vol_id,
                        built_at: &built_at,
                        fingerprint: &fingerprint,
                        truncated,
                    },
                    None,
                    Some(&cancel),
                )?;
                anyhow::Ok(vol_id)
            })
            .await
        };
        let index_outcome = match indexed {
            Ok(Ok(vol_id)) => Ok(SessionIndex {
                path: index_path,
                vol_id,
            }),
            Ok(Err(err)) => Err(format!("{err:#}")),
            Err(join_err) => Err(format!("index task crashed: {join_err}")),
        };
        let (body, index) = match index_outcome {
            Ok(index) => (None, Some(index)),
            Err(_) if cancel.is_cancelled() => {
                remove_scratch_file(&out_path);
                let payload =
                    SnapshotProgressPayload::new(&token_id, &image.id, &volume_key, "cancelled");
                emit_unified_snapshot_progress(&tasks, &payload);
                emit_progress(&app, payload);
                return;
            }
            Err(error) => {
                tracing::warn!(
                    "session snapshot index fallback image={} volume={} err={error}",
                    image.id,
                    volume_key
                );
                match load_fallback_body(&out_path).await {
                    Ok(body) => (Some(Arc::new(body)), None),
                    Err(message) => {
                        remove_scratch_file(&out_path);
                        let mut payload = SnapshotProgressPayload::new(
                            &token_id,
                            &image.id,
                            &volume_key,
                            "error",
                        );
                        payload.message = Some(message);
                        emit_unified_snapshot_progress(&tasks, &payload);
                        emit_progress(&app, payload);
                        return;
                    }
                }
            }
        };
        remove_scratch_file(&out_path);

        let index_failed = index.is_none();
        // Install — but first re-validate under the `session_snapshots` lock,
        // mirroring the case path's install-time guards (case.rs): this detached
        // build may have raced an unload or a case-open while it was walking /
        // loading. Bail if cancelled, if the image was unloaded (its slot was
        // purged), or if a case now owns the image (the persisted case snapshot
        // supersedes — `register_and_snapshot`'s purge already cleared our slot).
        // Checking all three while holding `session_snapshots` makes the decision
        // atomic with the insert: a concurrent `purge_session_snapshots` runs
        // either entirely before us (we observe it and bail) or entirely after
        // (it clears the entry we just inserted) — never resurrecting a slot.
        // Holding the session lock across the `images` / `active_case`
        // acquisitions is deadlock-free: no path ever holds either while awaiting
        // `session_snapshots` (both fast paths and `purge` drop them first), so
        // `session_snapshots` is always the outermost of the three here.
        {
            let mut store = session_snapshots.lock().await;
            let superseded = cancel.is_cancelled()
                || !images.lock().await.contains_key(&image.id)
                || active_case.lock().await.is_some();
            if superseded {
                drop(store);
                let mut payload =
                    SnapshotProgressPayload::new(&token_id, &image.id, &volume_key, "cancelled");
                payload.message = Some("superseded by case or unload during build".into());
                emit_unified_snapshot_progress(&tasks, &payload);
                emit_progress(&app, payload);
                return;
            }
            store.insert(
                key,
                Arc::new(SessionSnapshot {
                    body,
                    index,
                    volume_key: volume_key.clone(),
                    source_volume_size: built.header.source_volume_size,
                    entry_count: built.entry_count,
                    truncated: built.truncated,
                }),
            );
        }

        let mut payload = SnapshotProgressPayload::new(
            &token_id,
            &image.id,
            &volume_key,
            if index_failed { "error" } else { "done" },
        );
        payload.message =
            index_failed.then(|| "索引建立失败，已回退到内存目录；可重新建立索引".into());
        payload.entries = built.entry_count;
        payload.truncated = built.truncated;
        emit_unified_snapshot_progress(&tasks, &payload);
        emit_progress(&app, payload);
    });
}
