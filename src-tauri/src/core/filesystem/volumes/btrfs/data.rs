//! Extent-backed file reads and physical source runs.
use super::chunks::{map_run, read_logical};
use super::*;
use anyhow::ensure;

const MAX_COMPRESSED_BYTES: u64 = 128 * 1024;

impl<'a> BtrfsVolume<'a> {
    pub fn read_inode_data(
        &self,
        subvol: u64,
        item: &BtrfsInode,
        max_bytes: usize,
    ) -> anyhow::Result<Vec<u8>> {
        self.read_inode_range(subvol, item, 0, max_bytes)
    }

    pub fn read_inode_range(
        &self,
        subvol: u64,
        item: &BtrfsInode,
        offset: u64,
        length: usize,
    ) -> anyhow::Result<Vec<u8>> {
        if length == 0 || offset >= item.size {
            return Ok(Vec::new());
        }
        let mut bytes = Vec::new();
        self.visit_file_range(subvol, item, offset, length as u64, 64 * 1024, |part| {
            bytes.extend_from_slice(part);
            Ok(())
        })?;
        Ok(bytes)
    }

    pub fn stream_inode_data<F>(
        &self,
        subvol: u64,
        item: &BtrfsInode,
        chunk_size: usize,
        visitor: F,
    ) -> anyhow::Result<u64>
    where
        F: FnMut(&[u8]) -> anyhow::Result<()>,
    {
        self.visit_file_range(subvol, item, 0, item.size, chunk_size, visitor)
    }

    fn visit_file_range(
        &self,
        subvol: u64,
        item: &BtrfsInode,
        offset: u64,
        length: u64,
        chunk_size: usize,
        mut visit: impl FnMut(&[u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<u64> {
        if !matches!(item.kind(), NodeKind::File | NodeKind::Symlink) {
            return Ok(0);
        }
        let end = offset + length.min(item.size.saturating_sub(offset));
        if offset == end {
            return Ok(0);
        }
        let extents = self.collect_extents(subvol, item.objectid)?;
        let mut output = vec![0; chunk_size.clamp(64 * 1024, 4 * 1024 * 1024)];
        let mut position = offset;
        let mut index = 0;
        let mut decoded = Vec::new();
        let mut decoded_extent = None;
        while position < end {
            while let Some(extent) = extents.get(index) {
                if extent.end()? > position {
                    break;
                }
                index += 1;
            }
            let extent = extents
                .get(index)
                .filter(|extent| extent.file_offset <= position);
            let mut next = end;
            if let Some(extent) = extent {
                next = next.min(extent.end()?);
            } else if let Some(extent) = extents.get(index) {
                next = next.min(extent.file_offset);
            }
            let count = (next - position).min(output.len() as u64) as usize;
            let part = &mut output[..count];
            match extent.map(|extent| (&extent.kind, position - extent.file_offset)) {
                None => part.fill(0),
                Some((
                    ExtentKind::Regular {
                        ext_type: EXTENT_PREALLOC,
                        ..
                    },
                    _,
                ))
                | Some((ExtentKind::Regular { disk_bytenr: 0, .. }, _)) => part.fill(0),
                Some((
                    ExtentKind::Regular {
                        disk_bytenr,
                        offset,
                        compression: COMPRESS_NONE,
                        ..
                    },
                    within,
                )) => {
                    let logical = disk_bytenr
                        .checked_add(*offset)
                        .and_then(|base| base.checked_add(within))
                        .context(Failure::corrupt("btrfs: extent address overflow"))?;
                    read_logical(
                        self.image,
                        self.layout.as_ref(),
                        &self.chunks,
                        logical,
                        part,
                    )?;
                }
                Some((kind, within)) => {
                    if decoded_extent != Some(index) {
                        decoded = self.decode_extent(kind)?;
                        decoded_extent = Some(index);
                    }
                    let start = (kind.decoded_offset() + within) as usize;
                    let bytes = decoded.get(start..start + count).context(Failure::corrupt(
                        "btrfs: decoded extent shorter than logical length",
                    ))?;
                    part.copy_from_slice(bytes);
                }
            }
            visit(part)?;
            position += count as u64;
        }
        Ok(position - offset)
    }

    pub fn data_runs(&self, subvol: u64, item: &BtrfsInode) -> anyhow::Result<Vec<FsDataRun>> {
        let mut runs = Vec::new();
        for extent in self.collect_extents(subvol, item.objectid)? {
            if let ExtentKind::Regular {
                ext_type: EXTENT_REGULAR,
                disk_bytenr,
                offset,
                num_bytes,
                compression: COMPRESS_NONE,
                ..
            } = extent.kind
            {
                if disk_bytenr == 0 {
                    continue;
                }
                let mut logical = disk_bytenr
                    .checked_add(offset)
                    .context(Failure::corrupt("btrfs: physical extent address overflow"))?;
                let mut file_offset = extent.file_offset;
                let mut remaining = num_bytes.min(item.size.saturating_sub(file_offset));
                while remaining > 0 {
                    let run = map_run(&self.chunks, logical)?;
                    let length = remaining.min(run.length);
                    runs.push(FsDataRun {
                        logical_offset: file_offset,
                        physical_offset: run.physical,
                        length,
                    });
                    logical += length;
                    file_offset += length;
                    remaining -= length;
                }
            }
        }
        crate::core::filesystem::volumes::lvm::physical_runs(self.layout.as_ref(), runs)
    }

    pub(super) fn collect_extents(
        &self,
        subvol: u64,
        inode: u64,
    ) -> anyhow::Result<Vec<FileExtent>> {
        let mut out = Vec::new();
        let (root_bytenr, root_level) = self.subvol_root(subvol)?;
        let walker = self.walker();
        walker.iter_range(
            root_bytenr,
            root_level,
            &Key {
                objectid: inode.into(),
                key_type: KEY_EXTENT_DATA,
                offset: 0u64.into(),
            },
            &Key {
                objectid: inode.into(),
                key_type: KEY_EXTENT_DATA,
                offset: u64::MAX.into(),
            },
            |key, data| {
                if key.objectid.get() != inode || key.key_type != KEY_EXTENT_DATA {
                    return Ok(false);
                }
                let extent = parse_file_extent(key.offset.get(), data)
                    .context(Failure::corrupt("btrfs: invalid file extent"))?;
                out.push(extent);
                Ok(true)
            },
        )?;
        out.sort_by_key(|extent| extent.file_offset);
        let mut end = 0;
        for extent in &out {
            ensure!(
                extent.file_offset >= end,
                Failure::corrupt("btrfs: overlapping file extents")
            );
            end = extent.end()?;
        }
        Ok(out)
    }

    fn decode_extent(&self, kind: &ExtentKind) -> anyhow::Result<Vec<u8>> {
        let sectorsize = self.sectorsize as usize;
        match kind {
            ExtentKind::Inline {
                data,
                ram_bytes,
                compression,
            } => {
                ensure!(
                    *ram_bytes <= INLINE_MAX_BYTES,
                    Failure::corrupt("btrfs: inline extent length limit")
                );
                let bytes = decompress(*compression, data, *ram_bytes as usize, sectorsize)?;
                ensure!(
                    bytes.len() == *ram_bytes as usize,
                    Failure::corrupt("btrfs: decoded inline extent disagrees with logical size")
                );
                Ok(bytes)
            }
            ExtentKind::Regular {
                disk_bytenr,
                disk_num_bytes,
                ram_bytes,
                compression,
                ..
            } => {
                ensure!(
                    *ram_bytes <= MAX_COMPRESSED_BYTES && *disk_num_bytes <= MAX_COMPRESSED_BYTES,
                    Failure::corrupt("btrfs: compressed extent length limit")
                );
                let mut raw = vec![0; *disk_num_bytes as usize];
                read_logical(
                    self.image,
                    self.layout.as_ref(),
                    &self.chunks,
                    *disk_bytenr,
                    &mut raw,
                )?;
                decompress_regular(*compression, &raw, *ram_bytes as usize, sectorsize)
            }
        }
    }
}

impl FileExtent {
    fn end(&self) -> anyhow::Result<u64> {
        let length = match self.kind {
            ExtentKind::Inline { ram_bytes, .. } => ram_bytes,
            ExtentKind::Regular {
                disk_bytenr,
                disk_num_bytes,
                offset,
                num_bytes,
                ram_bytes,
                compression,
                ..
            } => {
                if disk_bytenr != 0 {
                    let logical_end = offset
                        .checked_add(num_bytes)
                        .context(Failure::corrupt("btrfs: extent logical length overflow"))?;
                    ensure!(
                        logical_end <= ram_bytes
                            && (compression != COMPRESS_NONE || logical_end <= disk_num_bytes)
                            && disk_bytenr.checked_add(disk_num_bytes).is_some(),
                        Failure::corrupt("btrfs: invalid extent allocation length")
                    );
                }
                num_bytes
            }
        };
        ensure!(length > 0, Failure::corrupt("btrfs: empty file extent"));
        self.file_offset
            .checked_add(length)
            .context(Failure::corrupt("btrfs: file extent end overflow"))
    }
}

impl ExtentKind {
    fn decoded_offset(&self) -> u64 {
        match self {
            Self::Inline { .. } => 0,
            Self::Regular { offset, .. } => *offset,
        }
    }
}
