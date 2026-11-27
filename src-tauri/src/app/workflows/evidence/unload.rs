//! unload implementation.
use crate::core::common::ids::ImageId;
use std::time::Duration;

use super::*;

/// How long unloading waits for the image's cancelled tasks to stop before it
/// releases their resources anyway.
const UNLOAD_TASK_STOP_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn image_unload(state: State<'_, AppState>, image_id: ImageId) -> Result<(), AppError> {
    remove_image_runtime(&state, &image_id).await
}

/// Unloads one image: revokes its registration, cancels its tasks and waits
/// for them to stop, then releases everything the process holds for it.
///
/// The destructuring below names every [`AppState`] field, so adding a field
/// does not compile until this function either releases the field's per-image
/// state or says why the field holds none.
pub(crate) async fn remove_image_runtime(
    state: &State<'_, AppState>,
    image_id: &str,
) -> Result<(), AppError> {
    let AppState {
        images,
        // The case generation gate; holds no image state.
        image_load_context: _,
        // Case-scope lease that closing the case drains; an image's BCP
        // import is one of the tasks cancelled and awaited below.
        bcp_imports: _,
        // Weak per-path entries that free themselves.
        image_load_locks: _,
        plugins,
        // Machine-wide market index and downloads; holds no image state.
        plugin_market: _,
        systems,
        // Serialises plugin opens; holds no image state.
        plugin_open_lock: _,
        tasks,
        // Case-scope tasks, withdrawn when evidence leaves the case.
        scheduler: _,
        carve_scans,
        assembled_arrays,
        #[cfg(all(windows, not(target_arch = "aarch64")))]
        dokan_mounts,
        // Unshared with each Dokan mount below.
        #[cfg(all(windows, not(target_arch = "aarch64")))]
            dokan_mount_shares: _,
        active_case: _,
        // Case-scope tombstones, cleared when the case changes.
        removed_case_evidence: _,
        sqlite_session,
        zip_session,
        realm_session,
        session_snapshots,
        media_temp,
        // Shared scratch directory and admission lock.
        media_scratch: _,
        media_writer: _,
        // Each run's `RunRegistration` leaves when its cancelled task stops.
        global_searches: _,
        timeline_builds: _,
        flat_walk_cache,
        size_trees,
        // Records into whichever case is active; holds no image state.
        task_journal: _,
    } = &**state;

    // Revoke registration before cleanup so an in-flight unlock cannot publish
    // a child after its source is closed.
    let (removed_image, children) = {
        let mut images = images.lock().await;
        let removed = images.remove(image_id);
        let children = images
            .values()
            .filter(|image| {
                image
                    .derived
                    .as_ref()
                    .is_some_and(|derived| derived.parent() == image_id)
            })
            .map(|image| image.id.clone())
            .collect::<Vec<_>>();
        (removed, children)
    };
    // Cleanup is best-effort from here on: the image is already unregistered,
    // so bailing out on the first failure would leak every later resource
    // (plugin volumes, temp files, caches). Keep going and report the first
    // error at the end.
    let mut first_error: Option<AppError> = None;
    let mut record = |error: AppError| {
        tracing::warn!(
            "image unload cleanup for image={image_id} failed: {}",
            error.message
        );
        first_error.get_or_insert(error);
    };
    for child in children {
        if let Err(error) = Box::pin(remove_image_runtime(state, &child)).await {
            record(error);
        }
    }
    // Stop the image's work before releasing what that work reads: temp
    // files, plugin volumes and mounts are only released once its tasks let
    // go, or after the timeout for tasks that ignore cancellation.
    let cancelled_tasks = tasks.cancel_image(image_id);
    if cancelled_tasks > 0 {
        tracing::info!("image_unload cancelled {cancelled_tasks} task(s) for image={image_id}");
    }
    let holding = tasks
        .wait_image_stopped(image_id, UNLOAD_TASK_STOP_TIMEOUT)
        .await;
    if !holding.is_empty() {
        tracing::warn!(
            "image_unload releases image={image_id} while tasks still stop: {}",
            holding.join(", ")
        );
    }
    tasks.forget_image(image_id);
    carve_scans
        .lock()
        .await
        .retain(|_, session| session.read().image_id != image_id);
    #[cfg(all(windows, not(target_arch = "aarch64")))]
    {
        let mounts = {
            let mut mounts = dokan_mounts.lock().await;
            let ids = mounts
                .iter()
                .filter(|(_, session)| session.view().image_id == image_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| mounts.remove(&id))
                .collect::<Vec<_>>()
        };
        for session in mounts {
            if let Err(error) =
                crate::app::workflows::mount::unshare_mount_by_id(state, &session.view().id).await
            {
                record(error);
            }
            match tokio::task::spawn_blocking(move || session.stop()).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => record(AppError::from(error)),
                Err(error) => record(AppError::task_panicked("Dokan unmount", error)),
            }
        }
    }
    if let Some(image) = removed_image {
        image.revoke_decrypted_partitions();
        image.revoke_apfs_keys();
        let manager = plugins.read().clone();
        for volume in image.take_external_volumes() {
            if let Err(error) = volume.close().await {
                tracing::debug!("plugin fs.close failed during image unload: {error}");
            }
            if let Some(manager) = &manager {
                manager.sources().remove(volume.source_id());
            }
        }
    }
    systems.forget(image_id);
    assembled_arrays
        .lock()
        .await
        .retain(|_key, registered_id| registered_id != image_id);
    {
        let mut realm_session = realm_session.lock().await;
        if realm_session
            .as_ref()
            .is_some_and(|entry| entry.key.image == image_id)
        {
            *realm_session = None;
        }
    }
    {
        let mut zip_session = zip_session.lock().await;
        if zip_session
            .as_ref()
            .is_some_and(|entry| entry.key.image == image_id)
        {
            *zip_session = None;
        }
    }
    {
        let mut sqlite_session = sqlite_session.lock().await;
        if sqlite_session
            .as_ref()
            .is_some_and(|session| session.image_id() == image_id)
        {
            *sqlite_session = None;
        }
    }
    flat_walk_cache.lock().purge_image(image_id);
    size_trees.lock().purge_image(image_id);
    // Drop (and delete) any media temp files materialised for this image's
    // video / audio previews — their node ids will never be looked up again.
    media_temp.lock().await.purge_image(image_id);
    // Drop the image's installed caseless session snapshots; their builds
    // were cancelled with the image's other tasks.
    crate::app::workflows::snapshot::snapshot_session::purge_session_snapshots(
        session_snapshots,
        tasks,
        image_id,
    )
    .await;
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
