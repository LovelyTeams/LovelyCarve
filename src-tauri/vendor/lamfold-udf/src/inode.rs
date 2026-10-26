//! File Entries / Extended File Entries and bounded allocation-extent chains.
use crate::{descriptors::*, udf::Udf};
use alloc::{collections::BTreeSet, string::String, vec::Vec};
use lamfold::{BlockSource, FileKind, FileTimes, FoldError, Metadata, Result};

#[derive(Clone)]
pub(crate) struct Extent {
    block: Option<u32>,
    length: u32,
}
#[derive(Clone)]
pub(crate) enum FileData {
    Inline { bytes: Vec<u8>, block: u32 },
    Extents(Vec<Extent>),
}
impl FileData {
    /// FID tag locations describe the partition-relative block where the FID
    /// starts, including inline directories and records crossing block edges.
    pub(crate) fn location(&self, offset: u64, block_size: u32) -> Result<u32> {
        match self {
            Self::Inline { block, .. } => Ok(*block),
            Self::Extents(extents) => {
                let mut position = 0;
                for extent in extents {
                    if offset < position + u64::from(extent.length) {
                        let block = extent
                            .block
                            .ok_or(FoldError::Corrupt("udf: sparse directory descriptor"))?;
                        return block
                            .checked_add(((offset - position) / u64::from(block_size)) as u32)
                            .ok_or(FoldError::Corrupt("udf: FID location overflow"));
                    }
                    position += u64::from(extent.length);
                }
                Err(FoldError::Corrupt("udf: FID outside allocation map"))
            }
        }
    }
}
#[derive(Clone)]
pub(crate) struct UdfInode {
    pub(crate) metadata: Metadata,
    pub(crate) times: FileTimes,
    pub(crate) data: FileData,
}
impl UdfInode {
    pub(crate) fn cache_cost(&self) -> usize {
        256 + match &self.data {
            FileData::Inline { bytes, .. } => bytes.len(),
            FileData::Extents(extents) => extents.len() * core::mem::size_of::<Extent>(),
        }
    }
}
impl<S: BlockSource> Udf<S> {
    pub(crate) fn read_fe(&mut self, block: u32) -> Result<UdfInode> {
        let bytes = self.partition_block(block)?;
        let (header_size, time_start, extended) = match descriptor(&bytes, block, 36)? {
            261 => (176, 72, false),
            266 => (216, 80, true),
            259 => return Err(FoldError::Unsupported("udf: indirect ICB strategy")),
            _ => return Err(FoldError::Corrupt("udf: expected file entry")),
        };
        require(
            u16_at(&bytes, 20)? == 4 && u16_at(&bytes, 24)? == 1,
            "udf: unsupported ICB strategy/entries",
        )?;
        let flags = u16_at(&bytes, 34)?;
        if flags & 0x1800 != 0 {
            return Err(FoldError::Unsupported("udf: transformed/multiversion file"));
        }
        let size = u64_at(&bytes, 56)?;
        let extended_attributes = u32_at(&bytes, header_size - 8)? as usize;
        let allocation_length = u32_at(&bytes, header_size - 4)? as usize;
        let allocation_start = header_size
            .checked_add(extended_attributes)
            .ok_or(FoldError::Corrupt("udf: EA offset overflow"))?;
        let allocation_end = allocation_start
            .checked_add(allocation_length)
            .ok_or(FoldError::Corrupt("udf: AD end overflow"))?;
        descriptor(&bytes, block, allocation_end)?;
        let allocation = bytes
            .get(allocation_start..allocation_end)
            .ok_or(FoldError::Corrupt("udf: allocation descriptor bounds"))?;
        if extended && (u64_at(&bytes, 64)? != size || u32_at(&bytes, 152)? != 0) {
            return Err(FoldError::Unsupported(
                "udf: named streams or differing object size",
            ));
        }
        let kind = match bytes[27] {
            4 => FileKind::Directory,
            5 => FileKind::Regular,
            12 => FileKind::Symlink,
            6 | 7 | 9 | 10 => FileKind::Other,
            _ => return Err(FoldError::Unsupported("udf: ICB file type")),
        };
        let data = match flags & 7 {
            3 => {
                require(
                    size <= allocation.len() as u64,
                    "udf: inline logical length exceeds allocation area",
                )?;
                FileData::Inline {
                    bytes: allocation[..size as usize].to_vec(),
                    block,
                }
            }
            ad_type @ (0 | 1) => self.allocations(allocation, ad_type == 1, size)?,
            _ => {
                return Err(FoldError::Unsupported(
                    "udf: extended allocation descriptors",
                ))
            }
        };
        let permissions = u32_at(&bytes, 44)?;
        let mode =
            ((permissions & 7) | ((permissions >> 5 & 7) << 3) | ((permissions >> 10 & 7) << 6))
                | match kind {
                    FileKind::Regular => 0x8000,
                    FileKind::Directory => 0x4000,
                    FileKind::Symlink => 0xa000,
                    FileKind::Other => 0,
                };
        let times = FileTimes {
            accessed: timestamp(&bytes[time_start..])?,
            modified: timestamp(&bytes[time_start + 12..])?,
            created: if extended {
                timestamp(&bytes[time_start + 24..])?
            } else {
                None
            },
            changed: timestamp(&bytes[time_start + if extended { 36 } else { 24 }..])?,
        };
        let mut inode = UdfInode {
            metadata: Metadata { kind, size, mode },
            times,
            data,
        };
        if kind == FileKind::Symlink {
            require(size <= 65536, "udf: symbolic link size limit")?;
            let mut encoded = alloc::vec![0; size as usize];
            self.content(&inode, 0, &mut encoded)?;
            let target = decode_link(&encoded)?.into_bytes();
            inode.metadata.size = target.len() as u64;
            inode.data = FileData::Inline {
                bytes: target,
                block,
            };
        }
        Ok(inode)
    }
    fn allocations(&mut self, first: &[u8], long: bool, size: u64) -> Result<FileData> {
        let stride = if long { 16 } else { 8 };
        let mut allocation = first.to_vec();
        let mut extents = Vec::new();
        let mut visited = BTreeSet::new();
        let mut total = 0u64;
        loop {
            require(
                allocation.len().is_multiple_of(stride),
                "udf: partial allocation descriptor",
            )?;
            let mut continuation = None;
            for (index, record) in allocation.chunks_exact(stride).enumerate() {
                let raw_length = u32_at(record, 0)?;
                let length = raw_length & 0x3fff_ffff;
                let extent_type = raw_length >> 30;
                if length == 0 {
                    continue;
                }
                if long {
                    require(
                        u16_at(record, 8)? == 0,
                        "udf: foreign allocation partition reference",
                    )?;
                }
                let block = u32_at(record, 4)?;
                if extent_type == 3 {
                    require(
                        (index + 1) * stride == allocation.len()
                            && visited.len() < 128
                            && visited.insert(block),
                        "udf: allocation continuation cycle/limit/position",
                    )?;
                    require(
                        length >= 24 && length <= self.block_size,
                        "udf: allocation continuation length",
                    )?;
                    continuation = Some((block, length));
                    break;
                }
                require(
                    extents.len() < 65536
                        && (extents.is_empty() || total.is_multiple_of(u64::from(self.block_size))),
                    "udf: extent count/alignment",
                )?;
                if extent_type != 2 {
                    let allocated = u64::from(length).div_ceil(u64::from(self.block_size))
                        * u64::from(self.block_size);
                    self.physical(block, allocated)?;
                }
                total = total
                    .checked_add(u64::from(length))
                    .ok_or(FoldError::Corrupt("udf: file extent sum overflow"))?;
                extents.push(Extent {
                    block: (extent_type == 0).then_some(block),
                    length,
                });
            }
            let Some((block, length)) = continuation else {
                break;
            };
            let bytes = self.partition_block(block)?;
            let allocation_length = u32_at(&bytes, 20)? as usize;
            require(
                24 + allocation_length <= length as usize,
                "udf: allocation extent length mismatch",
            )?;
            require(
                descriptor(&bytes, block, 24 + allocation_length)? == 258,
                "udf: invalid allocation extent descriptor",
            )?;
            allocation = bytes
                .get(24..24 + allocation_length)
                .ok_or(FoldError::Corrupt("udf: allocation extent bounds"))?
                .to_vec();
        }
        require(
            total >= size,
            "udf: allocation map shorter than logical file",
        )?;
        Ok(FileData::Extents(extents))
    }
    pub(crate) fn content(
        &mut self,
        inode: &UdfInode,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        let want = (bytes.len() as u64).min(inode.metadata.size.saturating_sub(offset)) as usize;
        if want == 0 {
            return Ok(0);
        }
        match &inode.data {
            FileData::Inline { bytes: inline, .. } => {
                let start = usize::try_from(offset)
                    .map_err(|_| FoldError::Corrupt("udf: inline offset"))?;
                bytes[..want].copy_from_slice(
                    inline
                        .get(start..start + want)
                        .ok_or(FoldError::Corrupt("udf: inline short read"))?,
                );
            }
            FileData::Extents(extents) => {
                let mut position = 0;
                let mut produced = 0;
                for extent in extents {
                    let end = position + u64::from(extent.length);
                    let current = offset + produced as u64;
                    if current < end {
                        let within = current - position;
                        let count = (u64::from(extent.length) - within)
                            .min((want - produced) as u64)
                            as usize;
                        match extent.block {
                            Some(block) => {
                                let absolute =
                                    self.physical(block, u64::from(extent.length))? + within;
                                exact(
                                    &mut self.src,
                                    absolute,
                                    &mut bytes[produced..produced + count],
                                )?;
                            }
                            None => bytes[produced..produced + count].fill(0),
                        }
                        produced += count;
                        if produced == want {
                            break;
                        }
                    }
                    position = end;
                }
                require(produced == want, "udf: short logical read")?;
            }
        }
        Ok(want)
    }
}

fn decode_link(mut encoded: &[u8]) -> Result<String> {
    let mut target = String::new();
    while !encoded.is_empty() {
        require(encoded.len() >= 4, "udf: short symbolic link component")?;
        let component_type = encoded[0];
        let length = usize::from(encoded[1]);
        require(
            u16_at(encoded, 2)? == 0,
            "udf: symbolic link component version",
        )?;
        let value = encoded
            .get(4..4 + length)
            .ok_or(FoldError::Corrupt("udf: symbolic link component length"))?;
        if component_type != 5 {
            require(length == 0, "udf: symbolic link special component length")?;
        }
        match component_type {
            1 | 2 => {
                require(target.is_empty(), "udf: symbolic link root position")?;
                target.push('/');
            }
            3..=5 => {
                if !target.is_empty() && !target.ends_with('/') {
                    target.push('/');
                }
                if component_type == 3 {
                    target.push_str("..");
                } else if component_type == 4 {
                    target.push('.');
                } else {
                    let name = decode_name(value)?;
                    safe_component(&name)?;
                    target.push_str(&name);
                }
            }
            _ => return Err(FoldError::Unsupported("udf: symbolic link component type")),
        }
        encoded = &encoded[4 + length..];
    }
    require(!target.is_empty(), "udf: empty symbolic link")?;
    Ok(target)
}
