//! Runtime preview state.
use std::sync::Arc;

/// Cached bytes of one inner ZIP archive (see [`AppState::zip_session`]).
/// `truncated` records whether the read hit the structure-parse cap, so the
/// preview can warn without re-stat-ing the file on a cache hit.
pub struct ZipBytesCache {
    pub key: String,
    pub bytes: Arc<Vec<u8>>,
    pub truncated: bool,
}

/// Cached bytes of one Realm database (see [`AppState::realm_session`]).
pub struct RealmBytesCache {
    pub key: String,
    pub bytes: Arc<[u8]>,
    pub file_size: u64,
}
