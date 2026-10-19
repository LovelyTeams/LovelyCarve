//! unload implementation.
use super::*;

pub async fn image_unload(state: State<'_, AppState>, image_id: String) -> Result<(), AppError> {
    remove_image_runtime(&state, &image_id).await
}

pub(crate) async fn remove_image_runtime(
    state: &State<'_, AppState>,
    image_id: &str,
) -> Result<(), AppError> {
    // Revoke registration before cleanup so an in-flight unlock cannot publish
    // a child after its source is closed.
    let (removed_image, children) = {
        let mut images = state.images.lock().await;
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
    let cancelled_tasks = state.tasks.cancel_image(image_id);
    if cancelled_tasks > 0 {
        tracing::info!("image_unload cancelled {cancelled_tasks} task(s) for image={image_id}");
    }
    state.tasks.forget_image(image_id);
    crate::app::workflows::evidence::purge_image_scans(state, image_id).await;
    #[cfg(target_os = "windows")]
    {
        let mounts = {
            let mut mounts = state.dokan_mounts.lock().await;
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
        let manager = state.plugins.read().clone();
        for volume in image.take_external_volumes() {
            if let Err(error) = volume.close().await {
                tracing::debug!("plugin fs.close failed during image unload: {error}");
            }
            if let Some(manager) = &manager {
                manager.sources().remove(volume.source_id());
            }
        }
    }
    state.systems.forget(image_id);
    let mut assembled = state.assembled_arrays.lock().await;
    assembled.retain(|_key, registered_id| registered_id != image_id);
    drop(assembled);
    let image_key_prefix = format!("{image_id}\u{1f}");
    let mut realm_session = state.realm_session.lock().await;
    if realm_session
        .as_ref()
        .is_some_and(|entry| entry.key.starts_with(image_key_prefix.as_str()))
    {
        *realm_session = None;
    }
    drop(realm_session);
    let mut zip_session = state.zip_session.lock().await;
    if zip_session
        .as_ref()
        .is_some_and(|entry| entry.key.starts_with(image_key_prefix.as_str()))
    {
        *zip_session = None;
    }
    drop(zip_session);
    let mut sqlite_session = state.sqlite_session.lock().await;
    if sqlite_session
        .as_ref()
        .is_some_and(|session| session.image_id() == image_id)
    {
        *sqlite_session = None;
    }
    drop(sqlite_session);
    state.flat_walk_cache.lock().purge_image(image_id);
    // Drop (and delete) any media temp files materialised for this image's
    // video / audio previews — their node ids will never be looked up again.
    let mut media_temp = state.media_temp.lock().await;
    media_temp.purge_image(image_id);
    drop(media_temp);
    // Cancel any in-flight caseless session-snapshot build for this image and
    // drop its installed session snapshots.
    crate::app::workflows::snapshot::snapshot_session::purge_session_snapshots(
        &state.session_snapshots,
        &state.tasks,
        image_id,
    )
    .await;
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub(crate) async fn purge_image_scans(state: &State<'_, AppState>, image_id: &str) {
    let mut scans = state.carve_scans.lock().await;
    scans.retain(|_, session| session.read().image_id != image_id);
}
