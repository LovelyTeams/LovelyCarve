//! build implementation.
use super::*;
use crate::core::filesystem::evidence_session::EvidenceSession;

/// Background build (or reuse) of one volume's snapshot and index.
/// Fire-and-forget; publishes the results into the active case.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_volume_snapshot(
    app: AppHandle,
    active_case: Arc<Mutex<Option<CaseState>>>,
    tasks: TaskManager,
    image: Arc<EvidenceSession>,
    volume_key: String,
    token_id: String,
    force_rebuild: bool,
) {
    // Claim before spawning so wait/shutdown cannot overlook a not-yet-polled build.
    let Some(task) = tasks.register_if_absent(
        token_id.clone(),
        TaskKind::VolumeSnapshot,
        Some(image.id.clone()),
    ) else {
        tracing::debug!(
            "case: snapshot already active image={} volume={volume_key}",
            image.id
        );
        return;
    };
    // Evidence joining the case builds its snapshots on its own; a rebuild
    // is the user's.
    if !force_rebuild {
        task.set_automatic();
    }
    tokio::spawn(async move {
        run_volume_snapshot(
            &app,
            &active_case,
            &tasks,
            image,
            &volume_key,
            &token_id,
            force_rebuild,
            task,
        )
        .await;
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_volume_snapshot(
    app: &AppHandle,
    active_case: &Arc<Mutex<Option<CaseState>>>,
    tasks: &TaskManager,
    image: Arc<EvidenceSession>,
    volume_key: &str,
    token_id: &str,
    force_rebuild: bool,
    task: TaskRegistration,
) {
    let fail = |phase: &str, message: String| {
        let mut payload = SnapshotProgressPayload::new(token_id, &image.id, volume_key, phase);
        payload.message = Some(message);
        report(app, tasks, payload);
    };
    let (case_dir, staging_dir, published) = {
        let guard = active_case.lock().await;
        let Some(case) = guard.as_ref().filter(|case| case.has_evidence(&image.id)) else {
            return;
        };
        let published = case
            .volume(&image.id, volume_key)
            .map(|record| PublishedVolume {
                snapshot_path: case.snapshot_path(record),
                index_path: case.index_path(record),
                record: record.clone(),
            });
        (case.dir.clone(), case.staging_dir(), published)
    };

    let resource_permit = match tasks.acquire_resources(&task).await {
        Ok(permit) => permit,
        Err(err) => {
            fail("cancelled", err.message());
            return;
        }
    };
    let cancel = task.token();

    let current_size = volume_size_for_key(&image, volume_key);
    let fingerprint = image.evidence_fingerprint();
    // volId = position in the image's filesystem array (fs_all).
    let vol_id = {
        let image = Arc::clone(&image);
        let volume_key = volume_key.to_string();
        tokio::task::spawn_blocking(move || fs_all::vol_id_for_key(&image, &volume_key)).await
    };
    let vol_id = match vol_id {
        Ok(Ok(vol_id)) => vol_id,
        Ok(Err(err)) => {
            fail("error", format!("resolve volume id: {err:#}"));
            return;
        }
        Err(join_err) => {
            fail("error", format!("resolve volume id crashed: {join_err}"));
            return;
        }
    };

    // Reuse the published snapshot of the same content instead of re-walking.
    if let Some(published) = published.filter(|published| {
        let record = &published.record;
        !force_rebuild
            && record.source_size == current_size
            && record.fingerprint == fingerprint
            && record.vol_id == vol_id
            && published.snapshot_path.is_file()
    }) {
        let record = published.record;
        if published.index_path.is_some_and(|index| index.is_file()) {
            task.update(|view| {
                view.flags.insert(TaskFlag::Reused);
            });
            let mut payload = SnapshotProgressPayload::new(token_id, &image.id, volume_key, "done");
            payload.entries = record.entry_count;
            payload.truncated = record.truncated;
            payload.message = Some("已复用现有索引".into());
            report(app, tasks, payload);
            let mut reused = SnapshotProgressPayload::new("", &image.id, volume_key, "reused");
            reused.entries = record.entry_count;
            emit_progress(app, reused);
            return;
        }
        // The snapshot is still good but its index is missing: import again.
        index_published_snapshot(
            app,
            active_case,
            tasks,
            IndexJob {
                task_id: token_id.to_string(),
                case_dir,
                image_id: image.id.clone(),
                volume_key: volume_key.to_string(),
                snapshot_path: published.snapshot_path,
                staging_dir,
                built_at: record.built_at,
                entry_count: record.entry_count,
                truncated: record.truncated,
                vol_id,
                fingerprint,
                cancel,
            },
            resource_permit,
        )
        .await;
        return;
    }

    report(
        app,
        tasks,
        SnapshotProgressPayload::new(token_id, &image.id, volume_key, "started"),
    );
    let staged = staging_dir.join(format!(
        "{}-{}.dirz",
        case_paths::volume_segment(volume_key),
        case_paths::new_token()
    ));
    let build = {
        let app = app.clone();
        let tasks = tasks.clone();
        let token_id = token_id.to_string();
        let image_id = image.id.clone();
        let volume_key = volume_key.to_string();
        let image = Arc::clone(&image);
        let staged = staged.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let mut last = Instant::now()
                .checked_sub(PROGRESS_THROTTLE)
                .unwrap_or_else(Instant::now);
            let mut on_progress = |entries: u64| {
                if last.elapsed() >= PROGRESS_THROTTLE {
                    last = Instant::now();
                    let mut payload =
                        SnapshotProgressPayload::new(&token_id, &image_id, &volume_key, "walking");
                    payload.entries = entries;
                    report(&app, &tasks, payload);
                }
            };
            volume_snapshot::build_volume_snapshot(
                &image,
                &volume_key,
                &staged,
                DEFAULT_MAX_SNAPSHOT_ENTRIES,
                case_ledger::now(),
                &cancel,
                &mut on_progress,
            )
        })
        .await
    };
    let built = match build {
        Ok(Ok(built)) => built,
        Ok(Err(err)) => {
            remove_staged_file(&staged);
            let phase = if cancel.is_cancelled() {
                "cancelled"
            } else {
                "error"
            };
            fail(phase, err.to_string());
            return;
        }
        Err(join_err) => {
            remove_staged_file(&staged);
            fail("error", format!("snapshot task crashed: {join_err}"));
            return;
        }
    };

    // Publish the walk before importing: the expensive result is kept even
    // when the import fails or the case closes meanwhile.
    let publication = case::snapshot_publication(vol_id, &built, &fingerprint);
    let record = match publish_staged_snapshot(
        active_case,
        &case_dir,
        &staged,
        publication,
        &cancel,
    )
    .await
    {
        Ok(Some(record)) => record,
        Ok(None) => {
            let message = if cancel.is_cancelled() {
                "快照构建已取消"
            } else {
                "案件或检材已切换"
            };
            fail("cancelled", message.to_string());
            return;
        }
        Err(err) => {
            fail("error", format!("保存快照失败：{err:#}"));
            return;
        }
    };
    let snapshot_path =
        case_paths::volume_dir(&case_dir, &image.id, volume_key).join(&record.snapshot_file);
    index_published_snapshot(
        app,
        active_case,
        tasks,
        IndexJob {
            task_id: token_id.to_string(),
            case_dir,
            image_id: image.id.clone(),
            volume_key: volume_key.to_string(),
            snapshot_path,
            staging_dir,
            built_at: record.built_at,
            entry_count: record.entry_count,
            truncated: record.truncated,
            vol_id,
            fingerprint,
            cancel,
        },
        resource_permit,
    )
    .await;
}

/// Publishes a staged snapshot into the active case on a blocking thread.
/// `Ok(None)` when the case or the evidence changed, the build was cancelled,
/// or the case already holds a newer snapshot; the staged file is removed then.
pub(super) async fn publish_staged_snapshot(
    active_case: &Arc<Mutex<Option<CaseState>>>,
    case_dir: &Path,
    staged: &Path,
    publication: SnapshotPublication,
    cancel: &CancellationToken,
) -> anyhow::Result<Option<VolumeRecord>> {
    let active_case = Arc::clone(active_case);
    let case_dir = case_dir.to_path_buf();
    let staged = staged.to_path_buf();
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        let mut guard = active_case.blocking_lock();
        let Some(case) = guard.as_mut().filter(|case| {
            case.dir == case_dir
                && case.has_evidence(&publication.image_id)
                && !cancel.is_cancelled()
        }) else {
            remove_staged_file(&staged);
            return Ok(None);
        };
        match case.publish_snapshot(&staged, publication)? {
            SnapshotPublish::Published(record) => Ok(Some(record)),
            SnapshotPublish::Discarded => Ok(None),
        }
    })
    .await
    .map_err(|join_err| anyhow::anyhow!("publish snapshot task crashed: {join_err}"))?
}

pub(crate) fn remove_staged_file(path: &Path) {
    if let Err(err) = case_paths::remove_database_files(path) {
        tracing::warn!("case: remove staged file {} failed: {err}", path.display());
    }
}
