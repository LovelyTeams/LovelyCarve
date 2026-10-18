//! export implementation.
use super::*;
use crate::core::filesystem::evidence_session::ImageExportMode;

/// Stream an entire image (regular or assembled) to a raw `.dd` file at
/// `destination`. Emits `image-export-progress` events with a
/// `{ task_id, bytes_done, bytes_total }` payload at every chunk.
///
/// Cancellation: the caller passes a `task_id` it has just registered via
/// the existing `task_cancel` IPC. The exporter checks that token between
/// chunks and aborts cleanly with a partial file (the partial result is
/// not deleted — leaving artefact recovery to the user).
pub async fn image_export_raw(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    destination: String,
    task_id: String,
    read_fault_policy: Option<ReadFaultPolicy>,
    mode: Option<ImageExportMode>,
) -> Result<ImageExportResult, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let worker_image = Arc::clone(&image);
    let read_fault_policy = read_fault_policy.unwrap_or_default();
    let task = state.tasks.register(
        task_id.clone(),
        TaskKind::ImageExport,
        Some(image_id.clone()),
    );
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();
    let task_id_for_worker = task_id.clone();
    let tasks_for_worker = state.tasks.clone();
    let result =
        tokio::task::spawn_blocking(move || -> Result<(u64, ReadFaultReport), AppError> {
            let _resource_permit = resource_permit;
            worker_image.validate_source_file_stable().map_err(|err| {
                AppError::source_changed(format!("RAW 镜像导出前校验失败：{err}"))
            })?;
            let total = worker_image.len();
            let reader = worker_image
                .raw_export_reader(mode.unwrap_or_default())
                .map_err(AppError::from_anyhow)?;
            let path = PathBuf::from(&destination);
            let file = File::options()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|err| AppError::from_io(format!("create {}", path.display()), err))?;
            let mut writer = BufWriter::with_capacity(EXPORT_CHUNK_BYTES, file);
            let mut offset = 0u64;
            let mut bad_regions = Vec::new();
            let mut buffer = vec![0u8; EXPORT_CHUNK_BYTES];
            let mut last_emit = Instant::now();
            // Always emit a starting tick so the frontend can switch from
            // "preparing" to "running" before the first chunk hits disk.
            if let Err(error) = app.emit(
                "image-export-progress",
                ImageExportProgress {
                    task_id: task_id_for_worker.clone(),
                    bytes_done: 0,
                    bytes_total: total,
                },
            ) {
                tracing::warn!("RAW export starting progress event failed: {error}");
            }
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
                let (read, faults) = worker_image
                    .collect_read_faults(|| reader.read_into(offset, &mut buffer[..want]));
                let detected_faults = faults.read_faults_detected;
                bad_regions.extend(faults.bad_regions);
                let read_len = read.map_err(|err| {
                    AppError::new(ErrorCode::Io, format!("read {offset}: {err:#}"))
                })?;
                if read_len == 0 {
                    break;
                }
                if read_fault_policy == ReadFaultPolicy::Strict && detected_faults {
                    return Err(AppError::new(
                        ErrorCode::CorruptData,
                        format!(
                        "strict read detected synthetic zero bytes in the block at offset {offset}"
                    ),
                    ));
                }
                writer
                    .write_all(&buffer[..read_len])
                    .map_err(|err| AppError::from_io(format!("write {offset}"), err))?;
                offset += read_len as u64;
                let reached_end = read_len < want || offset >= total;
                if reached_end || last_emit.elapsed() >= EXPORT_PROGRESS_THROTTLE {
                    if let Err(error) = app.emit(
                        "image-export-progress",
                        ImageExportProgress {
                            task_id: task_id_for_worker.clone(),
                            bytes_done: offset,
                            bytes_total: total,
                        },
                    ) {
                        tracing::warn!("RAW export progress event failed: {error}");
                    }
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
                    format!("源盘只读到 {offset} / {total} 字节（短读），导出的 RAW 镜像不完整。"),
                ));
            }
            worker_image.validate_source_file_stable().map_err(|err| {
                AppError::source_changed(format!("RAW 镜像导出后校验失败：{err}"))
            })?;
            Ok((offset, ReadFaultReport::from_regions(bad_regions)))
        })
        .await;
    let outcome = result.map_err(|err| AppError::task_panicked("image_export_raw", err))?;
    let read_faults = image_read_fault_report(&state, &image_id, &image).await;
    let (bytes_written, operation_faults) = match outcome {
        Ok(result) => result,
        Err(error) => {
            if let Err(store_error) = read_faults {
                tracing::warn!(
                    "export fault audit persistence failed after error: {}",
                    store_error.message
                );
            }
            return Err(error);
        }
    };
    read_faults?;
    Ok(ImageExportResult {
        bytes_written,
        read_faults: operation_faults,
    })
}
