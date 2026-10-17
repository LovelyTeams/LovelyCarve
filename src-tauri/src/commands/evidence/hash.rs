//! Source-image hash IPC. Streams the decoded image bytes through a
//! parallel pipeline so MD5 / SHA-1 / SHA-256 / SM3 are computed in a
//! single read pass with one hasher per CPU core — important for E01 /
//! VMDK / VHD where each `read_range` call pays for decoding. Progress
//! is emitted on `image-hash-progress`; the command honors `task_cancel`
//! between chunks.

use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::host::{AppHandle, State};
use md5::{Digest as _, Md5};
use serde::Serialize;
use sha1::Sha1;
use sha2::Sha256;
use sm3::Sm3;
use tokio_util::sync::CancellationToken;

use crate::core::common::hash::{HashAlgo, MultiHashOutput};
use crate::core::evidence::image::{ReadFaultPolicy, ReadFaultReport};
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskProgressUnit};
use {
    crate::app::access::image_read_fault_report,
    crate::app::access::loaded_image_app_error,
    crate::app::access::reject_if_folder,
    crate::app::error::{AppError, ErrorCode},
};

const HASH_CHUNK_BYTES: usize = 16 * 1024 * 1024;
/// Bounded channel capacity per hasher. With one slot in flight (plus the
/// chunk currently being hashed) the reader can stay ~one chunk ahead of
/// the slowest hasher, which is enough to overlap I/O with CPU without
/// letting memory balloon: peak in-flight bytes = 4 hashers × 2 chunks ×
/// 16 MiB ≈ 128 MiB.
const CHANNEL_DEPTH: usize = 2;
/// Throttle progress emits so a 100 GB image doesn't flood Tauri's event
/// bus with thousands of events. ~150 ms matches the BitLocker extract.
const PROGRESS_THROTTLE: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Serialize)]
pub struct ImageHashProgress {
    pub task_id: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageHashResult {
    #[serde(flatten)]
    pub hashes: MultiHashOutput,
    #[serde(flatten)]
    pub read_faults: ReadFaultReport,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn image_hash_compute(
    app: AppHandle,
    state: State<'_, AppState>,
    image_id: String,
    algorithms: Vec<HashAlgo>,
    task_id: String,
    read_fault_policy: Option<ReadFaultPolicy>,
) -> Result<ImageHashResult, AppError> {
    if algorithms.is_empty() {
        return Err(AppError::invalid_input(
            "must request at least one hash algorithm",
        ));
    }
    let image = loaded_image_app_error(&state, &image_id).await?;
    reject_if_folder(&image)?;
    let read_fault_policy = read_fault_policy.unwrap_or_default();

    let task = state
        .tasks
        .register(task_id.clone(), TaskKind::ImageHash, Some(image_id.clone()));
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();

    let task_id_for_call = task_id.clone();
    let app_for_call = app.clone();
    let image_for_call = Arc::clone(&image);
    let tasks_for_call = state.tasks.clone();

    let outcome = tokio::task::spawn_blocking(move || -> Result<MultiHashOutput, AppError> {
        let _resource_permit = resource_permit;

        let on_progress = |done: u64, total: u64| {
            let _ = app_for_call.emit(
                "image-hash-progress",
                ImageHashProgress {
                    task_id: task_id_for_call.clone(),
                    bytes_done: done,
                    bytes_total: total,
                },
            );
            tasks_for_call.emit_progress(
                &task_id_for_call,
                TaskProgressUnit::Bytes,
                done,
                Some(total),
                None,
            );
        };

        let result = (|| {
            image_for_call
                .validate_source_file_stable()
                .map_err(|err| AppError::source_changed(format!("源盘哈希前校验失败: {err}")))?;
            let result = run_parallel_hash(
                image_for_call.as_ref(),
                &algorithms,
                &on_progress,
                &token,
                read_fault_policy,
            );
            if result.is_ok() {
                image_for_call
                    .validate_source_file_stable()
                    .map_err(|err| {
                        AppError::source_changed(format!("源盘哈希后校验失败: {err}"))
                    })?;
            }
            result
        })();

        result
    })
    .await;
    let outcome = outcome.map_err(|err| AppError::task_panicked("image_hash_compute", err))?;
    let read_faults = image_read_fault_report(&state, &image_id, &image).await;
    let hashes = match outcome {
        Ok(hashes) => hashes,
        Err(error) => {
            if let Err(store_error) = read_faults {
                tracing::warn!(
                    "hash fault audit persistence failed after error: {}",
                    store_error.message
                );
            }
            return Err(error);
        }
    };
    Ok(ImageHashResult {
        hashes,
        read_faults: read_faults?,
    })
}

/// Reader-producer + N hasher-consumer pipeline. The reader pulls 16 MiB
/// chunks from the image and fans each `Arc<Vec<u8>>` out to every
/// selected algorithm's channel. The hasher threads run on the OS
/// scheduler, so on a 4-core box MD5 / SHA-1 / SHA-256 / SM3 share cores
/// with the EWF decoder instead of serialising behind it.
///
/// Cancellation: `cancel` is polled between chunks; on signal the reader
/// drops its senders, causing all hashers to exit cleanly via channel
/// close. The function then returns an error without finalising digests.
fn run_parallel_hash(
    image: &crate::core::filesystem::evidence_session::EvidenceSession,
    algorithms: &[HashAlgo],
    on_progress: &dyn Fn(u64, u64),
    cancel: &CancellationToken,
    read_fault_policy: ReadFaultPolicy,
) -> Result<MultiHashOutput, AppError> {
    let total = image.len();

    // First tick before any I/O so the UI swaps from "queued" to a real
    // progress bar immediately.
    on_progress(0, total);

    let want_md5 = algorithms.contains(&HashAlgo::Md5);
    let want_sha1 = algorithms.contains(&HashAlgo::Sha1);
    let want_sha256 = algorithms.contains(&HashAlgo::Sha256);
    let want_sm3 = algorithms.contains(&HashAlgo::Sm3);

    let mut output = MultiHashOutput::default();

    thread::scope(|scope| -> Result<(), AppError> {
        // One bounded channel per selected algorithm. Each hasher owns
        // its receiver; dropping the sender (in the reader) is the only
        // way each hasher exits, so we keep the senders inside Option<>
        // and `take()` them at the end.
        let mut md5_tx: Option<SyncSender<Arc<Vec<u8>>>> = None;
        let mut sha1_tx: Option<SyncSender<Arc<Vec<u8>>>> = None;
        let mut sha256_tx: Option<SyncSender<Arc<Vec<u8>>>> = None;
        let mut sm3_tx: Option<SyncSender<Arc<Vec<u8>>>> = None;

        let mut md5_handle = None;
        let mut sha1_handle = None;
        let mut sha256_handle = None;
        let mut sm3_handle = None;

        if want_md5 {
            let (tx, rx) = mpsc::sync_channel::<Arc<Vec<u8>>>(CHANNEL_DEPTH);
            md5_tx = Some(tx);
            md5_handle = Some(scope.spawn(move || {
                let mut h = Md5::new();
                while let Ok(chunk) = rx.recv() {
                    h.update(chunk.as_slice());
                }
                ::hex::encode(h.finalize())
            }));
        }
        if want_sha1 {
            let (tx, rx) = mpsc::sync_channel::<Arc<Vec<u8>>>(CHANNEL_DEPTH);
            sha1_tx = Some(tx);
            sha1_handle = Some(scope.spawn(move || {
                let mut h = Sha1::new();
                while let Ok(chunk) = rx.recv() {
                    h.update(chunk.as_slice());
                }
                ::hex::encode(h.finalize())
            }));
        }
        if want_sha256 {
            let (tx, rx) = mpsc::sync_channel::<Arc<Vec<u8>>>(CHANNEL_DEPTH);
            sha256_tx = Some(tx);
            sha256_handle = Some(scope.spawn(move || {
                let mut h = Sha256::new();
                while let Ok(chunk) = rx.recv() {
                    h.update(chunk.as_slice());
                }
                ::hex::encode(h.finalize())
            }));
        }
        if want_sm3 {
            let (tx, rx) = mpsc::sync_channel::<Arc<Vec<u8>>>(CHANNEL_DEPTH);
            sm3_tx = Some(tx);
            sm3_handle = Some(scope.spawn(move || {
                let mut h = Sm3::new();
                while let Ok(chunk) = rx.recv() {
                    h.update(chunk.as_slice());
                }
                ::hex::encode(h.finalize())
            }));
        }

        // Reader loop on the current thread.
        let mut offset = 0u64;
        let mut last_emit = Instant::now();
        let mut cancelled = false;
        while offset < total {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
            let want = (total - offset).min(HASH_CHUNK_BYTES as u64) as usize;
            let mut bytes = vec![0u8; want];
            let read_len = image
                .read_into(offset, &mut bytes)
                .map_err(|err| AppError::new(ErrorCode::Io, format!("read @ {offset}: {err:#}")))?;
            if read_len == 0 {
                break;
            }
            if read_fault_policy == ReadFaultPolicy::Strict
                && image.has_bad_region_in_range(offset, read_len as u64)
            {
                return Err(AppError::new(
                    ErrorCode::CorruptData,
                    format!("严格读取在偏移 {offset} 的数据块检测到合成零字节，哈希计算已终止。"),
                ));
            }
            bytes.truncate(read_len);
            let shared = Arc::new(bytes);
            // Fan-out to every active hasher. `send` blocks when the
            // channel is full (the hasher is behind), which is exactly
            // the back-pressure we want.
            if let Some(tx) = md5_tx.as_ref() {
                tx.send(Arc::clone(&shared))
                    .map_err(|_| AppError::new(ErrorCode::Internal, "md5 hasher channel closed"))?;
            }
            if let Some(tx) = sha1_tx.as_ref() {
                tx.send(Arc::clone(&shared)).map_err(|_| {
                    AppError::new(ErrorCode::Internal, "sha1 hasher channel closed")
                })?;
            }
            if let Some(tx) = sha256_tx.as_ref() {
                tx.send(Arc::clone(&shared)).map_err(|_| {
                    AppError::new(ErrorCode::Internal, "sha256 hasher channel closed")
                })?;
            }
            if let Some(tx) = sm3_tx.as_ref() {
                tx.send(Arc::clone(&shared))
                    .map_err(|_| AppError::new(ErrorCode::Internal, "sm3 hasher channel closed"))?;
            }
            drop(shared);
            offset += read_len as u64;
            if last_emit.elapsed() >= PROGRESS_THROTTLE {
                on_progress(offset, total);
                last_emit = Instant::now();
            }
            if read_len < want {
                break;
            }
        }

        // Closing the senders is what tells the hashers to finalise.
        // Always drop them before joining, even on the cancel path.
        drop(md5_tx.take());
        drop(sha1_tx.take());
        drop(sha256_tx.take());
        drop(sm3_tx.take());

        let md5_value = md5_handle.map(|h| h.join());
        let sha1_value = sha1_handle.map(|h| h.join());
        let sha256_value = sha256_handle.map(|h| h.join());
        let sm3_value = sm3_handle.map(|h| h.join());

        if cancelled {
            return Err(AppError::cancelled("源盘哈希计算已取消"));
        }
        // A short read (backend returned fewer bytes than the image length)
        // must never yield a "complete" digest: the hash would silently cover
        // only a prefix of the evidence.
        if offset != total {
            return Err(AppError::new(
                ErrorCode::Io,
                format!("源盘只读到 {offset} / {total} 字节（短读），哈希未覆盖完整镜像，已终止。"),
            ));
        }

        on_progress(offset, total);

        output.md5 = md5_value.transpose().map_err(|err| {
            AppError::new(
                ErrorCode::TaskPanicked,
                format!("md5 hasher panicked: {err:?}"),
            )
        })?;
        output.sha1 = sha1_value.transpose().map_err(|err| {
            AppError::new(
                ErrorCode::TaskPanicked,
                format!("sha1 hasher panicked: {err:?}"),
            )
        })?;
        output.sha256 = sha256_value.transpose().map_err(|err| {
            AppError::new(
                ErrorCode::TaskPanicked,
                format!("sha256 hasher panicked: {err:?}"),
            )
        })?;
        output.sm3 = sm3_value.transpose().map_err(|err| {
            AppError::new(
                ErrorCode::TaskPanicked,
                format!("sm3 hasher panicked: {err:?}"),
            )
        })?;
        Ok(())
    })?;

    Ok(output)
}
