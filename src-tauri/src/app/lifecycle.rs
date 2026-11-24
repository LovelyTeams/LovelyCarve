//! Media leases and ordered shutdown shared by the desktop app and the CLI.
use crate::state::AppState;

pub(crate) async fn release_media(state: &AppState) {
    state.media_temp.lock().await.clear();
    let scratch = state.media_scratch.lock().clone();
    if let Some(scratch) = scratch {
        scratch.cleanup();
    }
}

/// How long shutdown waits for queued task events to reach the case journal.
const JOURNAL_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Writes the task events queued so far into the case journal. Shutdown calls
/// it before the case closes, so tasks that already ended keep their record.
pub(crate) async fn flush_task_journal(state: &AppState) {
    if let Some(journal) = state.task_journal.get() {
        journal.flush(JOURNAL_FLUSH_TIMEOUT).await;
    }
}

pub(crate) async fn shutdown_plugins(state: &AppState) {
    let manager = state.plugins.read().clone();
    if let Some(manager) = manager {
        manager.shutdown().await;
    }
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
        crate::host::evidence_links::stop(app_handle);
        let state = app_handle.state::<AppState>();
        tauri::async_runtime::block_on(flush_task_journal(&state));
        tauri::async_runtime::block_on(release_media(&state));
        // 先解绑任务调度(运行中任务置 offline,下次打开案件时恢复),
        // 再关停插件子进程。
        let scheduler = state.scheduler.clone();
        tauri::async_runtime::block_on(scheduler.unbind_case());
        tauri::async_runtime::block_on(shutdown_plugins(&state));
    }
}
