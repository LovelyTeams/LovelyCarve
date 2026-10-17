//! Filesystem read and lightweight preview IPC: bounded file reads, raw/text
//! previews, media materialisation, preview cache, and file stat.

use crate::host::{LeaseOwner, State};

use crate::core::filesystem::tree::fs_tree::{self, FsStat};
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskProgressUnit};
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

/// Hard limit for a single raw IPC file read. This protects the Rust heap,
/// Tauri response buffer, and WebView ArrayBuffer even when a caller skips its
/// frontend size guard.
const FS_READ_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Read a whole, bounded file. Files larger than [`FS_READ_MAX_BYTES`] are
/// rejected instead of silently returning a prefix; callers must use ranged
/// reads, streaming export, or the media asset path for larger files.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_read(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<crate::host::Response, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, AppError> {
        let mut session = fs_tree::FsSession::new(&image);
        let stat = session.file_stat(&node_id).map_err(AppError::from_anyhow)?;
        if stat.size > FS_READ_MAX_BYTES {
            return Err(AppError::resource_limit(format!(
                "文件大小 {} 字节，超过单次读取上限 {} 字节；请使用分段读取或导出。",
                stat.size, FS_READ_MAX_BYTES
            )));
        }
        session
            .read_file_limited(&node_id, FS_READ_MAX_BYTES as usize)
            .map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_read", err))??;
    Ok(crate::host::Response::new(data))
}

/// Read a bounded logical file range. Offset and length are validated in u64
/// space before converting the allocation size to `usize`, so 32-bit targets
/// cannot truncate an untrusted IPC length.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_read_range(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    offset: u64,
    length: u64,
) -> Result<crate::host::Response, AppError> {
    if length > FS_READ_MAX_BYTES {
        return Err(AppError::resource_limit(format!(
            "分段读取长度 {length} 字节，超过单次读取上限 {FS_READ_MAX_BYTES} 字节。"
        )));
    }
    offset.checked_add(length).ok_or_else(|| {
        AppError::resource_limit(format!(
            "文件读取范围溢出：offset={offset}, length={length}"
        ))
    })?;
    let length = usize::try_from(length)
        .map_err(|_| AppError::resource_limit("当前平台无法表示该读取长度"))?;

    let image = loaded_image_app_error(&state, &image_id).await?;
    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, AppError> {
        fs_tree::read_file_range(&image, &node_id, offset, length).map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_read_range", err))??;
    Ok(crate::host::Response::new(data))
}

/// Rich previews use a bounded, cancellable memory read for small documents.
/// Larger stream-capable documents use the leased asset path.
const FS_READ_LARGE_MAX_BYTES: u64 = 16 * 1024 * 1024;

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_read_large(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    task_id: String,
) -> Result<crate::host::Response, AppError> {
    let task = state
        .tasks
        .register_if_absent(
            task_id,
            TaskKind::MediaMaterialization,
            Some(image_id.clone()),
        )
        .ok_or_else(|| AppError::invalid_input("重复的预览任务 ID"))?;
    let permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();
    let image = loaded_image_app_error(&state, &image_id).await?;
    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, AppError> {
        let _permit = permit;
        let mut session = fs_tree::FsSession::new(&image);
        let stat = session.file_stat(&node_id).map_err(AppError::from_anyhow)?;
        if stat.size > FS_READ_LARGE_MAX_BYTES {
            return Err(AppError::resource_limit(
                "文件超过 16 MiB 内存预览上限，请使用流式预览或导出",
            ));
        }
        if token.is_cancelled() {
            return Err(AppError::cancelled("预览已取消"));
        }
        let mut data = Vec::with_capacity(stat.size as usize);
        session
            .stream_file(&node_id, 1024 * 1024, |chunk| {
                anyhow::ensure!(!token.is_cancelled(), "预览已取消");
                anyhow::ensure!(
                    data.len().saturating_add(chunk.len()) <= FS_READ_LARGE_MAX_BYTES as usize,
                    "文件超过内存预览上限"
                );
                data.extend_from_slice(chunk);
                Ok(())
            })
            .map_err(AppError::from_anyhow)?;
        if token.is_cancelled() {
            return Err(AppError::cancelled("预览已取消"));
        }
        Ok(data)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_read_large", error))??;
    Ok(crate::host::Response::new(data))
}

/// Pick the temp-file extension the asset protocol uses to content-type a
/// materialised media file: the frontend's hint (derived from the effective /
/// identified type, so it sees through a renamed extension) sanitised to a bare
/// alphanumeric token, else a generic `bin`.
fn sanitize_media_ext(ext: Option<&str>) -> String {
    ext.map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "bin".to_string())
}

/// Materialise one leased preview. Requests for the same key serialize before
/// checking the cache; each caller owns an independent cancellation token, and
/// an owner that closes (a desktop window) abandons its lease. The CLI process
/// owns its leases until explicit release or exit.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_media_temp(
    owner: LeaseOwner,
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    ext: Option<String>,
    task_id: String,
) -> Result<String, AppError> {
    use std::io::Write;
    let window_closed = {
        let owner = owner.clone();
        move || owner.is_closed()
    };
    let owner = owner.label().to_owned();
    let task = state
        .tasks
        .register_if_absent(
            task_id.clone(),
            TaskKind::MediaMaterialization,
            Some(image_id.clone()),
        )
        .ok_or_else(|| AppError::invalid_input("重复的预览任务 ID"))?;
    let token = task.token();
    let image = loaded_image_app_error(&state, &image_id).await?;
    let ext = sanitize_media_ext(ext.as_deref());
    let key = format!("{image_id}\u{1f}{node_id}\u{1f}{ext}");
    let key_lock = state.media_temp.lock().await.key_lock(&key);
    let key_guard = tokio::select! {
        biased;
        _ = token.cancelled() => return Err(AppError::cancelled("预览已取消")),
        guard = key_lock.lock_owned() => guard,
    };
    if token.is_cancelled() || window_closed() {
        return Err(AppError::cancelled("预览已取消"));
    }
    if let Some(path) = state
        .media_temp
        .lock()
        .await
        .acquire(&key, &task_id, &owner)
    {
        return Ok(path.to_string_lossy().into_owned());
    }
    // One writer also bounds transient disk use and serialises byte admission.
    let writer = tokio::select! {
        biased;
        _ = token.cancelled() => return Err(AppError::cancelled("预览已取消")),
        guard = state.media_writer.clone().lock_owned() => guard,
    };
    let permit = state.tasks.acquire_resources(&task).await?;
    let scratch = state
        .media_scratch
        .lock()
        .clone()
        .ok_or_else(|| AppError::invalid_input("预览缓存尚未初始化"))?;
    let cache = state.media_temp.clone();
    let tasks = state.tasks.clone();
    tokio::task::spawn_blocking(move || -> Result<String, AppError> {
        let _key_guard = key_guard;
        let _writer = writer;
        let _permit = permit;
        if token.is_cancelled() {
            return Err(AppError::cancelled("预览已取消"));
        }
        let mut session = fs_tree::FsSession::new(&image);
        let stat = session.file_stat(&node_id).map_err(AppError::from_anyhow)?;
        cache
            .blocking_lock()
            .make_room(stat.size)
            .map_err(|error| AppError::resource_limit(error.to_string()))?;
        let mut file = tempfile::Builder::new()
            .suffix(&format!(".{ext}"))
            .tempfile_in(scratch.path())
            .map_err(|error| AppError::from_io("创建预览临时文件失败", error))?;
        let mut written = 0u64;
        let mut last_progress = std::time::Instant::now();
        session
            .stream_file(&node_id, 1024 * 1024, |chunk| {
                anyhow::ensure!(!token.is_cancelled(), "预览已取消");
                written = written.saturating_add(chunk.len() as u64);
                anyhow::ensure!(written <= stat.size, "预览源文件大小发生变化");
                file.write_all(chunk)?;
                if last_progress.elapsed() >= std::time::Duration::from_millis(150) {
                    tasks.emit_progress(
                        &task_id,
                        TaskProgressUnit::Bytes,
                        written,
                        Some(stat.size),
                        None,
                    );
                    last_progress = std::time::Instant::now();
                }
                Ok(())
            })
            .map_err(AppError::from_anyhow)?;
        if written != stat.size {
            return Err(AppError::source_changed("预览源文件未完整读取"));
        }
        file.flush()
            .map_err(|error| AppError::from_io("写入预览临时文件失败", error))?;
        let mut cache = cache.blocking_lock();
        if token.is_cancelled() {
            return Err(AppError::cancelled("预览已取消"));
        }
        if window_closed() {
            return Err(AppError::cancelled("预览窗口已关闭"));
        }
        let path = cache.insert(key, file.into_temp_path(), written, task_id, owner);
        Ok(path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_media_temp", error))?
}

/// Release only this viewer's lease; other windows continue using the file.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_media_release(state: State<'_, AppState>, task_id: String) -> Result<(), AppError> {
    state.media_temp.lock().await.release(&task_id);
    Ok(())
}

/// Read up to `max_bytes` (hard-capped at 2 MiB) of a file's bytes. Returns raw
/// binary via `crate::host::Response` (`ArrayBuffer` on the JS side) rather than a
/// JSON `Vec<u8>`, avoiding the ~4x integer-array bloat on every preview. Drives
/// the hex view; text views use `fs_preview_text` instead.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_preview(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    max_bytes: usize,
) -> Result<crate::host::Response, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, AppError> {
        let mut session = fs_tree::FsSession::new(&image);
        session
            .read_file_limited(&node_id, max_bytes.min(2 * 1024 * 1024))
            .map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_preview", error))??;
    Ok(crate::host::Response::new(data))
}

/// 检测文件文本的字符编码（chardetng）并解码为 UTF-8 回传，让 GBK/GB18030/Big5/
/// Shift-JIS/EUC-* 等非 UTF-8 文本在「原类型 / 文本视图」正常显示而非乱码。读取上限与
/// `fs_preview` 一致（由调用方 `max_bytes` 传入、后端再以 2 MiB 封顶；前端两者都传同一个
/// PREVIEW_CAP），故与十六进制视图所用的原始字节是同一段前缀，二者一致。十六进制视图仍用
/// `fs_preview` 的原始字节；文本类视图用本命令的解码结果。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_preview_text(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    max_bytes: usize,
) -> Result<crate::core::common::text_decode::DecodedText, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(
        move || -> Result<crate::core::common::text_decode::DecodedText, AppError> {
            let mut session = fs_tree::FsSession::new(&image);
            let data = session
                .read_file_limited(&node_id, max_bytes.min(2 * 1024 * 1024))
                .map_err(AppError::from_anyhow)?;
            Ok(crate::core::common::text_decode::decode(&data))
        },
    )
    .await
    .map_err(|err| AppError::task_panicked("fs_preview_text", err))?
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_stat(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<FsStat, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || -> Result<FsStat, AppError> {
        let mut session = fs_tree::FsSession::new(&image);
        session.file_stat(&node_id).map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_stat", error))?
}
