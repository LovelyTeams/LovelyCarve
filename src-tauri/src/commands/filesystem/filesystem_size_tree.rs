//! Treemap data of the file list: `fs_size_tree`.
//!
//! The treemap sizes a directory by everything beneath it, which only a
//! whole-volume pass can tell. It is served from the volume's published
//! index — the active case's, or the caseless session's — aggregated once
//! into a [`SizeTree`] kept in [`crate::state::SizeTreeCache`]. For now only
//! NTFS directories are served.

use std::sync::Arc;

use serde::Serialize;

use crate::app::access::loaded_image_app_error;
use crate::app::error::AppError;
use crate::commands::filesystem::filesystem_snapshot_index::{
    index_targets, IndexCoverage, SnapshotIndexTarget,
};
use crate::core::catalog::size_tree::{
    self, DeletedEntries, SizeMetric, SizeTree, SizeTreeView, ViewOptions,
};
use crate::core::evidence::image::PartitionKind;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::host::State;
use crate::state::AppState;

/// Levels a view opens when the caller does not say.
const DEFAULT_DEPTH: u32 = 3;

/// The treemap data of one directory, or why there is none.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum FsSizeTree {
    /// The directory and what lies beneath it.
    Ready { view: Box<SizeTreeView> },
    /// No published index covers the directory: its volume snapshot is still
    /// building or failed, was cut short before reaching it, or the directory
    /// is deleted (deleted directories are not walked).
    Unindexed,
    /// Not an NTFS directory: the treemap serves NTFS volumes only.
    Unsupported,
}

/// Space usage of NTFS directory `node_id` (a directory id, or the volume
/// key of the volume root) and of up to `depth` levels beneath it, sized by
/// `metric` (`bytes`, the default, or `files`). Deleted entries count only
/// with `include_deleted`. Entries worth less than `min_share` of the
/// directory merge into their parent's `others` tail.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_size_tree(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    depth: Option<u32>,
    metric: Option<String>,
    include_deleted: Option<bool>,
    min_share: Option<f64>,
) -> Result<FsSizeTree, AppError> {
    let metric = match metric.as_deref() {
        None | Some("bytes") => SizeMetric::Bytes,
        Some("files") => SizeMetric::Files,
        Some(other) => {
            return Err(AppError::invalid_input(format!(
                "树图不支持按「{other}」度量"
            )))
        }
    };
    let deleted = if include_deleted.unwrap_or(false) {
        DeletedEntries::Include
    } else {
        DeletedEntries::Exclude
    };
    let options = ViewOptions::new(
        depth.unwrap_or(DEFAULT_DEPTH),
        metric,
        deleted,
        min_share.unwrap_or(0.0),
    );
    let image = loaded_image_app_error(&state, &image_id).await?;
    let targets = index_targets(&state, &image, &image_id, IndexCoverage::Any).await;
    let located = {
        let node_id = node_id.clone();
        tokio::task::spawn_blocking(move || locate(&targets, &node_id))
            .await
            .map_err(|err| AppError::task_panicked("fs_size_tree locate", err))?
    };
    let Some((target, ntfs)) = located else {
        return Ok(if may_be_ntfs(&image, &node_id) {
            FsSizeTree::Unindexed
        } else {
            FsSizeTree::Unsupported
        });
    };
    if !ntfs {
        return Ok(FsSizeTree::Unsupported);
    }

    let started = std::time::Instant::now();
    let slot = state.size_trees.lock().slot(&image_id, &target.index);
    let tree: Arc<SizeTree> = slot
        .get_or_try_init(|| {
            let index = target.index.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    size_tree::build_size_tree(&index, unix_now()).map(Arc::new)
                })
                .await
                .map_err(|err| AppError::task_panicked("fs_size_tree build", err))?
                .map_err(AppError::from_anyhow)
            }
        })
        .await
        .map(Arc::clone)?;
    let index = target.index.clone();
    let start = node_id.clone();
    let view = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<SizeTreeView>> {
        let Some(mut view) = tree.view(&start, &options) else {
            return Ok(None);
        };
        size_tree::read_view_nodes(&index, &mut view)?;
        Ok(Some(view))
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_size_tree view", err))?
    .map_err(AppError::from_anyhow)?;
    tracing::debug!(
        "fs_size_tree image={image_id} node={node_id} volume={} elapsed={:.3?}",
        target.volume_key,
        started.elapsed()
    );
    Ok(match view {
        Some(view) => FsSizeTree::Ready {
            view: Box::new(view),
        },
        None => FsSizeTree::Unindexed,
    })
}

/// The index that walked directory `node_id`, and whether it is NTFS. An
/// index that cannot be read is skipped: another volume's may still answer.
fn locate(targets: &[SnapshotIndexTarget], node_id: &str) -> Option<(SnapshotIndexTarget, bool)> {
    for target in targets {
        match size_tree::ntfs_directory(&target.index, node_id) {
            Ok(Some(ntfs)) => return Some((target.clone(), ntfs)),
            Ok(None) => {}
            Err(err) => tracing::warn!(
                "fs_size_tree: index {} of volume {} unreadable: {err:#}",
                target.index.display(),
                target.volume_key
            ),
        }
    }
    None
}

/// Whether an unindexed `node_id` could still be an NTFS directory once its
/// volume is indexed: an NTFS id, an NTFS (or unlocked BitLocker) partition,
/// or a logical volume, whose filesystem only its index tells.
fn may_be_ntfs(image: &EvidenceSession, node_id: &str) -> bool {
    if node_id.starts_with(size_tree::NTFS_NODE_PREFIX) || node_id.starts_with("lvm:") {
        return true;
    }
    let Some(partition_id) = node_id
        .strip_prefix("part:")
        .and_then(|id| id.parse::<u8>().ok())
    else {
        return false;
    };
    image.partitions.iter().any(|partition| {
        partition.id == partition_id
            && (partition.kind == PartitionKind::Ntfs
                || (partition.kind == PartitionKind::Bitlocker
                    && image.bitlocker_unlocked(partition_id).is_some()))
    })
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}
