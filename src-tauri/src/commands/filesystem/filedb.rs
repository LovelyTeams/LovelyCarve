//! `filedb` IPC: metadata queries and the MetaAddr
//! addressing channel.
//!
//! * `filedb_volumes` — the image's filesystem array (`fs_all`): the mapping
//!   between `vol_id` (array position) and the walker's volume keys.
//! * `filedb_query_files` — the generic filter path over the lowercase copy
//!   columns (name / path prefix or substring, extension, size range, kind,
//!   delete state), naturally ordered and `LIMIT/OFFSET` paged.
//! * `filedb_resolve_meta_addr` — the `QueryFileSt` channel: resolve
//!   `(image, volId, metaAddr, metaAddrHigh)` back to the node id the read
//!   commands accept (`fs_read*`, `fs_preview*`, …).

use std::sync::Arc;

use crate::host::State;
use serde::Serialize;

use crate::app::access::loaded_image_app_error;
use crate::app::error::AppError;
use crate::commands::filesystem::filesystem_snapshot_index::{
    index_targets, IndexCoverage, SnapshotIndexTarget,
};
use crate::core::catalog::filedb::filedb_query::{self, FileFilter};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::fs_all::{self, FsEntry};
use crate::core::filesystem::meta_addr::FileQuery;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiledbVolume {
    pub vol_id: i64,
    pub volume_key: String,
    pub fs_prefix: String,
    pub kind: String,
    pub size: u64,
}

/// The image's filesystem array: `volId` is the index.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn filedb_volumes(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<Vec<FiledbVolume>, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    if image.is_index_only() {
        let active = state.active_case.lock().await;
        let case = active
            .as_ref()
            .ok_or_else(|| AppError::not_found("离线索引所属案件已关闭"))?;
        let targets = crate::commands::filesystem::filesystem_snapshot_index::case_index_targets(
            case,
            &image,
            &image_id,
            IndexCoverage::Any,
        );
        return Ok(targets
            .iter()
            .filter_map(|target| {
                case.volume(&image_id, &target.volume_key)
                    .map(|record| FiledbVolume {
                        vol_id: record.vol_id,
                        volume_key: record.volume_key.clone(),
                        fs_prefix: "indexed".to_string(),
                        kind: "indexed".to_string(),
                        size: record.source_size,
                    })
            })
            .collect());
    }
    let volumes = tokio::task::spawn_blocking(move || fs_all_volumes(&image))
        .await
        .map_err(|err| AppError::task_panicked("filedb_volumes fs_all", err))??;
    Ok(volumes)
}

fn fs_all_volumes(image: &Arc<EvidenceSession>) -> anyhow::Result<Vec<FiledbVolume>> {
    Ok(fs_all::FsAll::for_image(image)
        .entries()
        .iter()
        .map(
            |FsEntry {
                 vol_id,
                 volume_key,
                 kind,
                 fs_prefix,
                 size,
                 ..
             }| FiledbVolume {
                vol_id: *vol_id,
                volume_key: volume_key.clone(),
                fs_prefix: (*fs_prefix).to_string(),
                kind: format!("{kind:?}").to_lowercase(),
                size: *size,
            },
        )
        .collect())
}

/// Query the metadata index with structured filters. With no
/// `volume_key`, every indexed volume of the image is scanned and the results
/// concatenated (natural order within each volume).
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn filedb_query_files(
    state: State<'_, AppState>,
    image_id: String,
    volume_key: Option<String>,
    filter: FileFilter,
) -> Result<Vec<filedb_query::FileHit>, AppError> {
    let image = {
        let images = state.images.lock().await;
        images.get(&image_id).cloned()
    };
    let filter = filter.normalized();
    let targets = targets_for(&state, image.as_deref(), &image_id, volume_key.as_deref()).await;
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let hits =
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<filedb_query::FileHit>> {
            let mut hits = Vec::new();
            for target in &targets {
                hits.extend(filedb_query::query_files(&target.index, &filter)?);
            }
            Ok(hits)
        })
        .await
        .map_err(|err| AppError::task_panicked("filedb_query_files", err))??;
    Ok(hits)
}

/// `QueryFileSt` channel: map a MetaAddr four-tuple back to the node id that
/// `fs_read*` / `fs_stat` accept. Path-addressed entries (`metaAddr = 0`)
/// require `full_path`.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn filedb_resolve_meta_addr(
    state: State<'_, AppState>,
    query: FileQuery,
) -> Result<Option<String>, AppError> {
    let image_id = query.image_id.clone();
    let image = {
        let images = state.images.lock().await;
        images.get(&image_id).cloned()
    };
    let targets = targets_for(&state, image.as_deref(), &image_id, None).await;
    let full_path = query.full_path.clone();
    let meta_addr = query.meta_addr;
    let meta_addr_high = query.meta_addr_high;
    let nid = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        for target in targets
            .iter()
            .filter(|target| target.vol_id == query.vol_id)
        {
            if let Some(nid) = filedb_query::node_id_for_meta_addr(
                &target.index,
                meta_addr,
                meta_addr_high,
                full_path.as_deref(),
            )? {
                return Ok(Some(nid));
            }
        }
        Ok(None)
    })
    .await
    .map_err(|err| AppError::task_panicked("filedb_resolve_meta_addr", err))??;
    Ok(nid)
}

/// Index targets: the active case's published volume indexes, or the caseless
/// session indexes — same dual source the browsing commands use.
async fn targets_for(
    state: &AppState,
    image: Option<&EvidenceSession>,
    image_id: &str,
    volume_key: Option<&str>,
) -> Vec<SnapshotIndexTarget> {
    let Some(image) = image else {
        return Vec::new();
    };
    index_targets(state, image, image_id, IndexCoverage::Any)
        .await
        .into_iter()
        .filter(|target| volume_key.is_none_or(|wanted| wanted == target.volume_key))
        .collect()
}
