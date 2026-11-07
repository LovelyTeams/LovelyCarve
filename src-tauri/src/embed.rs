//! Embedding interface for in-process frontends such as the GPUI desktop app
//! (`src/bin/lovelycarve-gpui`). Like the CLI, an embedding host builds
//! `AppState` and an `AppHandle` itself, prepares the backend once, calls the
//! command functions directly, and shuts the backend down in order on exit.
use crate::{host::AppHandle, state::AppState};

/// Prepare `state` for commands: preview scratch space, plugin discovery and
/// the task, scheduler and plugin event routes to `host`.
pub fn initialize(state: &AppState, host: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    crate::app::setup::initialize(state, host).map(|_scratch| ())
}

/// Stop the backend before the process exits: cancel native tasks, close the
/// active case (which unbinds the task scheduler and unloads its images),
/// stop plugin processes and release media scratch space.
pub async fn shutdown(state: &AppState) {
    crate::app::lifecycle::shutdown(state).await;
}

/// A name from evidence made safe as one host path component (illegal
/// characters, trailing dots and spaces, Win32 device names), as every
/// export applies it.
pub fn sanitize_export_name(name: &str) -> String {
    crate::core::filesystem::tree::fs_tree_export::sanitize_export_component(name)
}
