//! UBI container navigation and raw logical-volume reads. Nested SquashFS
//! readers share the same bounded source and retain the UBI volume identity.
use std::sync::Arc;

use anyhow::{bail, ensure, Context, Result};
use ubidump::{ReadAt, UbiImage, UbiReader};

use crate::core::evidence::image::{find_partition, DiskImage, Partition, PartitionKind};
use crate::core::filesystem::fs_dispatch::build_stat;
use crate::core::filesystem::tree::fs_tree::{FsNode, FsStat, Mactb, NodeKind};
use crate::core::filesystem::volumes::squashfs::SquashfsSource;

#[cfg(test)]
#[path = "ubi_tests.rs"]
mod tests;

#[derive(Clone)]
pub struct UbiSource {
    image: Arc<DiskImage>,
    base: u64,
    length: u64,
}

impl ReadAt for UbiSource {
    fn size(&self) -> u64 {
        self.length
    }

    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        ensure!(
            offset
                .checked_add(length as u64)
                .is_some_and(|end| end <= self.length),
            "UBI read outside partition"
        );
        let bytes = self.image.read_range(
            self.base
                .checked_add(offset)
                .context("UBI source offset overflow")?,
            length,
        )?;
        ensure!(bytes.len() == length, "UBI partition short read");
        Ok(bytes)
    }
}

pub struct UbiVolume {
    partition_id: u8,
    source: UbiSource,
    layout: UbiImage,
}

pub fn parse_ubi_id(id: &str) -> Option<(u8, u32)> {
    let mut fields = id.strip_prefix("ubi:part:")?.split(':');
    let part_id = fields.next()?.parse().ok()?;
    let volume_id = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some((part_id, volume_id))
}

pub fn open_ubi_source(image: &Arc<DiskImage>, partition_id: u8) -> Result<UbiVolume> {
    UbiVolume::open(
        Arc::clone(image),
        find_partition(image, &partition_id.to_string())?,
    )
}

impl UbiVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        ensure!(partition.kind == PartitionKind::Ubi, "partition is not UBI");
        let base = partition
            .start_lba
            .checked_mul(image.sector_size as u64)
            .context("UBI base overflow")?;
        let length = partition
            .length_lba
            .checked_mul(image.sector_size as u64)
            .context("UBI length overflow")?;
        ensure!(
            base.checked_add(length)
                .is_some_and(|end| end <= image.len()),
            "UBI partition exceeds source"
        );
        let source = UbiSource {
            image,
            base,
            length,
        };
        let layout = UbiImage::open(&source)
            .with_context(|| format!("opening UBI part:{}", partition.id))?;
        Ok(Self {
            partition_id: partition.id,
            source,
            layout,
        })
    }

    pub fn reader(&self, volume_id: u32) -> Result<UbiReader<UbiSource>> {
        let volume = self
            .layout
            .volumes
            .get(&volume_id)
            .context("UBI volume not found")?;
        Ok(UbiReader::new(self.source.clone(), Arc::clone(volume)))
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        let mut nodes = Vec::new();
        for volume in self.layout.volumes.values() {
            let prefix = volume.read_at(&self.source, 0, 4)?;
            let squashfs = crate::core::evidence::partitions::squashfs::is_squashfs_magic(&prefix);
            nodes.push(FsNode {
                id: format!("ubi:part:{}:{}", self.partition_id, volume.id),
                name: format!("{}.img", volume.name),
                parent_id: Some(format!("part:{}", self.partition_id)),
                kind: NodeKind::File,
                size: volume.length,
                deleted: false,
                inode: None,
                timestamps: Mactb::default(),
                link_target: None,
                has_snapshot: false,
            });
            if squashfs {
                let source = SquashfsSource::Ubi {
                    part_id: self.partition_id,
                    volume_id: volume.id,
                };
                nodes.push(FsNode {
                    id: source.node_id(""),
                    name: volume.name.clone(),
                    parent_id: Some(format!("part:{}", self.partition_id)),
                    kind: NodeKind::Dir,
                    size: 0,
                    deleted: false,
                    inode: None,
                    timestamps: Mactb::default(),
                    link_target: None,
                    has_snapshot: false,
                });
            }
        }
        Ok(nodes)
    }

    pub fn children(&self, _volume_id: &u32) -> Result<Vec<FsNode>> {
        bail!("UBI raw logical volume is a file")
    }

    pub fn read_file(&self, volume_id: &u32, max_bytes: usize) -> Result<Vec<u8>> {
        self.read_file_range(volume_id, 0, max_bytes)
    }

    pub fn read_file_range(&self, volume_id: &u32, offset: u64, length: usize) -> Result<Vec<u8>> {
        let volume = self
            .layout
            .volumes
            .get(volume_id)
            .context("UBI volume not found")?;
        volume.read_at(&self.source, offset, length)
    }

    pub fn stream_file(
        &self,
        volume_id: &u32,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        let length = self
            .layout
            .volumes
            .get(volume_id)
            .context("UBI volume not found")?
            .length;
        let mut offset = 0;
        while offset < length {
            let count =
                ((length - offset).min(chunk_size.clamp(4096, 1024 * 1024) as u64)) as usize;
            let bytes = self.read_file_range(volume_id, offset, count)?;
            ensure!(bytes.len() == count, "UBI volume short stream");
            visitor(&bytes)?;
            offset += count as u64;
        }
        Ok(offset)
    }

    pub fn file_stat(&self, volume_id: &u32, node_id: &str) -> Result<FsStat> {
        let volume = self
            .layout
            .volumes
            .get(volume_id)
            .context("UBI volume not found")?;
        let sample = volume.read_at(&self.source, 0, 4096)?;
        Ok(build_stat(
            node_id,
            volume.length,
            None,
            None,
            &sample,
            Vec::new(),
        ))
    }
}
