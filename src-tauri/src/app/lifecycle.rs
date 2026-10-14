//! Window leases and ordered desktop shutdown.
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
        let discovery = manager.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || discovery.close_discovery()).await {
            tracing::warn!("closing plugin discovery failed: {error}");
        }
        manager.shutdown().await;
    }
}

pub(crate) fn on_event(app_handle: &tauri::AppHandle, event: tauri::RunEvent) {
    if let tauri::RunEvent::WindowEvent {
        label,
        event: tauri::WindowEvent::Destroyed,
        ..
    } = &event
    {
        use tauri::Manager as _;
        let state = app_handle.state::<AppState>();
        tauri::async_runtime::block_on(async {
            state.media_temp.lock().await.release_owner(label);
        });
    }
    if matches!(event, tauri::RunEvent::Exit) {
        use tauri::Manager as _;

        let state = app_handle.state::<AppState>();
        tauri::async_runtime::block_on(release_media(&state));
        // 先解绑任务调度(运行中任务置 offline,下次打开案件时恢复),
        // 再关停插件子进程。
        let scheduler = app_handle.state::<AppState>().scheduler.clone();
        tauri::async_runtime::block_on(scheduler.unbind_case());
        tauri::async_runtime::block_on(shutdown_plugins(&state));
    }
}
