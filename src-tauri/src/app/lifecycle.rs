//! Media leases and ordered shutdown shared by the desktop app and the CLI.
use crate::state::AppState;

pub(crate) async fn release_media(state: &AppState) {
    state.media_temp.lock().await.clear();
    let scratch = state.media_scratch.lock().clone();
    if let Some(scratch) = scratch {
        scratch.cleanup();
    }
}

pub(crate) async fn shutdown_plugins(state: &AppState) {
    let manager = state.plugins.read().clone();
    if let Some(manager) = manager {
        manager.shutdown().await;
    }
}

/// Ordered process shutdown for hosts that own the whole backend lifetime (the
/// CLI and the GPUI desktop app): cancel native tasks, close the active case
/// (which unbinds the task scheduler and unloads its images), stop plugin
/// processes and release media scratch space.
pub(crate) async fn shutdown(state: &AppState) {
    for task in state.tasks.list() {
        state.tasks.cancel(&task.id);
    }
    if let Err(error) =
        crate::app::workflows::casework::case_close(crate::host::State::new(state)).await
    {
        tracing::error!("shutdown case close: {}", error.message);
    }
    shutdown_plugins(state).await;
    release_media(state).await;
}

/// Desktop run-loop hook: closed windows release their media leases, and
/// exit unbinds the task scheduler before stopping plugin processes.
#[cfg(feature = "desktop")]
pub(crate) fn on_event(app_handle: &tauri::AppHandle, event: tauri::RunEvent) {
    use tauri::Manager as _;
    if let tauri::RunEvent::WindowEvent {
        label,
        event: tauri::WindowEvent::Destroyed,
        ..
    } = &event
    {
        let state = app_handle.state::<AppState>();
        tauri::async_runtime::block_on(async {
            state.media_temp.lock().await.release_owner(label);
        });
    }
    if matches!(event, tauri::RunEvent::Exit) {
        let state = app_handle.state::<AppState>();
        tauri::async_runtime::block_on(release_media(&state));
        // 先解绑任务调度(运行中任务置 offline,下次打开案件时恢复),
        // 再关停插件子进程。
        let scheduler = state.scheduler.clone();
        tauri::async_runtime::block_on(scheduler.unbind_case());
        tauri::async_runtime::block_on(shutdown_plugins(&state));
    }
}
