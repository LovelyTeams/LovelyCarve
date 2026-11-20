//! Import lifecycle and publication of BCP analysis results.

use std::sync::Arc;

use crate::{
    app::{access::loaded_image_app_error, error::AppError},
    core::{
        analysis::bcp::{
            import::{self, BcpImportResult},
            BcpPackage,
        },
        common::ids::ImageId,
    },
    host::State,
    state::AppState,
    task_manager::{TaskCount, TaskFlag, TaskKind, TaskLabel, TaskProgressUnit},
};

pub(crate) async fn import_bcp(
    state: State<'_, AppState>,
    image_id: ImageId,
    replace: bool,
) -> Result<BcpImportResult, AppError> {
    // Register before a case close or evidence withdrawal can cancel and drain.
    let admission = state.image_load_context.lock().await;
    let case_lease = Arc::clone(&state.bcp_imports).read_owned().await;
    let image = loaded_image_app_error(&state, &image_id).await?;
    if image.folder.is_none() && image.zip.is_none() {
        return Err(AppError::invalid_input("BCP 必须作为文件夹或 ZIP 检材加载"));
    }
    let import_lock = Arc::clone(&image.bcp_import_lock)
        .try_lock_owned()
        .map_err(|_| AppError::invalid_input("此检材的 BCP 导入正在运行"))?;
    let case_dir = {
        let case = state.active_case.lock().await;
        case.as_ref()
            .filter(|case| case.has_evidence(&image_id))
            .map(|case| case.dir.clone())
            .ok_or_else(|| AppError::invalid_input("请先将 BCP 检材添加到活动案件"))?
    };
    if state.removed_case_evidence.lock().await.contains(&image_id) {
        return Err(AppError::cancelled("检材已移除"));
    }
    let task_id = format!("bcp-import:{image_id}");
    let task = state
        .tasks
        .register_if_absent(task_id.clone(), TaskKind::BcpImport, Some(image_id.clone()))
        .ok_or_else(|| AppError::invalid_input("此检材的 BCP 导入正在运行"))?;
    drop(admission);
    task.update(|view| view.subject = vec![image.path.to_string_lossy().into_owned()]);
    let permit = match state.tasks.acquire_resources(&task).await {
        Ok(permit) => permit,
        Err(error) => {
            let result = Err(AppError::from(error));
            task.finish_with(&result);
            return result;
        }
    };
    let active_case = Arc::clone(&state.active_case);
    let images = Arc::clone(&state.images);
    let tasks = state.tasks.clone();
    tokio::task::spawn_blocking(move || {
        let _case_lease = case_lease;
        let _permit = permit;
        let _import_lock = import_lock;
        let token = task.token();
        let writable_case = || {
            let guard = active_case.blocking_lock();
            let case = guard
                .as_ref()
                .filter(|case| case.dir == case_dir && case.has_evidence(&image_id))
                .ok_or_else(|| AppError::cancelled("案件已关闭或检材已移除"))?;
            if case.is_locked() {
                return Err(AppError::invalid_input("请先解锁案件再导入 BCP"));
            }
            Ok(guard)
        };
        let result = (|| {
            if token.is_cancelled() {
                return Err(AppError::cancelled("BCP 导入已取消"));
            }
            let existing = if replace {
                None
            } else {
                import::published(&case_dir, &image_id)?
            };
            if let Some(published) = existing {
                return Ok(published);
            }
            drop(writable_case()?);
            let package = BcpPackage::open_cancellable(&image.path, image.zip.clone(), &token)?;
            let staged = import::stage(
                &case_dir,
                &image_id,
                &image.path,
                &package,
                &token,
                |done, total| {
                    tasks.emit_progress(
                        &task_id,
                        TaskProgressUnit::Entries,
                        done,
                        Some(total),
                        None,
                    );
                },
            )?;
            let _case = writable_case()?;
            if token.is_cancelled()
                || !images
                    .blocking_lock()
                    .get(&image_id)
                    .is_some_and(|loaded| Arc::ptr_eq(loaded, &image))
            {
                return Err(AppError::cancelled("BCP 导入已取消"));
            }
            staged.publish().map_err(AppError::from)
        })();
        let result = result.map_err(|error| {
            if token.is_cancelled() {
                AppError::cancelled("BCP 导入已取消")
            } else {
                error
            }
        });
        if let Ok(published) = &result {
            task.update(|view| {
                view.counts.insert(TaskCount::Rows, published.record_count);
                view.labels
                    .insert(TaskLabel::RunId, published.run_id.clone());
                view.flags.insert(TaskFlag::Persisted);
                if published.reused {
                    view.flags.insert(TaskFlag::Reused);
                }
            });
        }
        task.finish_with(&result);
        drop(task);
        result
    })
    .await
    .map_err(|error| AppError::task_panicked("BCP import", error))?
}
