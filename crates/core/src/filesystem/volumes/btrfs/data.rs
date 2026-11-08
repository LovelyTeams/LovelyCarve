//! Extent-backed file reads, streaming and data runs.
use super::*;

impl<'a> BtrfsVolume<'a> {
    pub fn read_inode_data(
        &self,
        subvol: u64,
        item: &BtrfsInode,
        max_bytes: usize,
    ) -> anyhow::Result<Vec<u8>> {
        if item.kind() != NodeKind::File && item.kind() != NodeKind::Symlink {
            return Ok(Vec::new());
        }
        let extents = self.collect_extents(subvol, item.objectid)?;
        let total = item.size.min(max_bytes as u64) as usize;
        let mut out = Vec::with_capacity(total.min(64 * 1024));
        for ext in &extents {
            if out.len() >= total {
                break;
            }
            let want = total - out.len();
            let chunk = self.read_extent(ext, want)?;
            out.extend_from_slice(&chunk);
        }
        out.truncate(total);
        Ok(out)
    }

    pub fn stream_inode_data<F>(
        &self,
        subvol: u64,
        item: &BtrfsInode,
        chunk_size: usize,
        mut visitor: F,
    ) -> anyhow::Result<u64>
    where
        F: FnMut(&[u8]) -> anyhow::Result<()>,
    {
        if item.kind() != NodeKind::File && item.kind() != NodeKind::Symlink {
            return Ok(0);
        }
        let chunk_size = chunk_size.max(64 * 1024);
        let extents = self.collect_extents(subvol, item.objectid)?;
        let mut emitted = 0u64;
        for ext in &extents {
            if emitted >= item.size {
                break;
            }
            let remaining = item.size - emitted;
            let bytes = self.read_extent(ext, remaining as usize)?;
            for slice in bytes.chunks(chunk_size) {
                visitor(slice)?;
            }
            emitted += bytes.len() as u64;
        }
        Ok(emitted.min(item.size))
    }

    pub fn data_runs(&self, subvol: u64, item: &BtrfsInode) -> anyhow::Result<Vec<FsDataRun>> {
        let extents = self.collect_extents(subvol, item.objectid)?;
        let mut runs = Vec::new();
        for ext in &extents {
            if let ExtentKind::Regular {
                disk_bytenr,
                offset,
                num_bytes,
                compression,
                ..
            } = ext.kind
            {
                if disk_bytenr == 0 || compression != COMPRESS_NONE {
                    continue;
                }
                if let Some(map) = self.logical_to_physical(disk_bytenr + offset) {
                    let length = num_bytes.min(map.length);
                    runs.push(FsDataRun {
                        logical_offset: ext.file_offset,
                        physical_offset: self.base_offset + map.physical,
                        length,
                    });
                }
            }
        }
        crate::filesystem::volumes::lvm::physical_runs(self.layout.as_ref(), runs)
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
                if let Some(ext) = parse_file_extent(key.offset.get(), data) {
                    out.push(ext);
                }
                Ok(true)
            },
        )?;
        out.sort_by_key(|e| e.file_offset);
        Ok(out)
    }

    pub(super) fn read_extent(&self, ext: &FileExtent, max: usize) -> anyhow::Result<Vec<u8>> {
        match &ext.kind {
            ExtentKind::Inline {
                ram_bytes,
                compression,
                data,
            } => {
                if *ram_bytes > INLINE_MAX_BYTES {
                    bail!(
                        "btrfs: inline extent ram_bytes {ram_bytes} > sanity cap {INLINE_MAX_BYTES}"
                    );
                }
                let decoded = decompress(
                    *compression,
                    data,
                    *ram_bytes as usize,
                    self.sectorsize as usize,
                )?;
                Ok(decoded.into_iter().take(max).collect())
            }
            ExtentKind::Regular {
                ext_type,
                disk_bytenr,
                disk_num_bytes,
                offset,
                num_bytes,
                ram_bytes,
                compression,
            } => {
                if *ext_type == EXTENT_PREALLOC || *disk_bytenr == 0 {
                    let zero = (*num_bytes as usize).min(max);
                    return Ok(vec![0u8; zero]);
                }
                if *compression == COMPRESS_NONE {
                    let map = self
                        .logical_to_physical(*disk_bytenr + *offset)
                        .ok_or_else(|| {
                            anyhow!(
                                "btrfs: extent at logical 0x{disk_bytenr:x}+0x{offset:x} is not chunk-mapped"
                            )
                        })?;
                    let phys = self.base_offset + map.physical;
                    let want = (*num_bytes as usize).min(max).min(map.length as usize);
                    crate::filesystem::volumes::lvm::read_backing_range(
                        self.image,
                        self.layout.as_ref(),
                        phys,
                        want,
                    )
                } else {
                    let map = self.logical_to_physical(*disk_bytenr).ok_or_else(|| {
                        anyhow!(
                            "btrfs: compressed extent at logical 0x{disk_bytenr:x} is not chunk-mapped"
                        )
                    })?;
                    let phys = self.base_offset + map.physical;
                    let on_disk = (*disk_num_bytes as usize).min(map.length as usize);
                    let raw = crate::filesystem::volumes::lvm::read_backing_range(
                        self.image,
                        self.layout.as_ref(),
                        phys,
                        on_disk,
                    )?;
                    let decoded = decompress(
                        *compression,
                        &raw,
                        *ram_bytes as usize,
                        self.sectorsize as usize,
                    )?;
                    let start = *offset as usize;
                    if start >= decoded.len() {
                        return Ok(Vec::new());
                    }
                    let end = decoded
                        .len()
                        .min(start + (*num_bytes as usize))
                        .min(start + max);
                    Ok(decoded[start..end].to_vec())
                }
            }
        }
    }

    pub(super) fn logical_to_physical(&self, logical: u64) -> Option<ChunkMappingResult> {
        // Chunks are sorted by `logical`; do a binary search for the first
        // chunk whose end is past `logical`, then check it actually covers.
        let idx = self
            .chunks
            .partition_point(|c| c.logical + c.length <= logical);
        let cm = self.chunks.get(idx)?;
        if logical < cm.logical || logical >= cm.logical + cm.length {
            return None;
        }
        let delta = logical - cm.logical;
        Some(ChunkMappingResult {
            physical: cm.physical + delta,
            length: cm.length - delta,
        })
    }
}
