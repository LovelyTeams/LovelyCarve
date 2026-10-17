//! Awaitable preparation of one Docker container; completion includes durable indexes.

use crate::host::State;
use serde::Serialize;

use crate::core::casework::case_ledger;
use crate::core::catalog::host_docker_index::{self, CaseTarget, PrepareMode};
use crate::state::AppState;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerIndexStatus {
    ready: bool,
    persisted: bool,
    entry_count: u64,
}

/// Status only validates/restores existing data; build composes and saves the
/// selected container. No sibling containers are analyzed and no events are
/// emitted: callers await the complete operation to finish their task row.
/// The build runs outside the case lock; its snapshot and index are then
/// published into the case like any other volume.
pub async fn fs_docker_prepare(
    state: State<'_, AppState>,
    image_id: String,
    root_id: String,
    case_dir: Option<String>,
    mode: PrepareMode,
) -> Result<DockerIndexStatus, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let target = {
        let guard = state.active_case.lock().await;
        let current = guard
            .as_ref()
            .map(|case| case.dir.to_string_lossy().into_owned());
        if current != case_dir {
            return Err(AppError::cancelled("案件已切换，请重新打开容器"));
        }
        match guard.as_ref() {
            Some(case) if !case.has_evidence(&image_id) => {
                return Err(AppError::not_found("检材已从案件移除"));
            }
            Some(case) => Some(CaseTarget::of(case, &image_id, &root_id)),
            None => None,
        }
    };
    *image.host_docker_cache_dir.write() = target.as_ref().map(|target| target.cache_dir.clone());
    let worker_target = target.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        host_docker_index::prepare(
            &image,
            &root_id,
            worker_target.as_ref(),
            mode,
            case_ledger::now(),
        )
    })
    .await
    .map_err(|error| AppError::task_panicked("Docker 索引", error))?
    .map_err(AppError::from_anyhow)?;
    let Some(staged) = prepared.staged else {
        return Ok(DockerIndexStatus {
            ready: prepared.ready,
            persisted: prepared.persisted,
            entry_count: prepared.entry_count,
        });
    };
    // Publish under the case lock on a blocking thread (rename + ledger).
    let active_case = std::sync::Arc::clone(&state.active_case);
    let expected_dir = target.map(|target| target.staging_dir);
    let persisted = tokio::task::spawn_blocking(move || {
        let mut guard = active_case.blocking_lock();
        let Some(case) = guard
            .as_mut()
            .filter(|case| Some(case.staging_dir()) == expected_dir)
        else {
            return Err(AppError::cancelled("案件已切换，容器索引未保存"));
        };
        host_docker_index::publish(case, staged).map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|error| AppError::task_panicked("Docker 索引发布", error))??;
    if !persisted {
        return Err(AppError::cancelled("案件或检材已切换，容器索引未保存"));
    }
    Ok(DockerIndexStatus {
        ready: prepared.ready,
        persisted,
        entry_count: prepared.entry_count,
    })
}
