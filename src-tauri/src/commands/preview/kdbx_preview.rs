//! KeePass KDBX IPC for the original-type view: the header without a key,
//! the decrypted database, and attachment bytes.

use crate::host::State;

use crate::core::common::ids::{ImageId, NodeId};
use crate::core::preview::kdbx_preview::{
    self, KdbxAttachmentRef, KdbxCredentials, KdbxDatabaseReport, KdbxHeaderReport,
};
use crate::state::AppState;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

/// The outer header of a KDBX database in the evidence: version, cipher,
/// compression, seeds and key derivation parameters. Needs no key.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_kdbx_header(
    state: State<'_, AppState>,
    image_id: ImageId,
    node_id: NodeId,
) -> Result<KdbxHeaderReport, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || {
        kdbx_preview::read_header(&image, &node_id).map_err(AppError::from)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_kdbx_header", err))?
}

/// Decrypt a KDBX database with a password and/or keyfile (on the host or in
/// the same evidence), or a transformed key, and report its groups, entries,
/// history, attachments and metadata. Wrong credentials fail with
/// `invalidInput`.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_kdbx_open(
    state: State<'_, AppState>,
    image_id: ImageId,
    node_id: NodeId,
    credentials: KdbxCredentials,
) -> Result<KdbxDatabaseReport, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    tokio::task::spawn_blocking(move || {
        let (database, used, size) = kdbx_preview::open_evidence(&image, &node_id, &credentials)?;
        Ok(kdbx_preview::database_report(&database, used, size))
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_kdbx_open", err))?
}

/// Up to `max_bytes` (at most 1 MiB) of one attachment, as raw bytes.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_kdbx_attachment(
    state: State<'_, AppState>,
    image_id: ImageId,
    node_id: NodeId,
    credentials: KdbxCredentials,
    attachment: KdbxAttachmentRef,
    max_bytes: usize,
) -> Result<crate::host::Response, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, AppError> {
        let (database, _, _) = kdbx_preview::open_evidence(&image, &node_id, &credentials)?;
        kdbx_preview::attachment_bytes(
            &database,
            &attachment,
            max_bytes.min(kdbx_preview::ATTACHMENT_READ_CAP),
        )
        .map_err(AppError::from)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_kdbx_attachment", err))??;
    Ok(crate::host::Response::new(data))
}
