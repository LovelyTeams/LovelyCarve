//! Media cleanup and ordered shutdown shared by the desktop app and the CLI.
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
