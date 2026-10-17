//! Single-file analysis IPC commands.
//!
//! These commands read one filesystem node and derive metadata from its
//! contents: cryptographic digests and Magika type identification.

use std::time::Instant;

use crate::host::State;
use serde::{Deserialize, Serialize};

use crate::core::analysis::identify::magika_ident;
use crate::core::common::hash::{HashAlgo, MultiHasher};
use crate::core::filesystem::tree::fs_tree;
use crate::state::AppState;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

/// Per-file digest result. Only the algorithms the caller requested are
/// `Some` — the file-list context menu computes hash types on demand.
#[derive(Debug, Serialize, Deserialize)]
pub struct FsDigest {
    pub md5: Option<String>,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
    pub sm3: Option<String>,
    pub bytes_hashed: u64,
    pub truncated: bool,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_digest(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    algos: Vec<HashAlgo>,
) -> Result<FsDigest, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let node_for_log = node_id.clone();
    let started = Instant::now();
    tracing::debug!("fs_digest begin image={image_id} node={node_for_log} algos={algos:?}");

    let result = tokio::task::spawn_blocking(move || -> Result<FsDigest, AppError> {
        // Defensive default: an empty selection means "everything" rather
        // than a digest of nothing.
        let algos = if algos.is_empty() {
            vec![
                HashAlgo::Md5,
                HashAlgo::Sha1,
                HashAlgo::Sha256,
                HashAlgo::Sm3,
            ]
        } else {
            algos
        };
        let mut hasher = MultiHasher::new(&algos);
        let mut session = fs_tree::FsSession::new(&image);
        let bytes_hashed = session
            .stream_file(&node_id, 1024 * 1024, |chunk| {
                hasher.update(chunk);
                Ok(())
            })
            .map_err(AppError::from_anyhow)?;

        let out = hasher.finalize();
        Ok(FsDigest {
            md5: out.md5,
            sha1: out.sha1,
            sha256: out.sha256,
            sm3: out.sm3,
            bytes_hashed,
            truncated: false,
        })
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_digest", err))?;
    match &result {
        Ok(digest) => tracing::debug!(
            "fs_digest done image={image_id} node={node_for_log} elapsed={:.3?} bytes={} md5={} sha1={} sha256={} sm3={}",
            started.elapsed(),
            digest.bytes_hashed,
            digest.md5.as_deref().unwrap_or("-"),
            digest.sha1.as_deref().unwrap_or("-"),
            digest.sha256.as_deref().unwrap_or("-"),
            digest.sm3.as_deref().unwrap_or("-"),
        ),
        Err(err) => tracing::warn!(
            "fs_digest err image={image_id} node={node_for_log} elapsed={:.3?} err={}",
            started.elapsed(),
            err.message,
        ),
    }
    result
}

/// AI 文件类型识别（Magika）。读盘 + 推理都在阻塞线程里做；模型只在首次
/// 调用时加载一次。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_identify_type(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<magika_ident::TypeIdentReport, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let node_for_log = node_id.clone();
    let started = Instant::now();
    let result =
        tokio::task::spawn_blocking(move || -> Result<magika_ident::TypeIdentReport, AppError> {
            magika_ident::identify_file(&image, &node_id).map_err(AppError::from_anyhow)
        })
        .await
        .map_err(|err| AppError::task_panicked("fs_identify_type", err))?;
    match &result {
        Ok(report) => tracing::debug!(
            "fs_identify_type done image={image_id} node={node_for_log} elapsed={:.3?} label={} score={:.3}",
            started.elapsed(),
            report.label,
            report.score,
        ),
        Err(err) => tracing::warn!(
            "fs_identify_type err image={image_id} node={node_for_log} elapsed={:.3?} err={}",
            started.elapsed(),
            err.message,
        ),
    }
    result
}
