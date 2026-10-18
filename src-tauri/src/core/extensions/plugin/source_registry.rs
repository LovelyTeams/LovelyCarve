use std::{collections::HashMap, sync::Arc};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::evidence::partitions::SectorSource;

use super::{PluginError, Result};

pub const MAX_SOURCE_READ: usize = 4 * 1024 * 1024;
pub const MAX_SOURCE_READV_RANGES: usize = 128;
// Leave room for the response envelope and up to 128 binary value headers.
pub const MAX_SOURCE_READV_BYTES: usize = super::MAX_FRAME_SIZE - 4096;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SourceHandle {
    pub source_id: String,
    pub size: u64,
    pub sector_size: u32,
    pub read_only: bool,
}

pub type SourceInfo = SourceHandle;

struct RegisteredSource {
    source: SourceReference,
    sector_size: u32,
}

enum SourceReference {
    Borrowed(std::sync::Weak<dyn SectorSource + Send + Sync>),
    Owned(Arc<dyn SectorSource + Send + Sync>),
}

impl SourceReference {
    fn get(&self) -> Option<Arc<dyn SectorSource + Send + Sync>> {
        match self {
            Self::Borrowed(source) => source.upgrade(),
            Self::Owned(source) => Some(Arc::clone(source)),
        }
    }
}

struct SourceSlice {
    parent: std::sync::Weak<dyn SectorSource + Send + Sync>,
    base: u64,
    length: u64,
}

impl SectorSource for SourceSlice {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        let parent = self
            .parent
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("parent source is no longer loaded"))?;
        let offset = offset.min(self.length);
        let available = self.length.saturating_sub(offset);
        let length = len.min(available.try_into().unwrap_or(usize::MAX));
        if length == 0 {
            return Ok(Vec::new());
        }
        let absolute = self
            .base
            .checked_add(offset)
            .ok_or_else(|| anyhow::anyhow!("source slice offset overflow"))?;
        let mut bytes = parent.read(absolute, length)?;
        bytes.truncate(length);
        Ok(bytes)
    }

    fn size(&self) -> u64 {
        self.length
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct MemorySource(Vec<u8>);

    impl SectorSource for MemorySource {
        fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(self.0.len());
            let end = start.saturating_add(len).min(self.0.len());
            Ok(self.0[start..end].to_vec())
        }

        fn size(&self) -> u64 {
            self.0.len() as u64
        }
    }

    #[test]
    fn slice_and_readv_stay_inside_logical_source() {
        let registry = SourceRegistry::default();
        let parent: Arc<dyn SectorSource + Send + Sync> =
            Arc::new(MemorySource(b"0123456789".to_vec()));
        let handle = registry
            .register_slice(Arc::clone(&parent), 2, 5, 512)
            .expect("register slice");
        assert_eq!(
            registry
                .readv(&handle.source_id, &[(0, 2), (4, 10)])
                .expect("read ranges"),
            vec![b"23".to_vec(), b"6".to_vec()]
        );
    }

    #[test]
    fn readv_rejects_excess_total_before_reading() {
        let registry = SourceRegistry::default();
        let parent: Arc<dyn SectorSource + Send + Sync> = Arc::new(MemorySource(vec![0; 16]));
        let handle = registry.register(Arc::clone(&parent), 512);
        assert!(matches!(
            registry.readv(&handle.source_id, &[(0, MAX_SOURCE_READV_BYTES + 1)]),
            Err(PluginError::Rpc { code, .. }) if code == "READ_TOO_LARGE"
        ));
    }

    struct CountingSource {
        reads: Arc<AtomicUsize>,
    }

    impl SectorSource for CountingSource {
        fn read(&self, _offset: u64, _len: usize) -> anyhow::Result<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(Vec::new())
        }

        fn size(&self) -> u64 {
            MAX_SOURCE_READV_BYTES as u64
        }
    }

    #[test]
    fn readv_validates_every_range_before_reading() {
        let registry = SourceRegistry::default();
        let reads = Arc::new(AtomicUsize::new(0));
        let parent: Arc<dyn SectorSource + Send + Sync> = Arc::new(CountingSource {
            reads: Arc::clone(&reads),
        });
        let handle = registry.register(Arc::clone(&parent), 512);
        assert!(matches!(
            registry.readv(&handle.source_id, &[(0, 1), (1, MAX_SOURCE_READ + 1)]),
            Err(PluginError::Rpc { code, .. }) if code == "READ_TOO_LARGE"
        ));
        assert_eq!(reads.load(Ordering::Relaxed), 0);
    }

    struct OversizedReadSource;

    impl SectorSource for OversizedReadSource {
        fn read(&self, _offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
            Ok(vec![0xA5; len.saturating_add(32)])
        }

        fn size(&self) -> u64 {
            64
        }
    }

    #[test]
    fn logical_reads_truncate_buggy_parent_results() {
        let registry = SourceRegistry::default();
        let parent: Arc<dyn SectorSource + Send + Sync> = Arc::new(OversizedReadSource);
        let whole = registry.register(Arc::clone(&parent), 512);
        assert_eq!(
            registry.read(&whole.source_id, 0, 4).expect("read").len(),
            4
        );

        let slice = registry
            .register_slice(Arc::clone(&parent), 8, 8, 512)
            .expect("register slice");
        assert_eq!(
            registry
                .read(&slice.source_id, 6, 8)
                .expect("slice read")
                .len(),
            2
        );
    }

    #[test]
    fn slice_registration_rejects_parent_overflow() {
        let registry = SourceRegistry::default();
        let parent: Arc<dyn SectorSource + Send + Sync> = Arc::new(OversizedReadSource);
        assert!(matches!(
            registry.register_slice(parent, 60, 8, 512),
            Err(PluginError::Rpc { code, .. }) if code == "INVALID_SOURCE_RANGE"
        ));
    }
}

#[derive(Clone, Default)]
pub struct SourceRegistry {
    sources: Arc<RwLock<HashMap<String, RegisteredSource>>>,
}

impl SourceRegistry {
    pub fn register(
        &self,
        source: Arc<dyn SectorSource + Send + Sync>,
        sector_size: u32,
    ) -> SourceHandle {
        let source_id = format!("src_{}", Uuid::new_v4().simple());
        let handle = SourceHandle {
            source_id: source_id.clone(),
            size: source.size(),
            sector_size,
            read_only: true,
        };
        self.sources.write().insert(
            source_id,
            RegisteredSource {
                source: SourceReference::Borrowed(Arc::downgrade(&source)),
                sector_size,
            },
        );
        handle
    }

    pub fn register_slice(
        &self,
        parent: Arc<dyn SectorSource + Send + Sync>,
        base: u64,
        length: u64,
        sector_size: u32,
    ) -> Result<SourceHandle> {
        let parent_size = parent.size();
        let end = base.checked_add(length).ok_or_else(|| PluginError::Rpc {
            code: "INVALID_SOURCE_RANGE".to_owned(),
            message: "logical source slice range overflow".to_owned(),
        })?;
        if end > parent_size {
            return Err(PluginError::Rpc {
                code: "INVALID_SOURCE_RANGE".to_owned(),
                message: format!(
                    "logical source slice {base}..{end} exceeds parent size {parent_size}"
                ),
            });
        }
        let source_id = format!("src_{}", Uuid::new_v4().simple());
        let handle = SourceHandle {
            source_id: source_id.clone(),
            size: length,
            sector_size,
            read_only: true,
        };
        let slice = SourceSlice {
            parent: Arc::downgrade(&parent),
            base,
            length,
        };
        self.sources.write().insert(
            source_id,
            RegisteredSource {
                source: SourceReference::Owned(Arc::new(slice)),
                sector_size,
            },
        );
        Ok(handle)
    }

    pub fn remove(&self, source_id: &str) -> bool {
        self.sources.write().remove(source_id).is_some()
    }

    pub fn info(&self, source_id: &str) -> Result<SourceInfo> {
        let sources = self.sources.read();
        let registered = sources.get(source_id).ok_or_else(|| PluginError::Rpc {
            code: "UNKNOWN_SOURCE".to_owned(),
            message: format!("source {source_id} is not registered"),
        })?;
        let source = registered.source.get().ok_or_else(|| PluginError::Rpc {
            code: "SOURCE_CLOSED".to_owned(),
            message: format!("source {source_id} is no longer loaded"),
        })?;
        Ok(SourceInfo {
            source_id: source_id.to_owned(),
            size: source.size(),
            sector_size: registered.sector_size,
            read_only: true,
        })
    }

    pub fn read(&self, source_id: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        if length > MAX_SOURCE_READ {
            return Err(PluginError::Rpc {
                code: "READ_TOO_LARGE".to_owned(),
                message: format!("source.read length {length} exceeds {MAX_SOURCE_READ}"),
            });
        }
        let source = self
            .sources
            .read()
            .get(source_id)
            .and_then(|registered| registered.source.get())
            .ok_or_else(|| PluginError::Rpc {
                code: "UNKNOWN_SOURCE".to_owned(),
                message: format!("source {source_id} is not registered"),
            })?;
        let available = source.size().saturating_sub(offset);
        let read_len = length.min(available.try_into().unwrap_or(usize::MAX));
        if read_len == 0 {
            return Ok(Vec::new());
        }
        let mut bytes = source
            .read(offset, read_len)
            .map_err(|error| PluginError::Rpc {
                code: "SOURCE_READ_FAILED".to_owned(),
                message: error.to_string(),
            })?;
        bytes.truncate(read_len);
        Ok(bytes)
    }

    pub fn readv(&self, source_id: &str, ranges: &[(u64, usize)]) -> Result<Vec<Vec<u8>>> {
        if ranges.len() > MAX_SOURCE_READV_RANGES {
            return Err(PluginError::Rpc {
                code: "TOO_MANY_RANGES".to_owned(),
                message: format!(
                    "source.readv has {} ranges; maximum is {MAX_SOURCE_READV_RANGES}",
                    ranges.len()
                ),
            });
        }
        if let Some((_, length)) = ranges.iter().find(|(_, length)| *length > MAX_SOURCE_READ) {
            return Err(PluginError::Rpc {
                code: "READ_TOO_LARGE".to_owned(),
                message: format!("source.readv range length {length} exceeds {MAX_SOURCE_READ}"),
            });
        }
        let total = ranges.iter().try_fold(0_usize, |total, (_, length)| {
            total.checked_add(*length).ok_or_else(|| PluginError::Rpc {
                code: "READ_TOO_LARGE".to_owned(),
                message: "source.readv total length overflow".to_owned(),
            })
        })?;
        if total > MAX_SOURCE_READV_BYTES {
            return Err(PluginError::Rpc {
                code: "READ_TOO_LARGE".to_owned(),
                message: format!(
                    "source.readv total length {total} exceeds {MAX_SOURCE_READV_BYTES}"
                ),
            });
        }
        ranges
            .iter()
            .map(|(offset, length)| self.read(source_id, *offset, *length))
            .collect()
    }
}

#[cfg(test)]
mod frame_budget_tests {
    use super::*;
    use crate::core::extensions::plugin::LppMessage;
    use rmpv::Value;

    struct ZeroSource;
    impl SectorSource for ZeroSource {
        fn size(&self) -> u64 {
            16 * 1024 * 1024
        }
        fn read(&self, _offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
            Ok(vec![0; length])
        }
    }

    #[test]
    fn largest_readv_response_fits_and_eight_mib_is_rejected_before_reading() {
        let registry = SourceRegistry::default();
        let source: Arc<dyn SectorSource + Send + Sync> = Arc::new(ZeroSource);
        let handle = registry.register(Arc::clone(&source), 512);
        let mut ranges =
            vec![(0, MAX_SOURCE_READV_BYTES / MAX_SOURCE_READV_RANGES); MAX_SOURCE_READV_RANGES];
        ranges[0].1 += MAX_SOURCE_READV_BYTES % MAX_SOURCE_READV_RANGES;
        let chunks = registry
            .readv(&handle.source_id, &ranges)
            .expect("largest valid request");
        let response = LppMessage::success(
            u64::MAX,
            Value::Array(chunks.into_iter().map(Value::Binary).collect()),
        );
        lovelycarve_plugin::wire::encode_frame(&response)
            .expect("valid request always fits its reply frame");
        assert!(registry
            .readv(
                &handle.source_id,
                &[(0, MAX_SOURCE_READ), (0, MAX_SOURCE_READ)]
            )
            .is_err());
    }
}
