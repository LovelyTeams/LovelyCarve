//! Filesystem native-preview IPC for structured file formats rendered in the
//! original-type view.

use std::sync::Arc;

use crate::host::State;

use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree;
use crate::core::preview::realm_preview::{self, RealmDigest, RealmRows};
use crate::core::preview::sqlite::sqlite_preview::{self, SqliteDigest, SqliteKey, SqliteRows};
use crate::core::preview::sqlite::sqlite_recovery::{self, SqliteRecovery};
use crate::core::preview::vim_swap::{self, VimSwapDigest};
use crate::state::AppState;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_sqlite_preview(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    parent_id: Option<String>,
    key: Option<SqliteKey>,
) -> Result<SqliteDigest, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let cache = state.sqlite_session.clone();
    let key = key.unwrap_or_default();
    tokio::task::spawn_blocking(move || -> Result<SqliteDigest, AppError> {
        let session = sqlite_preview::cached_session(
            &mut cache.blocking_lock(),
            &image,
            &image_id,
            &node_id,
            parent_id.as_deref(),
        )
        .map_err(AppError::from)?;
        sqlite_preview::preview_session(&session, &key).map_err(AppError::from)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_sqlite_preview", err))?
}

/// Recover deleted records from a SQLite database (and its `-wal` or
/// `-journal`). `key` is the key the browser decrypted an encrypted database
/// with; the pages are decrypted again for carving.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_sqlite_deleted_records(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    parent_id: Option<String>,
    key: Option<SqliteKey>,
) -> Result<SqliteRecovery, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let key = key.unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        sqlite_recovery::recover(&image, &node_id, parent_id.as_deref(), &key)
            .map_err(AppError::from)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_sqlite_deleted_records", err))?
}

/// One page of a single SQLite table's rows, with optional column sort.
/// Reuses the preview VFS session cached by `fs_sqlite_preview` (or imports it
/// on first call). `order_by` is validated against the table's real columns
/// server-side before it touches SQL. `key` decrypts an encrypted (SQLCipher)
/// database.
#[cfg_attr(feature = "desktop", tauri::command)]
#[allow(clippy::too_many_arguments)]
pub async fn fs_sqlite_table_rows(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    parent_id: Option<String>,
    table: String,
    offset: i64,
    limit: i64,
    order_by: Option<String>,
    descending: bool,
    key: Option<SqliteKey>,
) -> Result<SqliteRows, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let cache = state.sqlite_session.clone();
    let key = key.unwrap_or_default();
    tokio::task::spawn_blocking(move || -> Result<SqliteRows, AppError> {
        let session = sqlite_preview::cached_session(
            &mut cache.blocking_lock(),
            &image,
            &image_id,
            &node_id,
            parent_id.as_deref(),
        )
        .map_err(AppError::from)?;
        sqlite_preview::table_rows_for_session(
            &session,
            &key,
            &table,
            offset,
            limit,
            order_by.as_deref(),
            descending,
        )
        .map_err(AppError::from)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_sqlite_table_rows", err))?
}

/// Realm 解析需要随机访问完整文件；不做前缀截断解析，避免把上限造成的缺页误判为
/// 真实库损坏。256 MiB 覆盖常见移动端 App Realm 库，同时避免原类型视图常驻过大内存。
const REALM_READ_CAP: usize = 256 * 1024 * 1024;

fn realm_bytes_cached(
    image: &Arc<EvidenceSession>,
    cache: &mut Option<crate::state::RealmBytesCache>,
    image_id: &str,
    node_id: &str,
) -> Result<(Arc<[u8]>, u64), AppError> {
    let key = format!("{image_id}\u{1f}{node_id}");
    if let Some(entry) = cache.as_ref() {
        if entry.key == key {
            return Ok((entry.bytes.clone(), entry.file_size));
        }
    }

    let mut session = fs_tree::FsSession::new(image);
    let stat = session.file_stat(node_id).map_err(AppError::from_anyhow)?;
    if stat.size > REALM_READ_CAP as u64 {
        return Err(AppError::resource_limit(format!(
            "Realm 文件大于 {} MiB 原类型预览上限；为避免误读半截数据库，请导出后用专用工具查看。",
            REALM_READ_CAP / (1024 * 1024)
        )));
    }

    let data = session
        .read_file_limited(node_id, REALM_READ_CAP)
        .map_err(AppError::from_anyhow)?;
    let bytes: Arc<[u8]> = Arc::from(data.into_boxed_slice());
    *cache = Some(crate::state::RealmBytesCache {
        key,
        bytes: bytes.clone(),
        file_size: stat.size,
    });
    Ok((bytes, stat.size))
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_realm_preview(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<RealmDigest, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let cache = state.realm_session.clone();
    tokio::task::spawn_blocking(move || -> Result<RealmDigest, AppError> {
        let (bytes, file_size) = {
            let mut guard = cache.blocking_lock();
            realm_bytes_cached(&image, &mut guard, &image_id, &node_id)?
        };
        realm_preview::preview(bytes, file_size, false).map_err(AppError::from)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_realm_preview", err))?
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_realm_table_rows(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    table_number: usize,
    offset: usize,
    limit: usize,
) -> Result<RealmRows, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let cache = state.realm_session.clone();
    tokio::task::spawn_blocking(move || -> Result<RealmRows, AppError> {
        let (bytes, _file_size) = {
            let mut guard = cache.blocking_lock();
            realm_bytes_cached(&image, &mut guard, &image_id, &node_id)?
        };
        realm_preview::table_rows(bytes, table_number, offset, limit).map_err(AppError::from)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_realm_table_rows", err))?
}

/// 二进制结构解析的读取上限。头部/导入导出/版本资源都在文件前部，但节区熵需要
/// 节区原始数据 —— 64 MiB 覆盖绝大多数可执行文件；超出的标记 truncated。
const BINARY_READ_CAP: usize = 64 * 1024 * 1024;

/// 解析镜像内文件的二进制结构（ELF / Mach-O / PE / Unix ar，自动按魔数分派），
/// 供「原类型视图」渲染。节点须是受支持的二进制，否则返回解释性错误字符串。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_binary_preview(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<crate::core::preview::binary::binfmt::NativeReport, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(
        move || -> Result<crate::core::preview::binary::binfmt::NativeReport, AppError> {
            let mut session = fs_tree::FsSession::new(&image);
            let stat = session.file_stat(&node_id).map_err(AppError::from_anyhow)?;
            let data = session
                .read_file_limited(&node_id, BINARY_READ_CAP)
                .map_err(AppError::from_anyhow)?;
            crate::core::preview::binary::binfmt::parse(&data, stat.size.max(data.len() as u64))
                .map_err(AppError::from)
        },
    )
    .await
    .map_err(|err| AppError::task_panicked("fs_binary_preview", err))?
}

/// GGUF 解析的读取上限。头部 + 元数据 + 张量信息都在文件前部（即便十万级 token
/// 词表通常也只占数 MB），权重数据本体不读 —— 64 MiB 远超头区所需。
const GGUF_READ_CAP: usize = 64 * 1024 * 1024;

/// 解析镜像内文件的 GGUF 结构，供「原类型视图」渲染（节点须是 GGUF 文件，
/// 否则返回解释性错误字符串）。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_gguf_preview(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<crate::core::preview::gguf::GgufReport, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(
        move || -> Result<crate::core::preview::gguf::GgufReport, AppError> {
            let mut session = fs_tree::FsSession::new(&image);
            let stat = session.file_stat(&node_id).map_err(AppError::from_anyhow)?;
            let data = session
                .read_file_limited(&node_id, GGUF_READ_CAP)
                .map_err(AppError::from_anyhow)?;
            crate::core::preview::gguf::parse(&data, stat.size.max(data.len() as u64))
                .map_err(AppError::from)
        },
    )
    .await
    .map_err(|err| AppError::task_panicked("fs_gguf_preview", err))?
}

/// ZIP 结构解析的读取上限。压缩包的中央目录在文件**尾部**，而文件节点的读取原语
/// 没有按偏移随机读，故须把整包载入内存才能定位中央目录 —— 128 MiB 对齐应用既有
/// 「整文件读取」上限，覆盖绝大多数取证压缩包；超出的在尾部中央目录不可达时报错。
/// 成员内容点击复用 `AppState::zip_session` 缓存的这同一段字节，不再整包重读。
const ZIP_READ_CAP: usize = 128 * 1024 * 1024;

/// 成员内容预览的单次解压上限：文本/十六进制预览只看头部，1 MiB 足够且避免把
/// 多 GiB 成员整体解压进内存。
const ZIP_MEMBER_READ_CAP: usize = 1024 * 1024;

/// 取得（或填充）某个镜像内 ZIP 节点的整包字节，命中单槽缓存则直接复用。返回
/// `(字节, 是否因超上限被截断)`。仅在 `spawn_blocking` 内、持有 `zip_session`
/// 锁时调用。
fn zip_bytes_cached(
    image: &Arc<EvidenceSession>,
    cache: &mut Option<crate::state::ZipBytesCache>,
    image_id: &str,
    node_id: &str,
) -> Result<(Arc<Vec<u8>>, bool), AppError> {
    let key = format!("{image_id}\u{1f}{node_id}");
    if let Some(entry) = cache.as_ref() {
        if entry.key == key {
            return Ok((entry.bytes.clone(), entry.truncated));
        }
    }
    let mut session = fs_tree::FsSession::new(image);
    let stat = session.file_stat(node_id).map_err(AppError::from_anyhow)?;
    let data = session
        .read_file_limited(node_id, ZIP_READ_CAP)
        .map_err(AppError::from_anyhow)?;
    let truncated = stat.size > data.len() as u64;
    let bytes = Arc::new(data);
    *cache = Some(crate::state::ZipBytesCache {
        key,
        bytes: bytes.clone(),
        truncated,
    });
    Ok((bytes, truncated))
}

/// 解析镜像内 ZIP 文件的内部结构（中央目录），供「原类型视图」的目录树渲染。
/// 节点须是有效 ZIP，否则返回解释性错误字符串。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_zip_preview(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<crate::core::preview::zip_preview::ZipPreview, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let cache = state.zip_session.clone();
    tokio::task::spawn_blocking(move || -> Result<crate::core::preview::zip_preview::ZipPreview, AppError> {
        // 只在取字节时持锁，随后释放，避免解析期间阻塞另一格式的预览。
        let (bytes, truncated) = {
            let mut guard = cache.blocking_lock();
            zip_bytes_cached(&image, &mut guard, &image_id, &node_id)?
        };
        match crate::core::preview::zip_preview::parse(&bytes, bytes.len() as u64, truncated) {
            Ok(report) => Ok(report),
            Err(err) if truncated => {
                tracing::warn!("fs_zip_preview truncated parse failed: {err:#}");
                Err(AppError::resource_limit(format!(
                    "压缩包大于 {} MiB 结构解析上限，尾部中央目录不可达，无法读取内部结构；请导出该文件后查看。",
                    ZIP_READ_CAP / (1024 * 1024)
                )))
            }
            Err(err) => Err(AppError::from(err)),
        }
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_zip_preview", err))?
}

/// 解压镜像内 ZIP 文件的某一个成员，回传其内容（限 1 MiB）作文本/十六进制预览。
/// 原始字节经 `crate::host::Response` 回传（前端收 `ArrayBuffer`），避免 JSON 整数
/// 数组膨胀。目录/符号链接/加密/不支持压缩方法的成员返回解释性错误。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_zip_member_read(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    member_path: String,
    max_bytes: usize,
) -> Result<crate::host::Response, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let cache = state.zip_session.clone();
    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, AppError> {
        let (bytes, _truncated) = {
            let mut guard = cache.blocking_lock();
            zip_bytes_cached(&image, &mut guard, &image_id, &node_id)?
        };
        crate::core::preview::zip_preview::read_member(
            &bytes,
            &member_path,
            max_bytes.min(ZIP_MEMBER_READ_CAP),
        )
        .map_err(AppError::from)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_zip_member_read", err))??;
    Ok(crate::host::Response::new(data))
}

/// CSV / TSV 解析的读取上限。表格视图在内存里全量持有解析结果（含前端排序/
/// 统计），故读取窗口须有界 —— 16 MiB 覆盖绝大多数日志 / 导出表；超出的标记
/// truncated。返回行数另有 `csv_table::MAX_ROWS` 上限。
const CSV_READ_CAP: usize = 16 * 1024 * 1024;

/// 解析镜像内文件的 CSV / TSV 结构，供「原类型视图」表格渲染。`delimiter` 为
/// None / 空 / "auto" 时自动嗅探分隔符，否则按给定分隔符解析（前端切换分隔符时
/// 重新调用）。解析在后端，前端只负责渲染与搜索/排序交互。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_csv_preview(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    delimiter: Option<String>,
) -> Result<crate::core::preview::tables::csv_table::CsvTable, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(
        move || -> Result<crate::core::preview::tables::csv_table::CsvTable, AppError> {
            let mut session = fs_tree::FsSession::new(&image);
            let stat = session.file_stat(&node_id).map_err(AppError::from_anyhow)?;
            let data = session
                .read_file_limited(&node_id, CSV_READ_CAP)
                .map_err(AppError::from_anyhow)?;
            let truncated_bytes = stat.size > data.len() as u64;
            let delim =
                crate::core::preview::tables::csv_table::delimiter_from_str(delimiter.as_deref());
            Ok(crate::core::preview::tables::csv_table::parse(
                &data,
                delim,
                truncated_bytes,
            ))
        },
    )
    .await
    .map_err(|err| AppError::task_panicked("fs_csv_preview", err))?
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_vim_swap_preview(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<VimSwapDigest, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || {
        vim_swap::preview_vim_swap(&image, &node_id).map_err(AppError::from)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_vim_swap_preview", err))?
}
