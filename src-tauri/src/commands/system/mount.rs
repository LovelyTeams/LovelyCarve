//! Read-only filesystem projection commands.

use crate::host::State;
use crate::state::AppState;

use crate::app::error::AppError;

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_mount_readonly(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    mount_point: String,
) -> Result<DokanMountView, AppError> {
    crate::app::workflows::mount::fs_mount_readonly(state, image_id, node_id, mount_point).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_unmount_readonly(
    state: State<'_, AppState>,
    mount_id: String,
) -> Result<(), AppError> {
    crate::app::workflows::mount::fs_unmount_readonly(state, mount_id).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_mounts(state: State<'_, AppState>) -> Result<Vec<DokanMountView>, AppError> {
    crate::app::workflows::mount::fs_mounts(state).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_suggest_mount_point(state: State<'_, AppState>) -> Result<String, AppError> {
    crate::app::workflows::mount::fs_suggest_mount_point(state).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_available_mount_points(
    state: State<'_, AppState>,
) -> Result<Vec<String>, AppError> {
    crate::app::workflows::mount::fs_available_mount_points(state).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_share_mount(
    state: State<'_, AppState>,
    mount_id: String,
    share_name: Option<String>,
) -> Result<MountShareView, AppError> {
    crate::app::workflows::mount::fs_share_mount(state, mount_id, share_name).await
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_unshare_mount(
    state: State<'_, AppState>,
    mount_id: String,
) -> Result<(), AppError> {
    crate::app::workflows::mount::fs_unshare_mount(state, mount_id).await
}

/// Create (or reuse) an empty subdirectory to serve as a folder mount point,
/// returning its absolute path. Layout is `<root>/<image_id>/<name>`: a
/// per-image grouping layer keeps every partition of one image together (and
/// apart from other images), under which the partition leaf lives. The leaf
/// name is sanitized from `name`; on collision with a non-empty entry we append
/// " (2)", " (3)", … An existing *empty* leaf is reused as-is, so re-mounting
/// the same partition doesn't pile up folders. `root` must already exist — the
/// image layer is created if missing (idempotent), only the leaf is the fresh
/// mount point. Cross-platform (`std::fs`); the Windows-only mount validates
/// NTFS/emptiness afterwards.
#[cfg_attr(feature = "desktop", tauri::command)]
pub fn fs_create_mount_dir(
    root: String,
    image_id: String,
    name: String,
) -> Result<String, AppError> {
    crate::app::workflows::mount::fs_create_mount_dir(root, image_id, name)
}

pub use crate::app::workflows::mount::{DokanMountView, MountShareView};
