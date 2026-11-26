//! Encrypted volumes: inspect their metadata, find BitLocker keys in memory,
//! and unlock BitLocker, LUKS, VeraCrypt or FileVault through one command into
//! the source partition or a separate session image. Source-partition unlock
//! also starts the existing snapshot/index workflow, whose
//! `snapshot-build-progress` events report its independent completion.

use crate::core::common::ids::ImageId;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::volumes::apfs::{self, ApfsEncryptionVolume};
use crate::host::{AppHandle, State};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use crate::core::evidence::crypto::bde_image::{BdeMetadataInfo, BdeVolume, VolumeByteReader};
use crate::core::evidence::crypto::bde_vmk::{
    self, BitlockerScanProgress, VmkCandidate, VmkScanCandidate, VmkScanResult,
};
use crate::core::evidence::crypto::encrypted_image::{
    self, EncryptionScheme, UnlockCredential, UnlockOptions, UnlockResult, UnlockTarget,
};
use crate::core::evidence::image::PartitionKind;
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskLabel, TaskProgressUnit, TaskRegistration};
use {
    crate::app::access::loaded_image_app_error, crate::app::access::reject_if_folder,
    crate::app::error::AppError,
};

/// List the APFS container's volume slots and their current session unlock state.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn apfs_inspect(
    state: State<'_, AppState>,
    image_id: ImageId,
    partition_id: u8,
) -> Result<Vec<ApfsEncryptionVolume>, AppError> {
    let source = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&source)?;
    tokio::task::spawn_blocking(move || apfs::encryption_inventory(&source, partition_id))
        .await
        .map_err(|error| AppError::task_panicked("APFS encryption inspection", error))?
        .map_err(AppError::from)
}

/// Inspect locked CoreStorage metadata and run the upstream forensic auditor.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn filevault_inspect(
    state: State<'_, AppState>,
    image_id: ImageId,
    options: crate::core::evidence::crypto::filevault::InspectOptions,
    task_id: Option<String>,
) -> Result<crate::core::evidence::crypto::filevault::Inventory, AppError> {
    let source = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&source)?;
    let task = state
        .tasks
        .register_if_absent(
            task_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            TaskKind::FilevaultInspect,
            Some(image_id.clone()),
        )
        .ok_or_else(|| AppError::invalid_input("task id is already in use"))?;
    let permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();
    let source_for_inspection = Arc::clone(&source);
    let token_for_inspection = token.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _task = task;
        crate::core::evidence::crypto::filevault::inspect(
            &source_for_inspection,
            options,
            token_for_inspection,
        )
    })
    .await
    .map_err(|error| AppError::task_panicked("FileVault inspection", error))?;
    if token.is_cancelled() {
        return Err(AppError::cancelled("FileVault 元数据读取已取消"));
    }
    if !state
        .images
        .lock()
        .await
        .get(&image_id)
        .is_some_and(|current| Arc::ptr_eq(current, &source))
    {
        return Err(AppError::source_changed(
            "FileVault evidence unloaded during inspection",
        ));
    }
    result.map_err(AppError::from)
}

/// Inspect a BitLocker partition's format, protector kinds and volume
/// identity without credentials.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bitlocker_inspect(
    state: State<'_, AppState>,
    image_id: ImageId,
    partition_id: u8,
) -> Result<BdeMetadataInfo, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&image)?;
    tokio::task::spawn_blocking(move || {
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.id == partition_id)
            .ok_or_else(|| AppError::not_found("partition not found"))?;
        if partition.kind != PartitionKind::Bitlocker {
            return Err(AppError::invalid_input("partition is not BitLocker"));
        }
        let offset = partition
            .start_lba
            .checked_mul(image.sector_size as u64)
            .ok_or_else(|| AppError::invalid_input("partition offset overflow"))?;
        let length = partition
            .length_lba
            .checked_mul(image.sector_size as u64)
            .ok_or_else(|| AppError::invalid_input("partition size overflow"))?;
        let source: Arc<dyn VolumeByteReader> = image.disk_image().clone();
        BdeVolume::read_metadata_io(Arc::downgrade(&source), offset, length).map_err(AppError::from)
    })
    .await
    .map_err(|error| AppError::task_panicked("bitlocker_inspect", error))?
}

/// Scan a memory image for BitLocker VMK candidates, each with the recovery
/// password it derives against the partition; an unlock with a candidate's
/// VMK goes through `encrypted_volume_unlock`.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bitlocker_scan_memory(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: ImageId,
    partition_id: u8,
    memory_path: String,
    task_id: String,
) -> Result<VmkScanResult, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&image)?;
    let memory_path = PathBuf::from(memory_path);

    // Register a cancellation token so `task_cancel(taskId)` drains the
    // rayon scan within seconds.
    let task = state.tasks.register(
        task_id.clone(),
        TaskKind::BitlockerScan,
        Some(image_id.clone()),
    );
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();

    let task_id_inner = task_id.clone();
    let app_inner = app.clone();
    let tasks_inner = state.tasks.clone();

    tokio::task::spawn_blocking(move || -> Result<VmkScanResult, AppError> {
        let _resource_permit = resource_permit;
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let scan_done = Arc::new(AtomicBool::new(false));

        // Bridge tokio CancellationToken → AtomicBool so `bde_vmk` stays
        // tokio-agnostic. The watcher exits cleanly when the scan signals
        // done, regardless of whether cancellation fired.
        let cancel_for_poll = Arc::clone(&cancel_flag);
        let scan_done_for_poll = Arc::clone(&scan_done);
        let watcher = std::thread::spawn(move || {
            while !scan_done_for_poll.load(Ordering::Relaxed) {
                if token.is_cancelled() {
                    cancel_for_poll.store(true, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });

        let progress = |done: u64, total: u64| {
            if let Err(error) = app_inner.emit(
                "bitlocker-scan-progress",
                BitlockerScanProgress {
                    task_id: task_id_inner.clone(),
                    bytes_done: done,
                    bytes_total: total,
                },
            ) {
                tracing::debug!("emit bitlocker-scan-progress failed: {error}");
            }
            tasks_inner.emit_progress(
                &task_id_inner,
                TaskProgressUnit::Bytes,
                done,
                Some(total),
                None,
            );
        };

        let raw_outcome =
            bde_vmk::scan_memory_file_with_progress(&memory_path, &progress, cancel_flag.as_ref());

        scan_done.store(true, Ordering::Relaxed);
        if watcher.join().is_err() {
            tracing::warn!("BitLocker memory-scan cancellation watcher panicked");
        }

        let raw_candidates = raw_outcome.map_err(AppError::from)?;

        let partition = image
            .partitions
            .iter()
            .find(|p| p.id == partition_id)
            .cloned()
            .ok_or_else(|| AppError::not_found(format!("partition not found: {partition_id}")))?;
        let mut candidates = Vec::with_capacity(raw_candidates.len());
        for VmkCandidate {
            offset,
            type_flag,
            key_type,
            vmk_hex,
        } in raw_candidates
        {
            let recovery_key = match ::hex::decode(&vmk_hex) {
                Ok(vmk) => bde_vmk::recovery_key_from_vmk(&image, &partition, &vmk)
                    .ok()
                    .flatten(),
                Err(_) => None,
            };
            candidates.push(VmkScanCandidate {
                offset,
                type_flag,
                key_type,
                vmk_hex,
                recovery_key,
                already_tried: false,
            });
        }
        Ok(VmkScanResult {
            partition_id,
            candidates,
        })
    })
    .await
    .map_err(|err| AppError::task_panicked("bitlocker_scan_memory", err))?
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn encrypted_volume_unlock(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: ImageId,
    options: UnlockOptions,
    task_id: Option<String>,
) -> Result<UnlockResult, AppError> {
    let source = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&source)?;
    let target = options.target;
    let partition_id = options.partition_id;
    if let Some(id) = partition_id {
        if target == UnlockTarget::SourcePartition
            && source
                .info()
                .unlocked_partitions
                .iter()
                .any(|unlocked| unlocked.id == id)
        {
            return Err(AppError::invalid_input("分区已解锁"));
        }
    }
    let task_id = task_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let progress_task_id = task_id.clone();
    let task = state
        .tasks
        .register_if_absent(
            task_id,
            TaskKind::EncryptedVolumeUnlock,
            Some(image_id.clone()),
        )
        .ok_or_else(|| AppError::invalid_input("task id is already in use"))?;
    task.update(|view| {
        view.labels
            .insert(TaskLabel::Scheme, options.scheme.as_str().to_owned());
        // The volume, then its label (none: the whole image or container).
        if let Some(id) = partition_id {
            view.subject.push(format!("part:{id}"));
            let label = source
                .partitions
                .iter()
                .find(|partition| partition.id == id)
                .and_then(|partition| partition.label.clone())
                .filter(|label| !label.is_empty());
            view.subject.extend(label);
        }
    });
    let unlocked = async {
        if options.scheme == EncryptionScheme::Apfs {
            return unlock_apfs(&app, &state, source, options, &task).await;
        }
        let permit = state.tasks.acquire_resources(&task).await?;
        let token = task.token();
        let source_for_unlock = Arc::clone(&source);
        let token_for_unlock = token.clone();
        let tasks = state.tasks.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            // VeraCrypt reports its header search in key derivation jobs.
            let progress = |done: u64, total: u64| {
                tasks.emit_progress(
                    &progress_task_id,
                    TaskProgressUnit::Attempts,
                    done,
                    Some(total),
                    None,
                );
            };
            encrypted_image::unlock_with_progress(
                &source_for_unlock,
                options,
                token_for_unlock,
                &progress,
            )
        })
        .await
        .map_err(|error| AppError::task_panicked("encrypted volume unlock", error))?;
        if token.is_cancelled() {
            return Err(AppError::cancelled("encrypted volume unlock cancelled"));
        }
        let (image, mut result) = result.map_err(AppError::from)?;
        let mut images = state.images.lock().await;
        if token.is_cancelled() {
            return Err(AppError::cancelled("encrypted volume unlock cancelled"));
        }
        if !images
            .get(&image_id)
            .is_some_and(|current| Arc::ptr_eq(current, &source))
        {
            return Err(AppError::source_changed(
                "encrypted evidence was unloaded during unlock",
            ));
        }
        if target == UnlockTarget::SourcePartition {
            let partition_id = partition_id
                .or_else(|| {
                    source
                        .partitions
                        .iter()
                        .find(|partition| {
                            partition.start_lba * source.sector_size as u64 == result.source_offset
                                && partition.length_lba * source.sector_size as u64
                                    == result.source_length
                        })
                        .map(|partition| partition.id)
                })
                .or_else(|| {
                    (source.partitions.is_empty()
                        && result.source_offset == 0
                        && result.source_length == source.len())
                    .then_some(0)
                })
                .ok_or_else(|| AppError::invalid_input("请选择源镜像中的完整加密分区"))?;
            source
                .attach_decrypted_partition(
                    partition_id,
                    image,
                    result.scheme,
                    result.read_cancellation.clone(),
                )
                .map_err(AppError::from)?;
            result.image = source.info();
            result.volume_key = Some(format!("part:{partition_id}"));
            drop(images);
            state.flat_walk_cache.lock().purge_image(&image_id);
            crate::app::workflows::snapshot::case_snapshot::snapshot_unlocked_volume(
                &app,
                &state,
                Arc::clone(&source),
                format!("part:{partition_id}").into(),
            )
            .await;
        } else {
            images.insert(
                image.id.clone(),
                Arc::new(crate::core::filesystem::evidence_session::EvidenceSession::from(image)),
            );
        }
        Ok(result)
    }
    .await;
    if let Ok(unlocked) = &unlocked {
        task.update(|view| {
            view.labels
                .insert(TaskLabel::Cipher, unlocked.cipher.clone());
        });
    }
    task.finish_with(&unlocked);
    unlocked
}

async fn unlock_apfs(
    app: &AppHandle,
    state: &State<'_, AppState>,
    source: Arc<EvidenceSession>,
    options: UnlockOptions,
    task: &TaskRegistration,
) -> Result<UnlockResult, AppError> {
    if options.target != UnlockTarget::SourcePartition
        || options.offset.is_some()
        || options.length.is_some()
        || options.encrypted_root_plist_path.is_some()
        || options.header != encrypted_image::VeracryptHeader::Auto
        || options.pim != 0
        || options.prf.is_some()
        || !options.keyfiles.is_empty()
    {
        return Err(AppError::invalid_input(
            "APFS requires a source partition and volume slot",
        ));
    }
    let partition_id = options
        .partition_id
        .ok_or_else(|| AppError::invalid_input("请选择 APFS 容器分区"))?;
    if !matches!(options.credential, UnlockCredential::Password { .. }) {
        return Err(AppError::invalid_input(
            "APFS requires a password or personal recovery key",
        ));
    }
    let permit = state.tasks.acquire_resources(task).await?;
    let token = task.token();
    let source_for_unlock = Arc::clone(&source);
    let token_for_unlock = token.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let UnlockCredential::Password { password } = &options.credential else {
            unreachable!()
        };
        apfs::prepare_unlock(
            &source_for_unlock,
            partition_id,
            options.logical_volume_index,
            password.as_bytes(),
            &token_for_unlock,
        )
    })
    .await
    .map_err(|error| AppError::task_panicked("APFS unlock", error))?
    .map_err(AppError::from)?;
    let images = state.images.lock().await;
    if token.is_cancelled() {
        return Err(AppError::cancelled("APFS volume unlock cancelled"));
    }
    if !images
        .get(&source.id)
        .is_some_and(|current| Arc::ptr_eq(current, &source))
    {
        return Err(AppError::source_changed(
            "APFS evidence was unloaded during unlock",
        ));
    }
    source.attach_apfs_key(
        prepared.container_offset,
        prepared.volume_index,
        prepared.key,
    );
    drop(images);
    state.flat_walk_cache.lock().purge_image(&source.id);
    let partition_key = format!("part:{}", prepared.partition_id);
    crate::app::workflows::snapshot::case_snapshot::snapshot_unlocked_volume(
        app,
        state,
        Arc::clone(&source),
        partition_key.into(),
    )
    .await;
    Ok(UnlockResult {
        read_cancellation: token,
        image: source.info(),
        scheme: EncryptionScheme::Apfs,
        cipher: "AES-128-XTS".into(),
        encryption_sector_size: 512,
        source_offset: prepared.source_offset,
        source_length: prepared.source_length,
        hidden: false,
        volume_key: Some(apfs::apfs_node_id(partition_id, prepared.volume_index, "")),
        logical_volume_identifier: Some(prepared.uuid.to_string()),
        logical_volume_index: Some(prepared.volume_index),
        recovery_password: None,
        veracrypt: None,
    })
}
