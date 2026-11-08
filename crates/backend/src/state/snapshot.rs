//! Runtime snapshot state.
use crate::core::catalog::snapshot::volume_snapshot::VolumeSnapshot;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

/// One process-local volume snapshot held outside any case (see
/// [`super::AppState::session_snapshots`]). Caseless sessions prefer a transient
/// SQLite index so large `.dirz` bodies do not have to stay resident. `body` is
/// only populated as a compatibility fallback when indexing fails.
pub struct SessionSnapshot {
    pub body: Option<Arc<VolumeSnapshot>>,
    /// The transient index file, deleted when the snapshot is dropped.
    pub index: Option<SessionIndex>,
    pub volume_key: String,
    pub source_volume_size: u64,
    pub entry_count: u64,
    pub truncated: bool,
}

/// A caseless session's volume index file (same format as a case volume index).
pub struct SessionIndex {
    pub path: PathBuf,
    pub vol_id: i64,
}

impl Drop for SessionSnapshot {
    fn drop(&mut self) {
        if let Some(index) = self.index.take() {
            if let Err(error) =
                crate::core::casework::case_paths::remove_database_files(&index.path)
            {
                tracing::debug!(
                    "session snapshot index {} not removed: {error}",
                    index.path.display()
                );
            }
        }
    }
}

/// Caseless volume snapshots keyed by `(image_id, volume_key)`. See
/// [`super::AppState::session_snapshots`].
pub type SessionSnapshots = Arc<Mutex<HashMap<(String, String), Arc<SessionSnapshot>>>>;
