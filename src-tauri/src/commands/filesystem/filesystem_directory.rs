//! Filtered directory pages, using the volume indexes before the snapshot/live
//! fallback.
use crate::host::State;
use crate::{
    core::{
        catalog::directory_query::{self, DirectoryQuery},
        catalog::filedb::filedb_query::NodePage,
    },
    state::AppState,
};
use {
    crate::app::access::loaded_image_app_error,
    crate::app::error::AppError,
    crate::commands::filesystem::filesystem_snapshot_index::{index_targets, IndexCoverage},
};

pub(crate) async fn page(
    state: State<'_, AppState>,
    image_id: String,
    parent_id: String,
    query: DirectoryQuery,
    cursor: Option<String>,
    page_size: Option<usize>,
) -> Result<NodePage, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let query = DirectoryQuery {
        filter: query.filter.normalized(),
        ..query
    };
    let limit =
        crate::commands::filesystem::filesystem_page::normalize_fs_node_page_size(page_size);
    let targets = index_targets(&state, &image, &image_id, IndexCoverage::Any).await;
    let indexed = tokio::task::spawn_blocking({
        let parent_id = parent_id.clone();
        let query = query.clone();
        let cursor = cursor.clone();
        move || -> anyhow::Result<Option<NodePage>> {
            for target in targets {
                if let Some(page) = directory_query::indexed_page(
                    &target.index,
                    &parent_id,
                    &query,
                    cursor.as_deref(),
                    limit,
                )? {
                    return Ok(Some(page));
                }
            }
            Ok(None)
        }
    })
    .await
    .map_err(|error| AppError::task_panicked("directory_page", error))?
    .map_err(AppError::from_anyhow)?;
    if let Some(page) = indexed {
        return Ok(page);
    }
    let nodes =
        crate::commands::filesystem::filesystem::fs_children(state, image_id, parent_id).await?;
    tokio::task::spawn_blocking(move || {
        directory_query::memory_page(nodes, &query, cursor.as_deref(), limit)
    })
    .await
    .map_err(|error| AppError::task_panicked("directory_page", error))?
    .map_err(AppError::from_anyhow)
}
