use std::{
    collections::HashMap,
    fs,
    io::{BufWriter, Write as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use parking_lot::Mutex;
use serde::Serialize;

use crate::core::{filesystem::evidence_session::EvidenceSession, filesystem::tree::fs_tree};

use super::{PluginError, Result};

pub(super) const MAX_FILE_READ: usize = 4 * 1024 * 1024;
pub(super) const MAX_FILE_READV_RANGES: usize = 64;
pub(super) const MAX_FILE_READV_TOTAL: usize = super::source_registry::MAX_SOURCE_READV_BYTES;
const MAX_MATERIALIZED_FILE: u64 = 4 * 1024 * 1024 * 1024;
const MAX_PLUGIN_MATERIALIZED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_PLUGIN_MATERIALIZED_LEASES: usize = 16;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FileHandle {
    pub file_id: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaterializedFileLease {
    pub lease_id: String,
    pub path: String,
    pub size: u64,
}

#[derive(Clone)]
struct FileEntry {
    owner_plugin_id: String,
    image_id: String,
    image: Arc<EvidenceSession>,
    node_id: String,
    handle: FileHandle,
}

struct LeaseEntry {
    owner_plugin_id: String,
    image_id: String,
    file_id: String,
    directory: PathBuf,
    size: u64,
}

#[derive(Default)]
struct PluginUsage {
    bytes: u64,
    leases: usize,
}

struct FileRegistryInner {
    files: Mutex<HashMap<String, FileEntry>>,
    leases: Mutex<HashMap<String, LeaseEntry>>,
    usage: Mutex<HashMap<String, PluginUsage>>,
    materialize_root: PathBuf,
}

impl Drop for FileRegistryInner {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.materialize_root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(
                    path = %self.materialize_root.display(),
                    "plugin materialization cleanup failed: {error}"
                );
            }
        }
    }
}

#[derive(Clone)]
pub struct FileRegistry {
    inner: Arc<FileRegistryInner>,
}

impl Default for FileRegistry {
    fn default() -> Self {
        let materialize_root = std::env::temp_dir().join(format!(
            "lovelycarve-plugins-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        Self {
            inner: Arc::new(FileRegistryInner {
                files: Mutex::new(HashMap::new()),
                leases: Mutex::new(HashMap::new()),
                usage: Mutex::new(HashMap::new()),
                materialize_root,
            }),
        }
    }
}

impl FileRegistry {
    pub fn register(
        &self,
        owner_plugin_id: &str,
        image_id: String,
        image: Arc<EvidenceSession>,
        node_id: String,
        name: String,
    ) -> Result<FileHandle> {
        let stat = fs_tree::file_stat(&image, &node_id).map_err(|error| {
            tracing::debug!(%owner_plugin_id, %image_id, %node_id, "plugin file stat failed: {error}");
            PluginError::Rpc {
                code: "FILE_UNAVAILABLE".to_owned(),
                message: "the selected logical file is unavailable".to_owned(),
            }
        })?;
        let file_id = format!("file_{}", uuid::Uuid::new_v4().simple());
        let handle = FileHandle {
            file_id: file_id.clone(),
            name: safe_file_name(&name),
            size: stat.size,
            mime: stat.mime,
            read_only: true,
        };
        self.inner.files.lock().insert(
            file_id,
            FileEntry {
                owner_plugin_id: owner_plugin_id.to_owned(),
                image_id,
                image,
                node_id,
                handle: handle.clone(),
            },
        );
        Ok(handle)
    }

    pub fn info(&self, owner_plugin_id: &str, file_id: &str) -> Result<FileHandle> {
        Ok(self.entry(owner_plugin_id, file_id)?.handle)
    }

    pub fn read(
        &self,
        owner_plugin_id: &str,
        file_id: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        if length > MAX_FILE_READ {
            return Err(PluginError::ResourceLimit(format!(
                "file.read length {length} exceeds {MAX_FILE_READ}"
            )));
        }
        let entry = self.entry(owner_plugin_id, file_id)?;
        if offset >= entry.handle.size || length == 0 {
            return Ok(Vec::new());
        }
        let available = entry.handle.size - offset;
        let length = length.min(usize::try_from(available).unwrap_or(usize::MAX));
        let mut bytes = fs_tree::read_file_range(&entry.image, &entry.node_id, offset, length)
            .map_err(|error| {
                tracing::debug!(%owner_plugin_id, %file_id, "plugin file read failed: {error}");
                PluginError::Rpc {
                    code: "FILE_READ_FAILED".to_owned(),
                    message: "logical file read failed".to_owned(),
                }
            })?;
        bytes.truncate(length);
        Ok(bytes)
    }

    pub fn readv(
        &self,
        owner_plugin_id: &str,
        file_id: &str,
        ranges: &[(u64, usize)],
    ) -> Result<Vec<Vec<u8>>> {
        if ranges.len() > MAX_FILE_READV_RANGES {
            return Err(PluginError::ResourceLimit(format!(
                "file.readv has more than {MAX_FILE_READV_RANGES} ranges"
            )));
        }
        let total = ranges.iter().try_fold(0_usize, |total, (_, length)| {
            total.checked_add(*length).ok_or_else(|| {
                PluginError::ResourceLimit("file.readv total length overflowed".to_owned())
            })
        })?;
        if total > MAX_FILE_READV_TOTAL {
            return Err(PluginError::ResourceLimit(format!(
                "file.readv total length {total} exceeds {MAX_FILE_READV_TOTAL}"
            )));
        }
        ranges
            .iter()
            .map(|(offset, length)| self.read(owner_plugin_id, file_id, *offset, *length))
            .collect()
    }

    pub fn materialize(
        &self,
        owner_plugin_id: &str,
        file_id: &str,
    ) -> Result<MaterializedFileLease> {
        let entry = self.entry(owner_plugin_id, file_id)?;
        if entry.handle.size > MAX_MATERIALIZED_FILE {
            return Err(PluginError::ResourceLimit(format!(
                "file is larger than the {} GiB materialization limit",
                MAX_MATERIALIZED_FILE / (1024 * 1024 * 1024)
            )));
        }
        self.reserve_lease(owner_plugin_id, entry.handle.size)?;
        match self.materialize_reserved(&entry) {
            Ok(lease) => Ok(lease),
            Err(error) => {
                self.release_usage(owner_plugin_id, entry.handle.size);
                Err(error)
            }
        }
    }

    pub fn release(&self, owner_plugin_id: &str, lease_id: &str) -> Result<()> {
        let lease = {
            let mut leases = self.inner.leases.lock();
            let Some(lease) = leases.get(lease_id) else {
                return Err(invalid_file_handle());
            };
            if lease.owner_plugin_id != owner_plugin_id {
                return Err(invalid_file_handle());
            }
            leases.remove(lease_id).ok_or_else(invalid_file_handle)?
        };
        self.remove_lease_directory(&lease.directory);
        self.release_usage(owner_plugin_id, lease.size);
        Ok(())
    }

    pub fn remove_file(&self, owner_plugin_id: &str, file_id: &str) {
        let mut files = self.inner.files.lock();
        if files
            .get(file_id)
            .is_some_and(|entry| entry.owner_plugin_id == owner_plugin_id)
        {
            files.remove(file_id);
        }
    }

    pub fn cleanup_file(&self, owner_plugin_id: &str, file_id: &str) {
        self.remove_file(owner_plugin_id, file_id);
        self.cleanup_leases(|lease| {
            lease.owner_plugin_id == owner_plugin_id && lease.file_id == file_id
        });
    }

    pub fn cleanup_plugin(&self, owner_plugin_id: &str) {
        self.inner
            .files
            .lock()
            .retain(|_, entry| entry.owner_plugin_id != owner_plugin_id);
        self.cleanup_leases(|lease| lease.owner_plugin_id == owner_plugin_id);
    }

    pub fn cleanup_image(&self, image_id: &str) {
        self.inner
            .files
            .lock()
            .retain(|_, entry| entry.image_id != image_id);
        self.cleanup_leases(|lease| lease.image_id == image_id);
    }

    fn entry(&self, owner_plugin_id: &str, file_id: &str) -> Result<FileEntry> {
        let entry = self
            .inner
            .files
            .lock()
            .get(file_id)
            .filter(|entry| entry.owner_plugin_id == owner_plugin_id)
            .cloned()
            .ok_or_else(invalid_file_handle)?;
        Ok(entry)
    }

    fn reserve_lease(&self, owner_plugin_id: &str, size: u64) -> Result<()> {
        let mut usage = self.inner.usage.lock();
        let usage = usage.entry(owner_plugin_id.to_owned()).or_default();
        let next_bytes = usage.bytes.checked_add(size).ok_or_else(|| {
            PluginError::ResourceLimit("materialization quota overflowed".to_owned())
        })?;
        if usage.leases >= MAX_PLUGIN_MATERIALIZED_LEASES
            || next_bytes > MAX_PLUGIN_MATERIALIZED_BYTES
        {
            return Err(PluginError::ResourceLimit(
                "plugin materialization quota exceeded".to_owned(),
            ));
        }
        usage.leases += 1;
        usage.bytes = next_bytes;
        Ok(())
    }

    fn materialize_reserved(&self, entry: &FileEntry) -> Result<MaterializedFileLease> {
        let lease_id = format!("lease_{}", uuid::Uuid::new_v4().simple());
        let directory = self.inner.materialize_root.join(&lease_id);
        fs::create_dir_all(&directory)?;
        let path = directory.join(&entry.handle.name);
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut writer = BufWriter::new(file);
        let mut written = 0_u64;
        let stream_result =
            fs_tree::stream_file_chunks(&entry.image, &entry.node_id, 1024 * 1024, |chunk| {
                written = written
                    .checked_add(chunk.len() as u64)
                    .ok_or_else(|| anyhow::anyhow!("materialized size overflowed"))?;
                if written > entry.handle.size || written > MAX_MATERIALIZED_FILE {
                    anyhow::bail!("logical file exceeded its declared size");
                }
                writer.write_all(chunk)?;
                Ok(())
            });
        let materialize_result = match stream_result {
            Ok(_) => writer.flush().map_err(anyhow::Error::from),
            Err(error) => Err(error),
        };
        drop(writer);
        if let Err(error) = materialize_result {
            self.remove_lease_directory(&directory);
            tracing::debug!(file_id = %entry.handle.file_id, "plugin materialization failed: {error}");
            return Err(PluginError::Rpc {
                code: "FILE_MATERIALIZE_FAILED".to_owned(),
                message: "logical file materialization failed".to_owned(),
            });
        }
        if written < entry.handle.size {
            self.reduce_reserved_bytes(&entry.owner_plugin_id, entry.handle.size - written);
        }
        self.inner.leases.lock().insert(
            lease_id.clone(),
            LeaseEntry {
                owner_plugin_id: entry.owner_plugin_id.clone(),
                image_id: entry.image_id.clone(),
                file_id: entry.handle.file_id.clone(),
                directory,
                size: written,
            },
        );
        Ok(MaterializedFileLease {
            lease_id,
            path: path.to_string_lossy().into_owned(),
            size: written,
        })
    }

    fn release_usage(&self, owner_plugin_id: &str, size: u64) {
        let mut usage_by_plugin = self.inner.usage.lock();
        if let Some(usage) = usage_by_plugin.get_mut(owner_plugin_id) {
            usage.bytes = usage.bytes.saturating_sub(size);
            usage.leases = usage.leases.saturating_sub(1);
            if usage.bytes == 0 && usage.leases == 0 {
                usage_by_plugin.remove(owner_plugin_id);
            }
        }
    }

    fn reduce_reserved_bytes(&self, owner_plugin_id: &str, size: u64) {
        if let Some(usage) = self.inner.usage.lock().get_mut(owner_plugin_id) {
            usage.bytes = usage.bytes.saturating_sub(size);
        }
    }

    fn cleanup_leases(&self, predicate: impl Fn(&LeaseEntry) -> bool) {
        let removed = {
            let mut leases = self.inner.leases.lock();
            let ids = leases
                .iter()
                .filter_map(|(lease_id, lease)| predicate(lease).then_some(lease_id.clone()))
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|lease_id| leases.remove(&lease_id))
                .collect::<Vec<_>>()
        };
        for lease in removed {
            self.remove_lease_directory(&lease.directory);
            self.release_usage(&lease.owner_plugin_id, lease.size);
        }
    }

    fn remove_lease_directory(&self, directory: &Path) {
        if !directory.starts_with(&self.inner.materialize_root) {
            tracing::warn!(path = %directory.display(), "refused plugin lease cleanup outside root");
            return;
        }
        if let Err(error) = fs::remove_dir_all(directory) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(path = %directory.display(), "plugin lease cleanup failed: {error}");
            }
        }
    }
}

fn safe_file_name(name: &str) -> String {
    let cleaned = name
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
        .take(120)
        .collect::<String>();
    let cleaned = cleaned.trim_matches([' ', '.']);
    if cleaned.is_empty() {
        return "evidence.bin".to_owned();
    }
    let stem = cleaned.split('.').next().unwrap_or(cleaned);
    if crate::core::filesystem::tree::fs_tree_export::is_windows_device_name(stem) {
        format!("_{cleaned}")
    } else {
        cleaned.to_owned()
    }
}

fn invalid_file_handle() -> PluginError {
    PluginError::Rpc {
        code: "INVALID_FILE_HANDLE".to_owned(),
        message: "file handle or lease is not available to this plugin".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_file() -> (tempfile::TempDir, Arc<EvidenceSession>, String) {
        let temporary = tempfile::tempdir().expect("temp directory");
        let path = temporary.path().join("image.raw");
        fs::write(&path, b"abcdef").expect("raw image");
        let image = Arc::new(EvidenceSession::open(&path).expect("open image"));
        (temporary, image, "diskgap:0x0:0x6".to_owned())
    }

    #[test]
    fn file_handles_are_owner_scoped_and_range_bounded() {
        let (_temporary, image, node_id) = test_file();
        let registry = FileRegistry::default();
        let handle = registry
            .register(
                "org.test.a",
                "image".to_owned(),
                image,
                node_id,
                "a?.bin".to_owned(),
            )
            .expect("register");
        assert_eq!(handle.name, "a.bin");
        assert_eq!(
            registry
                .read("org.test.a", &handle.file_id, 2, 20)
                .expect("read"),
            b"cdef"
        );
        assert!(registry.info("org.test.b", &handle.file_id).is_err());
    }

    #[test]
    fn materialization_is_leased_and_cleaned() {
        let (_temporary, image, node_id) = test_file();
        let registry = FileRegistry::default();
        let handle = registry
            .register(
                "org.test",
                "image".to_owned(),
                image,
                node_id,
                "file.bin".to_owned(),
            )
            .expect("register");
        let lease = registry
            .materialize("org.test", &handle.file_id)
            .expect("materialize");
        assert_eq!(fs::read(&lease.path).expect("lease bytes"), b"abcdef");
        registry
            .release("org.test", &lease.lease_id)
            .expect("release");
        assert!(!Path::new(&lease.path).exists());
    }
}
