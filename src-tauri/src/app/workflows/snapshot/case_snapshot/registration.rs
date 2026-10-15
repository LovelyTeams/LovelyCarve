//! registration implementation.
use super::*;
use crate::core::filesystem::evidence_session::EvidenceSession;

/// Volume keys worth snapshotting on an image: the supported partition roots,
/// or the single synthetic `"root"` for docker / folder / tar / zip / 7z / AD1 evidence.
/// Locked BitLocker partitions are skipped (their listing is only a placeholder
/// node) and snapshot after they are unlocked instead.
pub(super) fn browsable_volume_keys(image: &Arc<EvidenceSession>) -> Vec<String> {
    if image.docker.is_some()
        || image.folder.is_some()
        || image.tar.is_some()
        || image.zip.is_some()
        || image.sevenz.is_some()
        || image.ad1.is_some()
    {
        return vec!["root".to_string()];
    }
    image
        .partitions
        .iter()
        .filter(|partition| snapshot_supported_partition(image, partition.kind, partition.id))
        .map(|partition| format!("part:{}", partition.id))
        .collect()
}

pub(super) fn snapshot_supported_partition(
    image: &EvidenceSession,
    kind: PartitionKind,
    partition_id: u8,
) -> bool {
    if let Some(plaintext) = image.decrypted_session_for_node(&format!("part:{partition_id}")) {
        return plaintext
            .partitions
            .iter()
            .any(|partition| snapshot_supported_plain_partition(partition.kind));
    }
    match kind {
        PartitionKind::Bitlocker => image.bitlocker_unlocked(partition_id).is_some(),
        other => snapshot_supported_plain_partition(other),
    }
}

/// Index just the newly unlocked native volume under its original evidence identity.
pub(crate) async fn snapshot_unlocked_volume(
    app: &AppHandle,
    state: &State<'_, AppState>,
    image: Arc<EvidenceSession>,
    volume_key: String,
) {
    let case = state.active_case.lock().await;
    let in_case = case
        .as_ref()
        .is_some_and(|case| case.has_evidence(&image.id));
    let caseless = case.is_none();
    drop(case);
    if in_case {
        let task_id = format!("snapshot:{}:{volume_key}", image.id);
        spawn_volume_snapshot(
            app.clone(),
            Arc::clone(&state.active_case),
            state.tasks.clone(),
            image,
            volume_key,
            task_id,
            true,
        );
    } else if caseless {
        let task_id = format!("snapshot-session:{}:{volume_key}", image.id);
        crate::app::workflows::snapshot::snapshot_session::spawn_session_snapshot(
            app.clone(),
            Arc::clone(&state.session_snapshots),
            Arc::clone(&state.images),
            state.tasks.clone(),
            Arc::clone(&state.active_case),
            image,
            volume_key,
            task_id,
        );
    }
}

pub(super) fn snapshot_supported_plain_partition(kind: PartitionKind) -> bool {
    matches!(
        kind,
        PartitionKind::Ext4
            | PartitionKind::Btrfs
            | PartitionKind::Fat32
            | PartitionKind::Fat12
            | PartitionKind::Fat16
            | PartitionKind::ExFat
            | PartitionKind::Cdfs
            | PartitionKind::Squashfs
            | PartitionKind::ApfsContainer
            | PartitionKind::HfsPlus
            | PartitionKind::HfsX
            | PartitionKind::Lvm
            | PartitionKind::Ntfs
            | PartitionKind::Refs
            | PartitionKind::Ufs
            | PartitionKind::Xfs
            | PartitionKind::Zfs
    )
}

/// Register `image` into the active case (if any) as evidence (ledger row +
/// evidence directory), then kick off a background snapshot build for every
/// browsable volume. When NO case is open it instead builds caseless
/// **session** snapshots (see `app::workflows::snapshot::snapshot_session`) so the directory
/// structure is ready in the background right after load — except for raw
/// physical disks, which stay lazy. Called from `image_load` and on
/// `case_open` / `case_create` for already-loaded images. An error means the
/// image must not stay loaded.
pub(crate) async fn register_and_snapshot(
    app: &AppHandle,
    state: &State<'_, AppState>,
    image: Arc<EvidenceSession>,
) -> Result<(), AppError> {
    if image.is_index_only() {
        let active = state.active_case.lock().await;
        return if active.as_ref().is_some_and(|case| {
            case.evidence_image_id_for_path(&image.path.to_string_lossy())
                .as_deref()
                == Some(image.id.as_str())
        }) {
            Ok(())
        } else {
            Err(AppError::not_found("离线索引所属案件已关闭或检材已移除"))
        };
    }
    let removed_before_registration = {
        let removed = state.removed_case_evidence.lock().await;
        removed.contains(&image.id)
    };
    let admission = {
        let mut guard = state.active_case.lock().await;
        match guard.as_mut() {
            None => Admission::Caseless,
            Some(case) => {
                retire_outdated_backup_views(case, &image)?;
                *image.host_docker_cache_dir.write() = Some(
                    crate::core::catalog::host_docker_index::cache_directory(&case.dir, &image.id),
                );
                if removed_before_registration && !case.has_evidence(&image.id) {
                    Admission::Removed
                } else {
                    admit(case, &image)
                }
            }
        }
    };
    match admission {
        // Keep the tombstone until the case changes. Several same-path
        // `image_load` callers can already be queued behind the single-flight
        // lock; consuming it here would let the next waiter reopen and
        // resurrect evidence the user just removed.
        Admission::Removed => Err(AppError::cancelled("镜像已从案件移除，跳过重新加载")),
        Admission::SourceTaken { existing_image_id } => Err(AppError::invalid_input(format!(
            "该来源已作为检材 {existing_image_id} 登记在当前案件中"
        ))),
        Admission::Caseless => {
            // No case open: build directory snapshots into the session store
            // so caseless browsing / 全显 / child-counts are served from an
            // index. Physical disks stay lazy — a whole-tree walk holds the
            // shared block mutex for minutes and would starve interactive
            // reads.
            if matches!(image.info().source_format, ImageFormat::PhysicalDisk) {
                return Ok(());
            }
            for volume_key in browsable_volume_keys(&image) {
                let token_id = format!("snapshot-session:{}:{}", image.id, volume_key);
                crate::app::workflows::snapshot::snapshot_session::spawn_session_snapshot(
                    app.clone(),
                    Arc::clone(&state.session_snapshots),
                    Arc::clone(&state.images),
                    state.tasks.clone(),
                    Arc::clone(&state.active_case),
                    Arc::clone(&image),
                    volume_key,
                    token_id,
                );
            }
            Ok(())
        }
        Admission::Admitted { first_online } => {
            if first_online {
                start_first_tasks(app, state, &image).await;
            }
            // A case now owns this image: cancel any caseless session build
            // still in flight for it and drop any session snapshot already
            // installed — the case volumes below supersede them.
            crate::app::workflows::snapshot::snapshot_session::purge_session_snapshots(
                &state.session_snapshots,
                &state.tasks,
                &image.id,
            )
            .await;
            for volume_key in browsable_volume_keys(&image) {
                let token_id = format!("snapshot:{}:{}", image.id, volume_key);
                spawn_volume_snapshot(
                    app.clone(),
                    Arc::clone(&state.active_case),
                    state.tasks.clone(),
                    Arc::clone(&image),
                    volume_key,
                    token_id,
                    false,
                );
            }
            Ok(())
        }
    }
}

/// Retire derived metadata before browsing, rather than serving file nodes from
/// an older reader while a new directory snapshot builds in the background.
pub(super) fn retire_outdated_backup_views(
    case: &mut CaseState,
    image: &EvidenceSession,
) -> anyhow::Result<()> {
    if !image.has_expanded_backups() {
        return Ok(());
    }
    let fingerprint = image.evidence_fingerprint();
    let stale: Vec<_> = case
        .volumes()
        .iter()
        .filter(|record| record.image_id == image.id && record.fingerprint != fingerprint)
        .map(|record| record.volume_key.clone())
        .collect();
    for volume_key in stale {
        tracing::info!(
            "retiring outdated backup projection image={} volume={volume_key}",
            image.id
        );
        case.drop_volume(&image.id, &volume_key)?;
    }
    Ok(())
}

/// Registers `image` as evidence of `case` unless it already is, unchanged.
pub(super) fn admit(case: &mut CaseState, image: &EvidenceSession) -> Admission {
    let info = image.info();
    let existing = case.evidence_ref(&image.id);
    let unchanged = existing.as_ref().is_some_and(|evidence| {
        evidence.path == info.path
            && evidence.source_format == info.source_format
            && evidence.size_bytes == info.size_bytes
    });
    if unchanged {
        return Admission::Admitted {
            first_online: false,
        };
    }
    let added_at = existing
        .map(|evidence| evidence.added_at)
        .unwrap_or_else(case_ledger::now);
    match case.register_evidence(&EvidenceRef {
        image_id: image.id.clone(),
        path: info.path,
        source_format: info.source_format,
        size_bytes: info.size_bytes,
        added_at,
        platform: None,
        evidence_type: None,
    }) {
        Ok(EvidenceRegistration::Added) => Admission::Admitted { first_online: true },
        Ok(EvidenceRegistration::Known) => Admission::Admitted {
            first_online: false,
        },
        Ok(EvidenceRegistration::SourceTaken { existing_image_id }) => {
            Admission::SourceTaken { existing_image_id }
        }
        Err(err) => {
            tracing::warn!("case: register evidence {} failed: {err:#}", image.id);
            Admission::Admitted {
                first_online: false,
            }
        }
    }
}

/// Evidence went online for the first time: run the `autodetectfilesystem`
/// first task (platform / backup type detection, written back to the
/// evidence row), then the `trigger=auto` dependency-chain roots. Only on
/// first registration — reopening a case does not rerun the chain (its
/// results are persisted).
pub(super) async fn start_first_tasks(
    app: &AppHandle,
    state: &State<'_, AppState>,
    image: &Arc<EvidenceSession>,
) {
    crate::app::workflows::identify::spawn_first_task(
        app.clone(),
        state.tasks.clone(),
        Arc::clone(&state.active_case),
        Arc::clone(image),
    );
    // Creating the first tasks writes the ledger: keep it off the runtime.
    let scheduler = state.scheduler.clone();
    let image_id = image.id.clone();
    match tokio::task::spawn_blocking(move || scheduler.on_evidence_online(&image_id)).await {
        Ok(Ok(outcomes)) if !outcomes.is_empty() => tracing::info!(
            "case: evidence {} online, {} auto tasks considered",
            image.id,
            outcomes.len()
        ),
        Ok(Ok(_)) => {}
        Ok(Err(err)) => tracing::warn!("case: evidence {} first tasks: {err}", image.id),
        Err(err) => tracing::warn!("case: evidence {} first tasks crashed: {err}", image.id),
    }
}

/// On case open/create, fold every already-loaded image into the case.
pub(crate) async fn snapshot_already_loaded_images(app: &AppHandle, state: &State<'_, AppState>) {
    let loaded: Vec<Arc<EvidenceSession>> = {
        let images = state.images.lock().await;
        images.values().cloned().collect()
    };
    for image in loaded {
        let image_id = image.id.clone();
        if let Err(err) = register_and_snapshot(app, state, image).await {
            tracing::warn!(
                "case: loaded image {image_id} not folded in: {}",
                err.message
            );
        }
    }
}
