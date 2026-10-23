//! ROM / flash / optical / Minix / classic HFS adapters over the lamfold contract.
//! Paths retain their volume identity; no host extraction or symlink following.

use std::{cell::RefCell, collections::HashSet, sync::Arc};

use anyhow::{ensure, Context, Result};
use lamfold::{
    BlockCache, BlockSource, FileKind, FoldError, FoldFrontend, NoVerifier, SubstrateCtx,
};

use crate::core::evidence::image::{find_partition, DiskImage, Partition};
use crate::core::filesystem::fs_dispatch::build_stat;
use crate::core::filesystem::tree::fs_tree::{FsNode, FsStat, Mactb, NodeKind};

#[path = "classic_hfs_streams.rs"]
mod classic_hfs_streams;
#[cfg(test)]
#[path = "classic_hfs_tests.rs"]
mod classic_hfs_tests;

#[cfg(test)]
#[path = "flashfs_tests.rs"]
mod flashfs_tests;
#[cfg(test)]
#[path = "minix_tests.rs"]
mod minix_tests;
#[cfg(test)]
#[path = "rom_images_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "udf_tests.rs"]
mod udf_tests;

pub type CramfsVolume = RomVolume<lamfold_cramfs::Cramfs<RomImageSource>>;
pub type RomfsVolume = RomVolume<lamfold_romfs::Romfs<RomImageSource>>;
pub type ErofsVolume = RomVolume<lamfold_erofs::Erofs<RomImageSource>>;
pub type F2fsVolume = RomVolume<f2fs_reader::F2fs<RomImageSource>>;
pub type Qnx4Volume = RomVolume<qnx_reader::Qnx4<RomImageSource>>;
pub type Qnx6Volume = RomVolume<qnx_reader::Qnx6<RomImageSource>>;
pub type UdfVolume = RomVolume<lamfold_udf::Udf<RomImageSource>>;
pub type MinixVolume = RomVolume<minix_reader::Minix<RomImageSource>>;
pub type HfsVolume = RomVolume<classic_hfs::Hfs<RomImageSource>>;

pub struct RomImageSource {
    image: Arc<DiskImage>,
    base: u64,
    length: u64,
}

impl BlockSource for RomImageSource {
    fn len(&self) -> u64 {
        self.length
    }

    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> lamfold::Result<()> {
        if offset
            .checked_add(buffer.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(FoldError::Io("ROM image read outside partition"));
        }
        let absolute = self
            .base
            .checked_add(offset)
            .ok_or(FoldError::Io("ROM image offset overflow"))?;
        let bytes = (|| -> Result<Vec<u8>> {
            if let Some(firmware) = &self.image.firmware {
                if let Some(bytes) = firmware.read_plaintext(absolute, buffer.len())? {
                    return Ok(bytes);
                }
            }
            self.image.read_range(absolute, buffer.len())
        })()
        .map_err(|error| FoldError::Source(format!("ROM image at {absolute:#x}: {error:#}")))?;
        if bytes.len() != buffer.len() {
            return Err(FoldError::Io("ROM image short source read"));
        }
        buffer.copy_from_slice(&bytes);
        Ok(())
    }
}

pub struct RomVolume<F> {
    partition_id: u8,
    reader: RefCell<F>,
}

pub fn parse_cramfs_id(id: &str) -> Option<(u8, String)> {
    parse_id(id, "cramfs")
}
pub fn parse_romfs_id(id: &str) -> Option<(u8, String)> {
    parse_id(id, "romfs")
}
pub fn parse_erofs_id(id: &str) -> Option<(u8, String)> {
    parse_id(id, "erofs")
}
pub fn parse_qnx4_id(id: &str) -> Option<(u8, String)> {
    parse_id(id, "qnx4")
}
pub fn open_qnx4_source(image: &Arc<DiskImage>, partition: u8) -> Result<Qnx4Volume> {
    Qnx4Volume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}
pub fn parse_minix_id(id: &str) -> Option<(u8, String)> {
    parse_id(id, "minix")
}
pub fn parse_hfs_id(id: &str) -> Option<(u8, String)> {
    parse_id(id, "hfs")
}
pub fn open_hfs_source(image: &Arc<DiskImage>, partition: u8) -> Result<HfsVolume> {
    HfsVolume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}
pub fn open_minix_source(image: &Arc<DiskImage>, partition: u8) -> Result<MinixVolume> {
    MinixVolume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}
pub fn parse_udf_id(id: &str) -> Option<(u8, String)> {
    parse_id(id, "udf")
}
pub fn open_udf_source(image: &Arc<DiskImage>, partition: u8) -> Result<UdfVolume> {
    UdfVolume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}
pub fn parse_qnx6_id(id: &str) -> Option<(u8, String)> {
    parse_id(id, "qnx6")
}
pub fn open_qnx6_source(image: &Arc<DiskImage>, partition: u8) -> Result<Qnx6Volume> {
    Qnx6Volume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}
pub fn parse_f2fs_id(id: &str) -> Option<(u8, String)> {
    parse_id(id, "f2fs")
}
pub fn open_erofs_source(image: &Arc<DiskImage>, partition: u8) -> Result<ErofsVolume> {
    ErofsVolume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}
pub fn open_f2fs_source(image: &Arc<DiskImage>, partition: u8) -> Result<F2fsVolume> {
    F2fsVolume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}

fn parse_id(id: &str, prefix: &str) -> Option<(u8, String)> {
    let rest = id.strip_prefix(prefix)?.strip_prefix(":part:")?;
    let (partition, path) = rest.split_once(':')?;
    let path = String::from_utf8(hex::decode(path).ok()?).ok()?;
    valid_path(&path).then_some((partition.parse().ok()?, path))
}

fn valid_path(path: &str) -> bool {
    path.is_empty()
        || (path.starts_with('/')
            && !path.contains(['\\', '\0'])
            && path
                .split('/')
                .skip(1)
                .all(|component| !component.is_empty() && component != "." && component != ".."))
}

pub fn open_cramfs_source(image: &Arc<DiskImage>, partition: u8) -> Result<CramfsVolume> {
    CramfsVolume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}

pub fn open_romfs_source(image: &Arc<DiskImage>, partition: u8) -> Result<RomfsVolume> {
    RomfsVolume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}

impl<F: FoldFrontend<RomImageSource>> RomVolume<F> {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let base = partition
            .start_lba
            .checked_mul(image.sector_size as u64)
            .context("ROM image base overflow")?;
        let length = partition
            .length_lba
            .checked_mul(image.sector_size as u64)
            .context("ROM image length overflow")?;
        ensure!(
            base.checked_add(length)
                .is_some_and(|end| end <= image.len()),
            "ROM image partition exceeds source"
        );
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let reader = F::open(
            RomImageSource {
                image,
                base,
                length,
            },
            &mut context,
        )
        .with_context(|| format!("{} open part:{}", F::TAG, partition.id))?;
        Ok(Self {
            partition_id: partition.id,
            reader: RefCell::new(reader),
        })
    }

    fn node_id(&self, path: &str) -> String {
        format!(
            "{}:part:{}:{}",
            F::TAG,
            self.partition_id,
            hex::encode(path)
        )
    }

    fn resolve(
        reader: &mut F,
        path: &str,
        context: &mut SubstrateCtx<'_>,
    ) -> Result<(u64, HashSet<u64>)> {
        ensure!(valid_path(path), "invalid ROM image path");
        ensure!(path.split('/').count() <= 257, "ROM image path depth limit");
        let mut node = reader.root();
        let mut ancestors = HashSet::from([node]);
        for component in path.split('/').filter(|component| !component.is_empty()) {
            node = reader
                .lookup(node, component, context)?
                .context("ROM image entry not found")?;
            let metadata = reader.metadata(node, context)?;
            if metadata.kind == FileKind::Directory {
                ensure!(ancestors.insert(node), "ROM image directory ancestry cycle");
            }
        }
        Ok((node, ancestors))
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let mut reader = self.reader.borrow_mut();
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let (directory, ancestors) = Self::resolve(&mut reader, path, &mut context)?;
        let mut nodes = Vec::new();
        for entry in reader.read_dir(directory, &mut context)? {
            let metadata = reader.metadata(entry.node, &mut context)?;
            ensure!(
                metadata.kind != FileKind::Directory || !ancestors.contains(&entry.node),
                "ROM image directory ancestry cycle"
            );
            let child_path = format!("{path}/{}", entry.name);
            nodes.push(FsNode {
                id: self.node_id(&child_path),
                name: entry.name,
                parent_id: Some(self.node_id(path)),
                kind: match metadata.kind {
                    FileKind::Directory => NodeKind::Dir,
                    FileKind::Symlink => NodeKind::Symlink,
                    FileKind::Regular | FileKind::Other => NodeKind::File,
                },
                size: if matches!(metadata.kind, FileKind::Regular | FileKind::Symlink) {
                    metadata.size
                } else {
                    0
                },
                deleted: false,
                inode: Some(entry.node),
                timestamps: {
                    let times = reader.timestamps(entry.node, &mut context)?;
                    let local = reader.local_timestamps(entry.node, &mut context)?;
                    let format = |value: Option<(i64, u32)>,
                                  local: Option<(i64, u32)>|
                     -> Result<Option<String>> {
                        ensure!(
                            value.is_none() || local.is_none(),
                            "conflicting UTC/local filesystem timestamps"
                        );
                        value
                            .or(local)
                            .map(|(seconds, nanos)| {
                                chrono::DateTime::from_timestamp(seconds, nanos)
                                    .map(|time| {
                                        if local.is_some() {
                                            time.naive_utc()
                                                .format("%Y-%m-%dT%H:%M:%S%.f")
                                                .to_string()
                                        } else {
                                            time.to_rfc3339_opts(
                                                chrono::SecondsFormat::AutoSi,
                                                true,
                                            )
                                        }
                                    })
                                    .context("filesystem timestamp out of range")
                            })
                            .transpose()
                    };
                    Mactb {
                        modified: format(times.modified, local.modified)?,
                        accessed: format(times.accessed, local.accessed)?,
                        created: format(times.created, local.created)?,
                        mft_changed: format(times.changed, local.changed)?,
                        birth: None,
                    }
                },
                link_target: reader
                    .read_link(entry.node, &mut context)?
                    .map(String::from_utf8)
                    .transpose()?,
                has_snapshot: false,
            });
        }
        nodes.sort_by(|a, b| {
            (a.kind != NodeKind::Dir)
                .cmp(&(b.kind != NodeKind::Dir))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(nodes)
    }

    pub fn read_file(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        self.read_file_range(path, 0, max_bytes)
    }

    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        let mut reader = self.reader.borrow_mut();
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let (node, _) = Self::resolve(&mut reader, path, &mut context)?;
        let metadata = reader.metadata(node, &mut context)?;
        ensure!(
            matches!(metadata.kind, FileKind::Regular | FileKind::Symlink),
            "ROM image entry has no file content"
        );
        let count = lamfold::checked_full_read_len(
            metadata.size.saturating_sub(offset).min(length as u64),
        )?;
        let mut bytes = vec![0; count];
        let actual = reader.read_at(node, offset, &mut bytes, &mut context)?;
        ensure!(actual == count, "ROM image short logical file read");
        Ok(bytes)
    }

    pub fn stream_file(
        &self,
        path: &str,
        chunk_size: usize,
        mut visitor: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        let length = self.file_stat(path, &self.node_id(path))?.size;
        let mut offset = 0;
        while offset < length {
            let count = (length - offset).min(chunk_size.clamp(4096, 1024 * 1024) as u64) as usize;
            let bytes = self.read_file_range(path, offset, count)?;
            ensure!(bytes.len() == count, "ROM image short file stream");
            visitor(&bytes)?;
            offset += count as u64;
        }
        Ok(offset)
    }

    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        let (node, metadata) = {
            let mut reader = self.reader.borrow_mut();
            let mut cache = BlockCache::new(0);
            let mut context = SubstrateCtx {
                cache: &mut cache,
                verifier: &NoVerifier,
            };
            let (node, _) = Self::resolve(&mut reader, path, &mut context)?;
            (node, reader.metadata(node, &mut context)?)
        };
        let sample = self.read_file(path, 4096)?;
        Ok(build_stat(
            node_id,
            metadata.size,
            Some(node),
            Some(metadata.mode as u16),
            &sample,
            Vec::new(),
        ))
    }
}
