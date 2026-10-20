//! load implementation.
use super::*;
use crate::core::evidence::image::EvidenceCache;
use crate::core::evidence::image::RawReadMode;
use crate::core::filesystem::evidence_session::EvidenceSession;

struct ImageOpenOptions {
    qcow_key: Option<qcow2::EncryptionKey>,
    raw_read_mode: RawReadMode,
}

/// Map an [`OpenProgress`] to a localized `image-load-progress` event, throttling
/// intermediate byte updates. Called from the `spawn_blocking` open thread.
fn emit_load_progress(
    app: &AppHandle,
    path: &str,
    progress: crate::core::evidence::image::OpenProgress,
    last_emit: &std::sync::Mutex<Option<std::time::Instant>>,
    tasks: &crate::task_manager::TaskManager,
    task_id: &str,
) {
    use crate::core::evidence::image::OpenProgress;
    use crate::task_manager::{TaskLabel, TaskStage};
    let (stage, current, total, is_bytes) = match &progress {
        OpenProgress::Opening => ("打开镜像".to_string(), 0u64, 0u64, false),
        OpenProgress::Decompressing { label, done, total } => {
            (format!("解压 {label}"), *done, *total, true)
        }
        OpenProgress::DetectingPartitions => ("检测分区".to_string(), 0, 0, false),
        OpenProgress::DetectingFilesystems { done, total } => (
            "检测文件系统".to_string(),
            *done as u64,
            *total as u64,
            false,
        ),
    };
    tasks.update(task_id, |view| {
        view.stage = Some(match &progress {
            OpenProgress::Opening => TaskStage::Opening,
            OpenProgress::Decompressing { .. } => TaskStage::Decompressing,
            OpenProgress::DetectingPartitions => TaskStage::DetectingPartitions,
            OpenProgress::DetectingFilesystems { .. } => TaskStage::DetectingFilesystems,
        });
        match &progress {
            OpenProgress::Decompressing { label, .. } => {
                view.labels.insert(TaskLabel::Item, label.clone());
            }
            OpenProgress::Opening
            | OpenProgress::DetectingPartitions
            | OpenProgress::DetectingFilesystems { .. } => {
                view.labels.remove(&TaskLabel::Item);
            }
        }
    });
    // Phase changes and the final byte update emit immediately; intermediate
    // byte updates are rate-limited.
    if is_bytes && current < total {
        let now = std::time::Instant::now();
        let Ok(mut last) = last_emit.lock() else {
            return;
        };
        if let Some(prev) = *last {
            if now.duration_since(prev) < LOAD_PROGRESS_THROTTLE {
                return;
            }
        }
        *last = Some(now);
    }
    tasks.emit_progress(
        task_id,
        if is_bytes {
            crate::task_manager::TaskProgressUnit::Bytes
        } else {
            crate::task_manager::TaskProgressUnit::Entries
        },
        current,
        (total > 0).then_some(total),
        Some(stage.clone()),
    );
    if let Err(error) = app.emit(
        "image-load-progress",
        ImageLoadProgress {
            path: path.to_string(),
            stage,
            current,
            total,
        },
    ) {
        tracing::warn!("image load progress delivery failed: {error}");
    }
}

pub async fn image_load(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    qcow_key: Option<qcow2::EncryptionKey>,
    task_id: Option<String>,
    case_dir: Option<String>,
    raw_read_mode: RawReadMode,
) -> Result<ImageInfo, AppError> {
    let reference_id = uuid::Uuid::parse_str(&path).ok().map(|id| id.to_string());
    let assembled_reference = path
        .split_once(':')
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("assembled"));
    let session_hint = if assembled_reference {
        Some("组装卷引用未在当前会话加载或已失效。assembled:// 不是主机文件路径；请在组装所在会话中运行 images list 获取 image ID，再用 context use <imageId> 或 images open <imageId> 选择。若原进程已退出，请先 sessions open <名称>，在同一会话加载成员镜像并运行 raid detect，使用返回的 assembledImageId。")
    } else if reference_id.is_some() {
        Some("image ID not loaded; open its source or load the member images and run RAID detection again")
    } else if path.starts_with("decrypted://") {
        Some("decrypted session expired; unlock the source again")
    } else if path.starts_with(crate::core::evidence::vss_image::SESSION_PREFIX) {
        Some("shadow copy session expired; open it again from the source image")
    } else {
        None
    };
    if let Some(hint) = session_hint {
        if raw_read_mode == RawReadMode::Force || qcow_key.is_some() {
            return Err(AppError::invalid_input(
                "RAW mode and QCOW credentials require a regular source path, not a session reference",
            ));
        }
        return state
            .images
            .lock()
            .await
            .values()
            .find(|image| {
                reference_id.as_deref() == Some(image.id.as_str())
                    || image_load_key(&image.path.to_string_lossy()) == image_load_key(&path)
            })
            .map(|image| image.info())
            .ok_or_else(|| {
                let error = AppError::not_found(hint);
                if assembled_reference { error.with_context_value(serde_json::json!({
                    "reference": path, "referenceType": "assembled", "requiresLiveSession": true,
                    "recovery": ["sessions attach <name>", "images list", "context use <imageId>", "raid detect"]
                })) } else { error }
            });
    }
    let context = state.image_load_context.lock().await;
    if let Some(expected) = case_dir {
        let active = state.active_case.lock().await;
        if !active.as_ref().is_some_and(|case| {
            image_load_key(&case.dir.to_string_lossy()) == image_load_key(&expected)
        }) {
            return Err(AppError::cancelled(
                "image load cancelled: originating case closed",
            ));
        }
    }
    let generation = *context;
    let task = state
        .tasks
        .register_if_absent(
            task_id.unwrap_or_else(|| format!("image-load:{}", uuid::Uuid::new_v4())),
            crate::task_manager::TaskKind::ImageLoad,
            None,
        )
        .ok_or_else(|| AppError::invalid_input("image load task id already in use"))?;
    drop(context);
    task.update(|view| view.subject = vec![path.clone()]);
    let token = task.token();
    // Resolve the id only after acquiring the per-path lock. Without this,
    // concurrent callers (notably case tab restoration + auto-mount) can both
    // observe a cold path, mint/open the same image independently, and race to
    // overwrite the runtime entry. The follower now sees the first opener's
    // registered image and takes the cheap reuse path below.
    let load_lock = image_load_lock(&state, &path).await;
    let _load_guard = tokio::select! {
        guard = load_lock.lock() => guard,
        () = token.cancelled() => return Err(AppError::cancelled("image load cancelled")),
    };

    // Reuse the id of an image already loaded from this path so repeated loads
    // converge on one runtime entry. Failing that, reuse the id this path was
    // recorded under in the active case's evidence — so re-loading a case's
    // evidence (or just re-opening the same file across sessions) keeps a
    // *stable* id. That stability is load-bearing: evidence/snapshot manifest
    // entries dedup by image_id, and volume snapshots match by (image_id,
    // volume_key), so a fresh uuid per load would both pile up duplicate
    // manifest entries and miss every persisted snapshot. Only when neither
    // knows the path do we mint a new id.
    let image_id = {
        let path_key = image_load_key(&path);
        let by_session = {
            let images = state.images.lock().await;
            images
                .values()
                .find(|img| image_load_key(&img.path.to_string_lossy()) == path_key)
                .map(|img| img.id.clone())
        };
        match by_session {
            Some(id) => id,
            None => {
                let case = state.active_case.lock().await;
                case.as_ref()
                    .and_then(|case| case.evidence_image_id_for_path(&path))
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
            }
        }
    };
    let loaded = load_image_runtime(
        &state,
        &app,
        image_id,
        path,
        ImageOpenOptions {
            qcow_key,
            raw_read_mode,
        },
        &task,
        generation,
    )
    .await;
    task.finish_with(&loaded);
    loaded
}

async fn image_load_lock(state: &AppState, path: &str) -> Arc<tokio::sync::Mutex<()>> {
    let key = image_load_key(path);
    let mut locks = state.image_load_locks.lock().await;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

fn image_load_key(path: &str) -> String {
    crate::core::casework::case::evidence_path_key(path)
}

async fn load_image_runtime(
    state: &State<'_, AppState>,
    app: &AppHandle,
    image_id: String,
    path: String,
    options: ImageOpenOptions,
    task: &crate::task_manager::TaskRegistration,
    generation: u64,
) -> Result<ImageInfo, AppError> {
    let ImageOpenOptions {
        qcow_key,
        raw_read_mode,
    } = options;
    let token = task.token();
    let context = state.image_load_context.lock().await;
    if token.is_cancelled() || *context != generation {
        return Err(AppError::cancelled("image load cancelled: case changed"));
    }
    if let Some(existing) = {
        let images = state.images.lock().await;
        images.get(&image_id).cloned()
    } {
        if !existing.is_index_only() && existing.disk_image().raw_read_mode() != raw_read_mode {
            return Err(AppError::invalid_input(
                "image already loaded with another RAW read mode; close it before reopening",
            ));
        }
        if existing.is_index_only() {
            // Retry the real opener: a restored source switches back online,
            // while a still-missing source revalidates the current case indexes.
            remove_image_runtime(state, &image_id).await?;
        } else {
            // Already loaded this session — still fold it into the active case
            // (idempotent: evidence registration, fresh snapshots are reused).
            let info = existing.info();
            if let Err(error) =
                crate::app::workflows::snapshot::case_snapshot::register_and_snapshot(
                    app, state, existing,
                )
                .await
            {
                remove_image_runtime(state, &info.id).await?;
                return Err(error);
            }
            return Ok(info);
        }
    }

    drop(context);
    // Listings derived from the source (such as the backup projection of a
    // ZIP) persist in the evidence's case cache, so reopening skips them.
    let evidence_cache = {
        let case = state.active_case.lock().await;
        case.as_ref()
            .map(|case| crate::core::casework::case_paths::cache_dir(&case.dir, &image_id))
    };
    let resource_permit = state.tasks.acquire_resources(task).await?;
    let progress_tasks = state.tasks.clone();
    let progress_task_id = task.record().id().to_owned();
    let worker_token = token.clone();
    // Partition detection on a real physical drive performs real disk I/O
    // (probing each partition's filesystem signature). Keep it off the
    // tokio worker thread so other IPCs stay responsive while it runs.
    let open_path = path.clone();
    let open_id = image_id.clone();
    let progress_app = app.clone();
    let progress_path = path.clone();
    let started = std::time::Instant::now();
    tracing::info!("image_load begin id={image_id} path={path}");
    let image = tokio::task::spawn_blocking(move || {
        let _resource_permit = resource_permit;
        let last_emit = std::sync::Mutex::new(None::<std::time::Instant>);
        let observer = move |progress: crate::core::evidence::image::OpenProgress| {
            emit_load_progress(
                &progress_app,
                &progress_path,
                progress,
                &last_emit,
                &progress_tasks,
                &progress_task_id,
            );
        };
        let cache = match evidence_cache.as_deref() {
            Some(directory) => EvidenceCache::Directory(directory),
            None => EvidenceCache::Disabled,
        };
        if raw_read_mode == RawReadMode::Force {
            return crate::core::evidence::image::DiskImage::open_forced_raw_cancellable(
                open_id,
                std::path::Path::new(&open_path),
                &observer,
                &worker_token,
            );
        }
        crate::core::evidence::image::DiskImage::open_cancellable(
            open_id,
            &open_path,
            &observer,
            qcow_key,
            cache,
            &worker_token,
        )
    })
    .await
    .map_err(|err| AppError::task_panicked("image_load", err))?;
    // Only publication shares this gate with case transitions; decoding never
    // holds application locks, so switching cases does not wait for extraction.
    let context = state.image_load_context.lock().await;
    if token.is_cancelled() || *context != generation {
        return Err(AppError::cancelled("image load cancelled: case changed"));
    }
    let image = match image {
        Ok(image) => image,
        Err(error) => {
            let missing = error.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            }) || std::fs::metadata(&path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
            if missing || path.starts_with("assembled://") {
                if let Some(indexed) =
                    super::indexed::open_indexed_evidence(state, &image_id, &path).await?
                {
                    return Ok(indexed.info());
                }
            }
            return Err(AppError::from(error));
        }
    };
    let info = image.info();
    tracing::info!(
        "image_load opened id={} format={:?} size={} partitions={} elapsed={:.3?}",
        info.id,
        info.source_format,
        info.size_bytes,
        info.partitions.len(),
        started.elapsed()
    );
    let arc_image = Arc::new(EvidenceSession::from(image));
    let mut images = state.images.lock().await;
    images.insert(info.id.clone(), Arc::clone(&arc_image));
    drop(images);
    // If a case is open, register this image as evidence and auto-build a
    // volume snapshot for each browsable volume (background, cancellable).
    // Caseless loads build the equivalent session snapshot. Both NTFS paths
    // scan the complete MFT (including deleted records), so do not also launch
    // `prefetch_ntfs_deleted_snapshots` here: that used to parse the entire MFT
    // a second time and competed with the snapshot for the same image cache.
    // Locked BitLocker volumes still start the dedicated prefetch after unlock,
    // when no automatic snapshot is scheduled by this load path.
    if let Err(error) = crate::app::workflows::snapshot::case_snapshot::register_and_snapshot(
        app,
        state,
        Arc::clone(&arc_image),
    )
    .await
    {
        remove_image_runtime(state, &info.id).await?;
        return Err(error);
    }
    Ok(info)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
