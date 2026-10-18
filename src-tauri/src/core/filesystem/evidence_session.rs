//! Per-evidence filesystem state, owned separately from the byte reader.
//!
//! The session holds its image for the lifetime of directory operations and
//! owns lazy filesystem caches. Byte-only consumers borrow `disk_image()`;
//! they do not acquire plugin handles or depend on filesystem implementations.

use std::{
    collections::HashMap,
    ops::Deref,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use super::docker::host_docker::HostDocker;
use super::external_volume::ExternalVolume;
use super::volumes::ntfs::{NtfsDeletedSnapshot, NtfsSource};
use crate::core::evidence::image::{BadRegion, DiskImage};
use crate::core::evidence::partitions::SectorSource;
mod encrypted;
use encrypted::AttachedEncryptedVolume;
mod export;
pub use export::{ImageExportMode, RawExportReader};

#[derive(Debug)]
pub struct EvidenceSession {
    decrypted_partitions: [OnceLock<AttachedEncryptedVolume>; 256],
    indexed_roots: Option<Vec<super::tree::fs_tree::FsNode>>,
    image: Arc<DiskImage>,
    pub(crate) host_docker: OnceLock<Option<Arc<HostDocker>>>,
    pub(crate) host_docker_cache_dir: parking_lot::RwLock<Option<PathBuf>>,
    external_volumes: Mutex<HashMap<String, Arc<dyn ExternalVolume>>>,
    ntfs_deleted: Mutex<HashMap<NtfsSource, Arc<NtfsDeletedSnapshot>>>,
}

impl From<DiskImage> for EvidenceSession {
    fn from(image: DiskImage) -> Self {
        Self::from(Arc::new(image))
    }
}

impl From<Arc<DiskImage>> for EvidenceSession {
    fn from(image: Arc<DiskImage>) -> Self {
        Self {
            decrypted_partitions: std::array::from_fn(|_| OnceLock::new()),
            indexed_roots: None,
            image,
            host_docker: OnceLock::new(),
            host_docker_cache_dir: parking_lot::RwLock::new(None),
            external_volumes: Mutex::new(HashMap::new()),
            ntfs_deleted: Mutex::new(HashMap::new()),
        }
    }
}

impl Deref for EvidenceSession {
    type Target = Arc<DiskImage>;

    fn deref(&self) -> &Self::Target {
        &self.image
    }
}

impl EvidenceSession {
    /// Directory entry points of the verified case indexes in an offline session.
    pub fn from_indexed_image(image: DiskImage, roots: Vec<super::tree::fs_tree::FsNode>) -> Self {
        let mut session = Self::from(image);
        session.indexed_roots = Some(roots);
        session
    }

    pub fn indexed_roots(&self) -> Option<&[super::tree::fs_tree::FsNode]> {
        self.indexed_roots.as_deref()
    }
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        DiskImage::open(path).map(Self::from)
    }

    pub fn open_with_id(id: String, path: impl AsRef<Path>) -> anyhow::Result<Self> {
        DiskImage::open_with_id(id, path).map(Self::from)
    }

    pub fn disk_image(&self) -> &Arc<DiskImage> {
        &self.image
    }

    pub(crate) fn install_external_volume(&self, volume: impl ExternalVolume + 'static) {
        if let Ok(mut volumes) = self.external_volumes.lock() {
            volumes.insert(volume.host_root().to_owned(), Arc::new(volume));
        }
    }

    pub(crate) fn external_volume(&self, node_id: &str) -> Option<Arc<dyn ExternalVolume>> {
        self.external_volumes.lock().ok().and_then(|volumes| {
            volumes
                .values()
                .find(|volume| volume.is_alive() && volume.owns_node(node_id))
                .cloned()
        })
    }

    pub(crate) fn take_external_volumes(&self) -> Vec<Arc<dyn ExternalVolume>> {
        self.external_volumes
            .lock()
            .map(|mut volumes| volumes.drain().map(|(_, volume)| volume).collect())
            .unwrap_or_default()
    }

    pub(crate) fn remove_external_volume(
        &self,
        host_root: &str,
    ) -> Option<Arc<dyn ExternalVolume>> {
        self.external_volumes
            .lock()
            .ok()
            .and_then(|mut volumes| volumes.remove(host_root))
    }

    /// Reuse the immutable deleted-record projection for exactly this source.
    pub fn ntfs_deleted_snapshot(&self, source: NtfsSource) -> Option<Arc<NtfsDeletedSnapshot>> {
        self.ntfs_deleted
            .lock()
            .ok()
            .and_then(|map| map.get(&source).cloned())
    }

    /// Publish a freshly scanned MFT projection without retaining an FS reader.
    pub fn install_ntfs_deleted_snapshot(
        &self,
        source: NtfsSource,
        snapshot: Arc<NtfsDeletedSnapshot>,
    ) {
        if let Ok(mut map) = self.ntfs_deleted.lock() {
            map.insert(source, snapshot);
        }
    }
}

impl SectorSource for EvidenceSession {
    fn read(&self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        self.image.read(offset, length)
    }

    fn size(&self) -> u64 {
        self.image.len()
    }

    fn read_traced(&self, offset: u64, length: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        self.image.read_range_traced(offset, length)
    }
}

#[cfg(test)]
mod tests;
