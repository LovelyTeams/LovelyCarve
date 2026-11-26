//! File Entries / Extended File Entries and bounded allocation-extent chains.
use crate::{
    descriptors::*,
    partitions::{Address, MetadataGeometry},
    udf::Udf,
};
use alloc::{collections::BTreeSet, string::String, vec::Vec};
use lamfold::{BlockSource, FileKind, FileTimes, FoldError, Metadata, Result};

#[derive(Clone)]
pub(crate) struct Extent {
    pub(crate) address: Option<Address>,
    pub(crate) length: u32,
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
                            .address
                            .ok_or(FoldError::Corrupt("udf: sparse directory descriptor"))?;
                        return block
                            .block
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
struct FileEntry<'a> {
    extended: bool,
    time_start: usize,
    flags: u16,
    size: u64,
    allocation: &'a [u8],
}

impl<'a> FileEntry<'a> {
    fn parse(bytes: &'a [u8], block: u32) -> Result<Self> {
        let (header_size, time_start, extended) = match descriptor(bytes, block, 36)? {
            261 => (176, 72, false),
            266 => (216, 80, true),
            259 => return Err(FoldError::Unsupported("udf: indirect ICB strategy")),
            _ => return Err(FoldError::Corrupt("udf: expected file entry")),
        };
        require(
            u16_at(bytes, 20)? == 4 && u16_at(bytes, 24)? == 1,
            "udf: unsupported ICB strategy/entries",
        )?;
        let flags = u16_at(bytes, 34)?;
        if flags & 0x1800 != 0 {
            return Err(FoldError::Unsupported("udf: transformed/multiversion file"));
        }
        let size = u64_at(bytes, 56)?;
        let extended_attributes = u32_at(bytes, header_size - 8)? as usize;
        let allocation_length = u32_at(bytes, header_size - 4)? as usize;
        let allocation_start = header_size
            .checked_add(extended_attributes)
            .ok_or(FoldError::Corrupt("udf: EA offset overflow"))?;
        let allocation_end = allocation_start
            .checked_add(allocation_length)
            .ok_or(FoldError::Corrupt("udf: AD end overflow"))?;
        descriptor(bytes, block, allocation_end)?;
        let allocation = bytes
            .get(allocation_start..allocation_end)
            .ok_or(FoldError::Corrupt("udf: allocation descriptor bounds"))?;
        if extended && (u64_at(bytes, 64)? != size || u32_at(bytes, 152)? != 0) {
            return Err(FoldError::Unsupported(
                "udf: named streams or differing object size",
            ));
        }
        Ok(Self {
            extended,
            time_start,
            flags,
            size,
            allocation,
        })
    }
}

impl<S: BlockSource> Udf<S> {
    pub(crate) fn read_fe(&mut self, address: Address) -> Result<UdfInode> {
        let block = address.block;
        let bytes = self.partition_block(address)?;
        let entry = FileEntry::parse(&bytes, block)?;
        let data = self.file_data(address, &entry)?;
        let FileEntry {
            extended,
            time_start,
            size,
            ..
        } = entry;
        let kind = match bytes[27] {
            4 => FileKind::Directory,
            5 => FileKind::Regular,
            12 => FileKind::Symlink,
            6 | 7 | 9 | 10 => FileKind::Other,
            _ => return Err(FoldError::Unsupported("udf: ICB file type")),
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
    fn file_data(&mut self, address: Address, entry: &FileEntry<'_>) -> Result<FileData> {
        Ok(match entry.flags & 7 {
            3 => {
                require(
                    entry.size <= entry.allocation.len() as u64,
                    "udf: inline logical length exceeds allocation area",
                )?;
                FileData::Inline {
                    bytes: entry.allocation[..entry.size as usize].to_vec(),
                    block: address.block,
                }
            }
            ad_type @ (0 | 1) => FileData::Extents(self.allocations(
                address,
                entry.allocation,
                ad_type == 1,
                entry.size,
                None,
            )?),
            _ => {
                return Err(FoldError::Unsupported(
                    "udf: extended allocation descriptors",
                ))
            }
        })
    }

    pub(crate) fn vat_file_bytes(&mut self, block: u32, revision: u16) -> Result<Option<Vec<u8>>> {
        let address = Address {
            block,
            reference: self.maps.physical_ref,
        };
        let bytes = self.partition_block(address)?;
        let vat_type = if revision == 0x0150 { 0 } else { 248 };
        if !matches!(u16_at(&bytes, 0)?, 261 | 266) || bytes[27] != vat_type {
            return Ok(None);
        }
        let entry = FileEntry::parse(&bytes, block)?;
        require(
            entry.size <= 64 * 1024 * 1024,
            "udf: VAT exceeds read limit",
        )?;
        let data = self.file_data(address, &entry)?;
        let mut table = alloc::vec![0; entry.size as usize];
        self.read_data(&data, entry.size, 0, &mut table)?;
        Ok(Some(table))
    }

    pub(crate) fn metadata_extents(
        &mut self,
        address: Address,
        file_type: u8,
        geometry: MetadataGeometry,
    ) -> Result<Vec<Extent>> {
        let bytes = self.partition_block(address)?;
        let entry = FileEntry::parse(&bytes, address.block)?;
        let unique_id_offset = if entry.extended { 200 } else { 160 };
        require(
            bytes[27] == file_type
                && u64_at(&bytes, unique_id_offset)? == 0
                && entry.flags & 7 == 0
                && entry.size > 0,
            "udf: invalid metadata file entry",
        )?;
        self.allocations(address, entry.allocation, false, entry.size, Some(geometry))
    }

    fn allocations(
        &mut self,
        owner: Address,
        first: &[u8],
        long: bool,
        size: u64,
        metadata_geometry: Option<MetadataGeometry>,
    ) -> Result<Vec<Extent>> {
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
                let address = Address {
                    reference: if long {
                        u16_at(record, 8)?
                    } else {
                        owner.reference
                    },
                    block: u32_at(record, 4)?,
                };
                self.check_reference(address.reference)?;
                if extent_type == 3 {
                    require(
                        (index + 1) * stride == allocation.len()
                            && visited.len() < 128
                            && visited.insert(address),
                        "udf: allocation continuation cycle/limit/position",
                    )?;
                    require(
                        length >= 24 && length <= self.block_size,
                        "udf: allocation continuation length",
                    )?;
                    continuation = Some((address, length));
                    break;
                }
                require(
                    extents.len() < 65536
                        && (extents.is_empty() || total.is_multiple_of(u64::from(self.block_size))),
                    "udf: extent count/alignment",
                )?;
                if let Some(geometry) = metadata_geometry {
                    require(
                        extent_type != 1
                            && u64::from(length).is_multiple_of(
                                u64::from(geometry.allocation_unit) * u64::from(self.block_size),
                            )
                            && (extent_type == 2
                                || address.block.is_multiple_of(geometry.alignment_unit)),
                        "udf: metadata extent type or geometry",
                    )?;
                }
                if extent_type != 2 {
                    let allocated = u64::from(length).div_ceil(u64::from(self.block_size))
                        * u64::from(self.block_size);
                    self.check_partition(address, allocated)?;
                }
                total = total
                    .checked_add(u64::from(length))
                    .ok_or(FoldError::Corrupt("udf: file extent sum overflow"))?;
                extents.push(Extent {
                    address: (extent_type == 0).then_some(address),
                    length,
                });
            }
            let Some((address, length)) = continuation else {
                break;
            };
            let bytes = self.partition_block(address)?;
            let allocation_length = u32_at(&bytes, 20)? as usize;
            require(
                24 + allocation_length <= length as usize,
                "udf: allocation extent length mismatch",
            )?;
            require(
                descriptor(&bytes, address.block, 24 + allocation_length)? == 258,
                "udf: invalid allocation extent descriptor",
            )?;
            allocation = bytes
                .get(24..24 + allocation_length)
                .ok_or(FoldError::Corrupt("udf: allocation extent bounds"))?
                .to_vec();
        }
        require(
            total >= size && (metadata_geometry.is_none() || total == size),
            "udf: allocation map disagrees with logical file length",
        )?;
        Ok(extents)
    }
    pub(crate) fn content(
        &mut self,
        inode: &UdfInode,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        self.read_data(&inode.data, inode.metadata.size, offset, bytes)
    }

    fn read_data(
        &mut self,
        data: &FileData,
        size: u64,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        let want = (bytes.len() as u64).min(size.saturating_sub(offset)) as usize;
        if want == 0 {
            return Ok(0);
        }
        match data {
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
                        match extent.address {
                            Some(address) => self.read_partition(
                                address,
                                within,
                                &mut bytes[produced..produced + count],
                            )?,
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
