#![warn(missing_docs)]
//! Read-only QCOW 1/2/3 logical disks, raw DEFLATE, differential backing sources
//! and legacy AES-CBC. See LOVELYCARVE.md for scope and provenance.
mod crypto;
mod error;
mod extension;
mod feature;
mod header;
mod int;
mod path;
mod read;
pub use crypto::EncryptionKey;
pub use error::Error;
use lru::LruCache;
use positioned_io::{ReadAt, Size};
pub use read::Reader;
use std::{
    fmt,
    num::NonZeroUsize,
    path::Path,
    sync::{Arc, Mutex},
};

/// Result of a QCOW operation.
pub type Result<T> = std::result::Result<T, Error>;
/// Logical parent with a stable known size. Implementers must support concurrent
/// positioned reads and report short allocated reads instead of synthesizing data.
pub trait BackingSource: ReadAt + Size + Send + Sync {}
impl<T: ReadAt + Size + Send + Sync> BackingSource for T {}

/// Format metadata. Snapshot count does not imply selectable historical views.
#[derive(Debug, Clone)]
pub struct ImageInfo {
    /// On-disk version.
    pub version: u32,
    /// Logical disk length.
    pub media_size: u64,
    /// Allocation cluster length.
    pub cluster_size: u64,
    /// 0 = plaintext, 1 = legacy AES-CBC.
    pub encryption_method: u32,
    /// Original backing filename before resolution.
    pub backing_filename: Option<std::path::PathBuf>,
    /// Declared backing format, if present.
    pub backing_format: Option<String>,
    /// Number of internal snapshots in the header.
    pub snapshot_count: u32,
    /// Raw incompatible bitmap.
    pub incompatible_features: u64,
    /// Raw compatible bitmap.
    pub compatible_features: u64,
    /// Raw auto-clear bitmap.
    pub autoclear_features: u64,
}

/// Immutable logical disk with per-image caches; credentials are never debugged.
pub struct Qcow2<I: ReadAt> {
    header: header::Header,
    io: I,
    source_size: Option<u64>,
    l1: Vec<u8>,
    l2_cache: Mutex<LruCache<u64, u64>>,
    decoded_cache: Mutex<LruCache<u64, Arc<Vec<u8>>>>,
    key: Option<EncryptionKey>,
    backing: Option<Arc<dyn BackingSource>>,
    backing_size: Option<u64>,
}

impl<I: ReadAt + Size> Qcow2<I> {
    /// Open without credentials. Encrypted images remain inspectable; all
    /// logical reads fail until reopened with `open_with_key`.
    pub fn open(io: I) -> Result<Self> {
        Self::open_key(io, None)
    }
    /// Open with a legacy AES key. QCOW has no key verifier: verify plaintext
    /// independently before relying on it as authenticated evidence.
    pub fn open_with_key(io: I, key: EncryptionKey) -> Result<Self> {
        Self::open_key(io, Some(key))
    }
    fn open_key(io: I, key: Option<EncryptionKey>) -> Result<Self> {
        let source_size = io.size()?;
        let header = header::Header::read(&io, source_size)?;
        let mut l1 = vec![0; header.l1_size as usize * 8];
        io.read_exact_at(header.l1_offset, &mut l1)?;
        let entries =
            NonZeroUsize::new(4096).ok_or_else(|| Error::Internal("zero cache size".into()))?;
        let decoded =
            NonZeroUsize::new(8).ok_or_else(|| Error::Internal("zero cache size".into()))?;
        Ok(Self {
            header,
            io,
            source_size,
            l1,
            l2_cache: Mutex::new(LruCache::new(entries)),
            decoded_cache: Mutex::new(LruCache::new(decoded)),
            key,
            backing: None,
            backing_size: None,
        })
    }
}
impl<I: ReadAt> Qcow2<I> {
    /// Allocation cluster length.
    pub fn cluster_size(&self) -> u64 {
        self.header.cluster_size()
    }
    /// Logical disk length.
    pub fn guest_size(&self) -> u64 {
        self.header.size
    }
    /// True when logical reads require a legacy encryption key.
    pub fn is_locked(&self) -> bool {
        self.header.encryption == 1 && self.key.is_none()
    }
    /// Read metadata without exposing key material.
    pub fn info(&self) -> ImageInfo {
        ImageInfo {
            version: self.header.version,
            media_size: self.guest_size(),
            cluster_size: self.cluster_size(),
            encryption_method: self.header.encryption,
            backing_filename: self.header.backing_name.clone(),
            backing_format: self.header.backing_format.clone(),
            snapshot_count: self.header.snapshots,
            incompatible_features: self.header.incompatible,
            compatible_features: self.header.compatible,
            autoclear_features: self.header.autoclear,
        }
    }
    /// Attach an explicitly opened logical parent, including RAW. Ownership
    /// prevents dangling handles; attachment is allowed once, before sharing.
    pub fn set_backing(&mut self, backing: Arc<dyn BackingSource>) -> Result<()> {
        if self.header.backing_name.is_none() || self.backing.is_some() {
            return Err(Error::FileFormat(
                "no backing filename or parent already attached".into(),
            ));
        }
        self.backing_size = Some(
            backing
                .size()?
                .ok_or_else(|| Error::FileFormat("unknown backing size".into()))?,
        );
        self.backing = Some(backing);
        Ok(())
    }
    /// Read logical bytes. EOF returns zero; allocated short reads are errors.
    pub fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize> {
        self.read_at_cancellable(position, buffer, &|| false)
    }
    /// Operation-scoped cancellation, checked between clusters and around
    /// decoding. Blocking tasks can pass `|| token.is_cancelled()` directly.
    pub fn read_at_cancellable(
        &self,
        position: u64,
        buffer: &mut [u8],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<usize> {
        self.guest_read(position, buffer, cancelled)
    }
    /// Independent positioned and sequential/seekable cursor.
    pub fn reader(&self) -> Result<Reader<'_, I>> {
        Ok(Reader::new(self))
    }
}
impl<I: ReadAt> ReadAt for Qcow2<I> {
    fn read_at(&self, position: u64, buffer: &mut [u8]) -> std::io::Result<usize> {
        Qcow2::read_at(self, position, buffer).map_err(Into::into)
    }
}
impl<I: ReadAt> Size for Qcow2<I> {
    fn size(&self) -> std::io::Result<Option<u64>> {
        Ok(Some(self.guest_size()))
    }
}
impl<I: ReadAt> fmt::Debug for Qcow2<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Qcow2")
            .field("info", &self.info())
            .finish_non_exhaustive()
    }
}
/// Probe the shared signature, preserving I/O errors other than short inputs.
pub fn check_signature(io: &impl ReadAt) -> std::io::Result<bool> {
    let mut signature = [0; 4];
    match io.read_exact_at(0, &mut signature) {
        Ok(()) => Ok(signature == header::MAGIC),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}
impl Qcow2<std::fs::File> {
    /// Resolve relative RAW/QCOW backing files with cycle/depth checks.
    pub fn open_path(path: impl AsRef<Path>) -> Result<Self> {
        path::open(path.as_ref(), None)
    }
    /// Resolve a chain using this key for encrypted members. For different
    /// parent keys, explicitly open each image and use `set_backing`.
    pub fn open_path_with_key(path: impl AsRef<Path>, key: EncryptionKey) -> Result<Self> {
        path::open(path.as_ref(), Some(key))
    }
}
