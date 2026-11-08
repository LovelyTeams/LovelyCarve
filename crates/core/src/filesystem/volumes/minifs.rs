//! MINIFS adapter; source identity and compressed extents remain unchanged.
use crate::evidence::image::{find_partition, DiskImage, Partition};
use crate::filesystem::{
    fs_dispatch::build_stat,
    tree::fs_tree::{FsNode, FsStat, Mactb, NodeKind},
};
use anyhow::{ensure, Context, Result};
use std::{cell::RefCell, io, sync::Arc};

#[cfg(test)]
#[path = "minifs_tests.rs"]
mod tests;

struct Source {
    image: Arc<DiskImage>,
    base: u64,
    length: u64,
}

impl minifs::ReadAt for Source {
    fn len(&self) -> u64 {
        self.length
    }
    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        (|| -> Result<()> {
            ensure!(
                offset
                    .checked_add(buffer.len() as u64)
                    .is_some_and(|end| end <= self.length),
                "MINIFS read outside partition"
            );
            let absolute = self
                .base
                .checked_add(offset)
                .context("MINIFS source offset overflow")?;
            let bytes = if let Some(bytes) = self
                .image
                .firmware
                .as_ref()
                .map(|firmware| firmware.read_plaintext(absolute, buffer.len()))
                .transpose()?
                .flatten()
            {
                bytes
            } else {
                self.image.read_range(absolute, buffer.len())?
            };
            ensure!(bytes.len() == buffer.len(), "MINIFS short source read");
            buffer.copy_from_slice(&bytes);
            Ok(())
        })()
        .map_err(io::Error::other)
    }
}

pub struct MinifsVolume {
    partition: u8,
    reader: RefCell<minifs::Reader<Source>>,
}

pub fn parse_minifs_id(id: &str) -> Option<(u8, String)> {
    let (partition, path) = id.strip_prefix("minifs:part:")?.split_once(':')?;
    let path = String::from_utf8(hex::decode(path).ok()?).ok()?;
    (minifs::valid_path(&path) && path.len() <= 4096).then_some((partition.parse().ok()?, path))
}

pub fn open_minifs_source(image: &Arc<DiskImage>, partition: u8) -> Result<MinifsVolume> {
    MinifsVolume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}

impl MinifsVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let base = partition
            .start_lba
            .checked_mul(image.sector_size as u64)
            .context("MINIFS base overflow")?;
        let length = partition
            .length_lba
            .checked_mul(image.sector_size as u64)
            .context("MINIFS length overflow")?;
        ensure!(
            base.checked_add(length)
                .is_some_and(|end| end <= image.len()),
            "MINIFS partition exceeds source"
        );
        let reader = minifs::Reader::open(
            Source {
                image,
                base,
                length,
            },
            minifs::Limits::default(),
        )?;
        Ok(Self {
            partition: partition.id,
            reader: RefCell::new(reader),
        })
    }

    fn node_id(&self, path: &str) -> String {
        format!("minifs:part:{}:{}", self.partition, hex::encode(path))
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let reader = self.reader.borrow();
        let mut nodes = reader
            .index()
            .children(path)?
            .into_iter()
            .map(|entry| FsNode {
                id: self.node_id(entry.path),
                name: entry.path.rsplit('/').next().unwrap_or_default().to_owned(),
                parent_id: Some(self.node_id(path)),
                kind: if entry.file.is_some() {
                    NodeKind::File
                } else {
                    NodeKind::Dir
                },
                size: entry.file.map_or(0, |file| file.size),
                deleted: false,
                inode: None,
                timestamps: Mactb::default(),
                link_target: None,
                has_snapshot: false,
            })
            .collect::<Vec<_>>();
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
        self.reader.borrow_mut().read_range(path, offset, length)
    }
    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        let size = self
            .reader
            .borrow()
            .index()
            .entry(path)?
            .file
            .context("MINIFS directory has no content")?
            .size;
        let sample = self.read_file(path, 4096)?;
        Ok(build_stat(node_id, size, None, None, &sample, Vec::new()))
    }
    pub fn stream_file(
        &self,
        path: &str,
        chunk_size: usize,
        mut visitor: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        let size = self
            .reader
            .borrow()
            .index()
            .entry(path)?
            .file
            .context("MINIFS directory has no content")?
            .size;
        let mut offset = 0;
        while offset < size {
            let count = (size - offset).min(chunk_size.clamp(4096, 1024 * 1024) as u64) as usize;
            let bytes = self.read_file_range(path, offset, count)?;
            ensure!(bytes.len() == count, "MINIFS short stream");
            visitor(&bytes)?;
            offset += count as u64;
        }
        Ok(offset)
    }
}
