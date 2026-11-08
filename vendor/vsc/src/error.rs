//! Error type for the VSS reader.

/// An error decoding a Volume Shadow Copy volume.
///
/// The reader never panics on malformed input: out-of-range field reads yield
/// safe defaults through the bounds-checked helpers. Store metadata that
/// reconstruction depends on (diff-area and bitmap chains) is validated and a
/// defect surfaces as [`VssError::CorruptStoreMetadata`] rather than as
/// silently fabricated snapshot bytes.
#[derive(Debug, thiserror::Error)]
pub enum VssError {
    /// An I/O error reading the underlying volume.
    #[error("i/o error reading VSS volume: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid volume header, catalog chain, or snapshot geometry.
    #[error("VSS metadata at {offset:#x} is corrupt: {reason}")]
    CorruptVolumeMetadata {
        /// Volume-relative location of the offending metadata.
        offset: u64,
        /// The failed integrity check.
        reason: &'static str,
    },

    /// A store index passed to [`crate::VssVolume::store_info`] is past the end
    /// of the enumerated stores.
    #[error("store index {index} out of range ({count} store(s) enumerated)")]
    StoreIndexOutOfRange {
        /// The requested index.
        index: usize,
        /// The number of stores actually present.
        count: usize,
    },

    /// The store has no catalog type-0x03 pointer, so its store-information
    /// block cannot be located (e.g. a Windows 2003 R2 catalog).
    #[error("store {index} has no store-information pointer (no catalog type-0x03 entry)")]
    StoreInfoUnavailable {
        /// The store index whose pointer is missing.
        index: usize,
    },

    /// A store-header offset read from the catalog runs past the end of the
    /// volume — consistent with a corrupt or tampered catalog entry.
    #[error("store {index} header offset {offset} runs past the {volume_size}-byte volume")]
    StoreOffsetOutOfBounds {
        /// The store index whose pointer is out of range.
        index: usize,
        /// The offending offset.
        offset: u64,
        /// The volume size the offset exceeded.
        volume_size: u64,
    },

    /// A block of a store's diff-area or bitmap chain is out of range, has the
    /// wrong record type, is reached twice, or holds an invalid descriptor, so
    /// the snapshot cannot be reconstructed faithfully.
    #[error("store {index} metadata block at {offset:#x} is corrupt: {reason}")]
    CorruptStoreMetadata {
        /// The store index whose metadata is corrupt.
        index: usize,
        /// Volume offset of the offending block.
        offset: u64,
        /// What was wrong with it.
        reason: &'static str,
    },
}
