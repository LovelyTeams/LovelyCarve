//! Unlock encrypted evidence into its source partition or a separate session image.
//! Source-partition unlock also starts the existing snapshot/index workflow,
//! whose `snapshot-build-progress` events report its independent completion.

use crate::host::{AppHandle, State};
use std::sync::Arc;

use crate::core::evidence::crypto::encrypted_image::{
    self, EncryptionScheme, UnlockOptions, UnlockResult, UnlockTarget,
};
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskLabel};
use {
    crate::app::access::loaded_image_app_error, crate::app::access::reject_if_folder,
    crate::app::error::AppError,
};

/// Inspect locked CoreStorage metadata and run the upstream forensic auditor.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn filevault_inspect(
    state: State<'_, AppState>,
    image_id: String,
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
    result.map_err(AppError::from_anyhow)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn encrypted_volume_unlock(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    options: UnlockOptions,
    task_id: Option<String>,
) -> Result<UnlockResult, AppError> {
    let source = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&source)?;
    let target = options.target;
    let partition_id = options.partition_id;
    if let Some(id) = partition_id {
        if target == UnlockTarget::SourcePartition
            && source.info().unlocked_partitions.contains(&id)
        {
            return Err(AppError::invalid_input("分区已解锁"));
        }
    }
    let task_id = task_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let task = state
        .tasks
        .register_if_absent(
            task_id,
            TaskKind::EncryptedVolumeUnlock,
            Some(image_id.clone()),
        )
        .ok_or_else(|| AppError::invalid_input("task id is already in use"))?;
    task.update(|view| {
        view.labels.insert(
            TaskLabel::Scheme,
            match options.scheme {
                EncryptionScheme::Luks => "luks",
                EncryptionScheme::Veracrypt => "veracrypt",
                EncryptionScheme::Filevault => "filevault",
            }
            .to_owned(),
        );
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
        let permit = state.tasks.acquire_resources(&task).await?;
        let token = task.token();
        let source_for_unlock = Arc::clone(&source);
        let token_for_unlock = token.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            encrypted_image::unlock(&source_for_unlock, options, token_for_unlock)
        })
        .await
        .map_err(|error| AppError::task_panicked("encrypted volume unlock", error))?;
        if token.is_cancelled() {
            return Err(AppError::cancelled("encrypted volume unlock cancelled"));
        }
        let (image, mut result) = result.map_err(AppError::from_anyhow)?;
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
                .attach_decrypted_partition(partition_id, image, result.read_cancellation.clone())
                .map_err(AppError::from_anyhow)?;
            result.image = source.info();
            result.volume_key = Some(format!("part:{partition_id}"));
            drop(images);
            state.flat_walk_cache.lock().purge_image(&image_id);
            crate::app::workflows::snapshot::case_snapshot::snapshot_unlocked_volume(
                &app,
                &state,
                Arc::clone(&source),
                format!("part:{partition_id}"),
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
