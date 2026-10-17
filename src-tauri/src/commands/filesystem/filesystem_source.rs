use crate::host::State;
use crate::{
    core::{
        filesystem::fs_source_location::{indexed_source_chain, source_chain},
        filesystem::tree::fs_tree::NodeKind,
    },
    state::AppState,
};
use {
    crate::app::access::loaded_image_app_error,
    crate::app::error::AppError,
    crate::commands::filesystem::filesystem::{FsLocateNode, FsLocateResult},
    crate::commands::filesystem::filesystem_snapshot_index::{index_targets, IndexCoverage},
};

/// Locate a root-relative plugin source using live directories or verified offline indexes.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_source_locate(
    state: State<'_, AppState>,
    image_id: String,
    root_id: String,
    path: String,
) -> Result<FsLocateResult, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let indexes = if image.is_index_only() {
        index_targets(&state, &image, &image_id, IndexCoverage::Any)
            .await
            .into_iter()
            .map(|target| target.index)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    tokio::task::spawn_blocking(move || {
        let mut nodes = if image.is_index_only() {
            indexed_source_chain(&image, &indexes, &root_id, &path)
        } else {
            source_chain(&image, &root_id, &path)
        }
        .map_err(|error| AppError::not_found(format!("定位源文件失败：{error:#}")))?;
        let node_id = nodes
            .last()
            .map_or_else(|| root_id.clone(), |node| node.id.clone());
        let file_id = if nodes.last().is_some_and(|node| node.kind != NodeKind::Dir) {
            nodes.pop().map(|node| node.id)
        } else {
            None
        };
        let frames = nodes
            .into_iter()
            .map(|node| FsLocateNode {
                id: node.id,
                name: node.name,
                kind: "dir".to_owned(),
            })
            .collect();
        Ok(FsLocateResult {
            node_id,
            volume_key: root_id,
            frames,
            file_id,
        })
    })
    .await
    .map_err(|error| AppError::task_panicked("fs_source_locate", error))?
}
