//! BitLocker IPC: status / unlock (recovery-password & user-password modes),
//! VMK memory-dump scanning, and VMK-driven unlock. Plaintext credentials
//! never persist. There is no persistence layer, so recovered keys and
//! failed-attempt history are not retained — each unlock runs independently.

use std::{
    fs::File,
    io::{BufWriter, Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use crate::host::{AppHandle, State};

use crate::app::workflows::evidence::ImageExportProgress;
use crate::core::evidence::crypto::bde_image::BdeUnlockKey;
use crate::core::evidence::crypto::bde_vmk::{
    self, BitlockerScanProgress, BitlockerVmkUnlockResult, VmkCandidate, VmkScanCandidate,
    VmkScanResult,
};
use crate::core::evidence::crypto::bitlocker::{BitlockerUnlockProgress, UnlockObserver};
use crate::core::evidence::image::{BitlockerStatus, PartitionKind};
use crate::core::filesystem::tree::fs_tree;
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskProgressUnit};
use {
    crate::app::access::loaded_image_app_error,
    crate::app::access::reject_if_folder,
    crate::app::error::{AppError, ErrorCode},
};

const EXPORT_CHUNK_BYTES: usize = 16 * 1024 * 1024;
const EXPORT_PROGRESS_THROTTLE: Duration = Duration::from_millis(150);

/// Inspect format, protector kinds and volume identity without credentials.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bitlocker_metadata(
    state: State<'_, AppState>,
    image_id: String,
    partition_id: u8,
) -> Result<crate::core::evidence::crypto::bde_image::BdeMetadataInfo, AppError> {
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
        let source: Arc<dyn crate::core::evidence::crypto::bde_image::VolumeByteReader> =
            image.disk_image().clone();
        crate::core::evidence::crypto::bde_image::BdeVolume::read_metadata_io(
            Arc::downgrade(&source),
            offset,
            length,
        )
        .map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("bitlocker_metadata", error))?
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bitlocker_status(
    state: State<'_, AppState>,
    image_id: String,
    partition_id: u8,
) -> Result<BitlockerStatus, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&image)?;
    image
        .bitlocker_status(partition_id)
        .map_err(AppError::from_anyhow)
}

#[cfg_attr(feature = "desktop", tauri::command)]
#[allow(clippy::too_many_arguments)]
pub async fn bitlocker_unlock(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    partition_id: u8,
    recovery_password: Option<String>,
    password: Option<String>,
    startup_key_path: Option<String>,
    fvek_hex: Option<String>,
    tweak_hex: Option<String>,
    task_id: Option<String>,
) -> Result<BitlockerStatus, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&image)?;
    let unlock_image = Arc::clone(&image);
    let recovery_for_call = recovery_password.clone();
    let password_for_call = password.clone();
    let task_id_for_call = task_id.clone();
    let app_for_call = app.clone();
    let tasks_for_call = state.tasks.clone();

    // Register cancellation when a task ID is supplied. The unlock checks it
    // before and after key derivation; derivation itself is not interruptible.
    let task = task_id.as_deref().map(|id| {
        state
            .tasks
            .register(id, TaskKind::BitlockerUnlock, Some(image_id.clone()))
    });
    let resource_permit = match task.as_ref() {
        Some(task) => Some(state.tasks.acquire_resources(task).await?),
        None => None,
    };
    let cancel_token = task.as_ref().map(|task| task.token());

    let join_outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<BitlockerStatus> {
        let _resource_permit = resource_permit;
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let scan_done = Arc::new(AtomicBool::new(false));

        // Bridge tokio CancellationToken → AtomicBool the unlock can poll
        // without holding a tokio handle. Mirrors the pattern in
        // `bitlocker_scan_memory`.
        let watcher = cancel_token.as_ref().map(|tok| {
            let tok = tok.clone();
            let cancel_for_poll = Arc::clone(&cancel_flag);
            let done_for_poll = Arc::clone(&scan_done);
            std::thread::spawn(move || {
                while !done_for_poll.load(Ordering::Relaxed) {
                    if tok.is_cancelled() {
                        cancel_for_poll.store(true, Ordering::Relaxed);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
        });

        let progress = |done: u64, total: u64| {
            if let Some(tid) = task_id_for_call.as_deref() {
                let _ = app_for_call.emit(
                    "bitlocker-unlock-progress",
                    BitlockerUnlockProgress {
                        task_id: tid.to_string(),
                        bytes_done: done,
                        bytes_total: total,
                    },
                );
                tasks_for_call.emit_progress(tid, TaskProgressUnit::Bytes, done, Some(total), None);
            }
        };
        let observer = UnlockObserver {
            progress: &progress,
            cancel: cancel_flag.as_ref(),
        };

        let result = (|| {
            let count = [
                recovery_for_call.is_some(),
                password_for_call.is_some(),
                startup_key_path.is_some(),
                fvek_hex.is_some(),
            ]
            .into_iter()
            .filter(|present| *present)
            .count();
            anyhow::ensure!(count <= 1, "choose exactly one BitLocker credential kind");
            anyhow::ensure!(
                tweak_hex.is_none() || fvek_hex.is_some(),
                "TWEAK requires FVEK"
            );
            let mut startup_key = Vec::new();
            if let Some(path) = startup_key_path {
                File::open(path)?
                    .take(65537)
                    .read_to_end(&mut startup_key)?;
                anyhow::ensure!(startup_key.len() <= 65536, "BEK file exceeds 64 KiB");
            }
            let decode = |value: &str| -> anyhow::Result<Vec<u8>> {
                anyhow::ensure!(value.len() <= 256, "key hex input is too long");
                hex::decode(value.split_whitespace().collect::<String>())
                    .map_err(|_| anyhow::anyhow!("invalid key hex"))
            };
            let fvek = fvek_hex.as_deref().map(decode).transpose()?;
            let tweak = tweak_hex
                .as_deref()
                .map(decode)
                .transpose()?
                .unwrap_or_default();
            let key = if let Some(value) = recovery_for_call.as_deref() {
                BdeUnlockKey::RecoveryPassword(value)
            } else if let Some(value) = password_for_call.as_deref() {
                BdeUnlockKey::Password(value)
            } else if !startup_key.is_empty() {
                BdeUnlockKey::StartupKey(&startup_key)
            } else if let Some(fvek) = fvek.as_deref() {
                BdeUnlockKey::Keys {
                    fvek,
                    tweak: &tweak,
                }
            } else {
                anyhow::ensure!(count == 0, "empty BEK file");
                BdeUnlockKey::Clear
            };
            unlock_image.unlock_bitlocker_key_with_observer(partition_id, key, &observer)
        })();

        scan_done.store(true, Ordering::Relaxed);
        if let Some(handle) = watcher {
            let _ = handle.join();
        }
        result
    })
    .await
    .map_err(|err| AppError::task_panicked("bitlocker_unlock", err))?;

    let status = join_outcome.map_err(AppError::from_anyhow)?;
    // Now that the NTFS volume is reachable, kick off the deleted-records
    // scan in the background so the FsTree view sees them inline on first
    // expansion.
    fs_tree::prefetch_ntfs_deleted_snapshots(image);
    Ok(status)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bitlocker_scan_memory(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
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

    let scan_outcome = tokio::task::spawn_blocking(move || -> Result<VmkScanResult, AppError> {
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
            let _ = app_inner.emit(
                "bitlocker-scan-progress",
                BitlockerScanProgress {
                    task_id: task_id_inner.clone(),
                    bytes_done: done,
                    bytes_total: total,
                },
            );
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
        let _ = watcher.join();

        let raw_candidates = raw_outcome.map_err(AppError::from_anyhow)?;

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
            let vmk_bytes = ::hex::decode(&vmk_hex).unwrap_or_default();
            let recovery_key = if vmk_bytes.is_empty() {
                None
            } else {
                bde_vmk::recovery_key_from_vmk(&image, &partition, &vmk_bytes)
                    .ok()
                    .flatten()
            };
            candidates.push(VmkScanCandidate {
                offset,
                type_flag,
                key_type,
                vmk_hex,
                recovery_key,
                // No persistence layer: every candidate is presented fresh,
                // there is no failed-attempt history to mark against.
                already_tried: false,
            });
        }
        Ok(VmkScanResult {
            partition_id,
            candidates,
        })
    })
    .await
    .map_err(|err| AppError::task_panicked("bitlocker_scan_memory", err))?;

    scan_outcome
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bitlocker_unlock_with_vmk(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    partition_id: u8,
    vmk_hex: String,
    task_id: Option<String>,
) -> Result<BitlockerVmkUnlockResult, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&image)?;
    let vmk = bde_vmk::parse_vmk_hex(&vmk_hex)
        .map_err(|err| AppError::invalid_input(format!("invalid VMK hex: {err}")))?;
    let task_id_for_call = task_id.clone();
    let app_for_call = app.clone();
    let tasks_for_call = state.tasks.clone();

    let task = task_id.as_deref().map(|id| {
        state
            .tasks
            .register(id, TaskKind::BitlockerUnlock, Some(image_id.clone()))
    });
    let resource_permit = match task.as_ref() {
        Some(task) => Some(state.tasks.acquire_resources(task).await?),
        None => None,
    };
    let cancel_token = task.as_ref().map(|task| task.token());

    let join_outcome =
        tokio::task::spawn_blocking(move || -> anyhow::Result<(BitlockerStatus, String)> {
            let _resource_permit = resource_permit;
            let cancel_flag = Arc::new(AtomicBool::new(false));
            let scan_done = Arc::new(AtomicBool::new(false));

            let watcher = cancel_token.as_ref().map(|tok| {
                let tok = tok.clone();
                let cancel_for_poll = Arc::clone(&cancel_flag);
                let done_for_poll = Arc::clone(&scan_done);
                std::thread::spawn(move || {
                    while !done_for_poll.load(Ordering::Relaxed) {
                        if tok.is_cancelled() {
                            cancel_for_poll.store(true, Ordering::Relaxed);
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                })
            });

            let progress = |done: u64, total: u64| {
                if let Some(tid) = task_id_for_call.as_deref() {
                    let _ = app_for_call.emit(
                        "bitlocker-unlock-progress",
                        BitlockerUnlockProgress {
                            task_id: tid.to_string(),
                            bytes_done: done,
                            bytes_total: total,
                        },
                    );
                    tasks_for_call.emit_progress(
                        tid,
                        TaskProgressUnit::Bytes,
                        done,
                        Some(total),
                        None,
                    );
                }
            };
            let observer = UnlockObserver {
                progress: &progress,
                cancel: cancel_flag.as_ref(),
            };

            let result =
                image.unlock_bitlocker_with_vmk_with_observer(partition_id, &vmk, &observer);

            scan_done.store(true, Ordering::Relaxed);
            if let Some(handle) = watcher {
                let _ = handle.join();
            }
            result
        })
        .await
        .map_err(|err| AppError::task_panicked("bitlocker_unlock_with_vmk", err))?;

    let (status, recovery_key) = join_outcome.map_err(AppError::from_anyhow)?;
    Ok(BitlockerVmkUnlockResult {
        status,
        recovery_key,
    })
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn bitlocker_export_decrypted_image(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    partition_id: u8,
    destination: String,
    task_id: String,
) -> Result<u64, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&image)?;
    let task = state.tasks.register(
        task_id.clone(),
        TaskKind::BitlockerExport,
        Some(image_id.clone()),
    );
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();
    let task_id_for_worker = task_id.clone();
    let tasks_for_worker = state.tasks.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<u64, AppError> {
        let _resource_permit = resource_permit;
        image.validate_source_file_stable().map_err(|err| {
            AppError::source_changed(format!("BitLocker 镜像导出前校验失败: {err}"))
        })?;
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.id == partition_id)
            .cloned()
            .ok_or_else(|| AppError::not_found(format!("partition not found: {partition_id}")))?;
        if partition.kind != PartitionKind::Bitlocker {
            return Err(AppError::new(
                ErrorCode::UnsupportedFormat,
                format!("partition {partition_id} is not BitLocker"),
            ));
        }
        let unlocked = image.bitlocker_unlocked(partition_id).ok_or_else(|| {
            AppError::invalid_input(format!(
                "BitLocker partition {partition_id} is not unlocked"
            ))
        })?;

        let total = image.len();
        let partition_start = partition.start_lba.checked_mul(image.sector_size as u64)
            .ok_or_else(|| AppError::invalid_input("partition offset overflow"))?;
        let partition_len = partition.length_lba.checked_mul(image.sector_size as u64)
            .ok_or_else(|| AppError::invalid_input("partition size overflow"))?;
        let plaintext_len = partition_len.min(unlocked.size());
        let plaintext_end = partition_start.checked_add(plaintext_len)
            .filter(|end| *end <= total).ok_or_else(|| AppError::invalid_input("BitLocker plaintext range outside image"))?;

        let path = PathBuf::from(&destination);
        let file = File::options().write(true).create_new(true).open(&path)
            .map_err(|err| AppError::from_io(format!("create {}", path.display()), err))?;
        let mut writer = BufWriter::with_capacity(EXPORT_CHUNK_BYTES, file);
        let mut offset = 0u64;
        let mut buffer = vec![0u8; EXPORT_CHUNK_BYTES];
        let mut last_emit = Instant::now();

        let _ = app.emit(
            "image-export-progress",
            ImageExportProgress {
                task_id: task_id_for_worker.clone(),
                bytes_done: 0,
                bytes_total: total,
            },
        );
        tasks_for_worker.emit_progress(
            &task_id_for_worker,
            TaskProgressUnit::Bytes,
            0,
            Some(total),
            None,
        );

        while offset < total {
            if token.is_cancelled() {
                writer
                    .flush()
                    .map_err(|err| AppError::from_io("flush before cancel", err))?;
                return Err(AppError::cancelled("export cancelled"));
            }

            let want = (total - offset).min(EXPORT_CHUNK_BYTES as u64) as usize;
            let (read_result, faults) = image.collect_read_faults(|| image.read_into(offset, &mut buffer[..want]));
            if faults.read_faults_detected {
                return Err(AppError::new(ErrorCode::Io, format!("BitLocker export encountered {} synthesized source bytes; see image_bad_regions", faults.synthetic_bytes)));
            }
            let read_len = read_result
                .map_err(|err| AppError::new(ErrorCode::Io, format!("read {offset}: {err:#}")))?;
            if read_len == 0 {
                break;
            }

            let bytes = &mut buffer[..read_len];
            let chunk_end = offset + read_len as u64;
            let overlap_start = offset.max(partition_start);
            let overlap_end = chunk_end.min(plaintext_end);
            if overlap_start < overlap_end {
                let output_start = (overlap_start - offset) as usize;
                let output_end = (overlap_end - offset) as usize;
                fill_unlocked_plaintext(
                    &unlocked,
                    overlap_start - partition_start,
                    &mut bytes[output_start..output_end],
                )?;
            }

            writer
                .write_all(bytes)
                .map_err(|err| AppError::from_io(format!("write {offset}"), err))?;
            offset += read_len as u64;
            let reached_end = read_len < want || offset >= total;
            if reached_end || last_emit.elapsed() >= EXPORT_PROGRESS_THROTTLE {
                let _ = app.emit(
                    "image-export-progress",
                    ImageExportProgress {
                        task_id: task_id_for_worker.clone(),
                        bytes_done: offset,
                        bytes_total: total,
                    },
                );
                tasks_for_worker.emit_progress(
                    &task_id_for_worker,
                    TaskProgressUnit::Bytes,
                    offset,
                    Some(total),
                    None,
                );
                last_emit = Instant::now();
            }
            if read_len < want {
                break;
            }
        }

        writer
            .flush()
            .map_err(|err| AppError::from_io("flush at end", err))?;
        if offset != total {
            return Err(AppError::new(
                ErrorCode::Io,
                format!("源盘只读到 {offset} / {total} 字节（短读），导出的解密镜像不完整。"),
            ));
        }
        image.validate_source_file_stable().map_err(|err| {
            AppError::source_changed(format!("BitLocker 镜像导出后校验失败: {err}"))
        })?;
        Ok(offset)
    })
    .await
    .map_err(|err| AppError::task_panicked("bitlocker_export_decrypted_image", err))?;

    result
}

fn fill_unlocked_plaintext(
    unlocked: &crate::core::evidence::crypto::bitlocker::UnlockedVolume,
    offset: u64,
    output: &mut [u8],
) -> Result<(), AppError> {
    let mut filled = 0usize;
    while filled < output.len() {
        let read = unlocked
            .read_at(offset + filled as u64, &mut output[filled..])
            .map_err(|err| {
                AppError::new(
                    ErrorCode::Io,
                    format!("read decrypted BitLocker bytes at {offset}: {err:#}"),
                )
            })?;
        if read == 0 {
            return Err(AppError::new(
                ErrorCode::CorruptData,
                format!(
                    "decrypted BitLocker stream ended early at {} of {} bytes",
                    filled,
                    output.len()
                ),
            ));
        }
        filled += read;
    }
    Ok(())
}
