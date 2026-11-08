//! Normalized Minix layouts, adapted from rusty-backup src/fs/minix.rs.
//! Local changes: endian-aware checked fields, V3 first-zone derivation,
//! inode/map/source bounds and independently retained timestamp roles.
use crate::require;
use lamfold::{FileKind, FileTimes, FoldError, Metadata, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    V1,
    V2,
    V3,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}
impl Endian {
    pub(crate) fn u16(self, bytes: &[u8], offset: usize) -> Result<u16> {
        let value: [u8; 2] = bytes
            .get(offset..offset + 2)
            .ok_or(FoldError::Corrupt("minix: truncated u16"))?
            .try_into()
            .map_err(|_| FoldError::Corrupt("minix: u16"))?;
        Ok(match self {
            Self::Little => u16::from_le_bytes(value),
            Self::Big => u16::from_be_bytes(value),
        })
    }
    pub(crate) fn u32(self, bytes: &[u8], offset: usize) -> Result<u32> {
        let value: [u8; 4] = bytes
            .get(offset..offset + 4)
            .ok_or(FoldError::Corrupt("minix: truncated u32"))?
            .try_into()
            .map_err(|_| FoldError::Corrupt("minix: u32"))?;
        Ok(match self {
            Self::Little => u32::from_le_bytes(value),
            Self::Big => u32::from_be_bytes(value),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Superblock {
    pub(crate) version: Version,
    pub(crate) endian: Endian,
    pub(crate) inodes: u32,
    pub(crate) inode_map_blocks: u32,
    pub(crate) zone_map_blocks: u32,
    pub(crate) first_data_zone: u32,
    pub(crate) log_zone_size: u32,
    pub(crate) max_size: u32,
    pub(crate) zones: u32,
    pub(crate) block_size: u32,
    pub(crate) name_length: usize,
}
impl Superblock {
    /// Parse the fixed 32-byte superblock at source byte offset 1024. Validates
    /// all format geometry; call `validate_source` before reading disk data.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        require(bytes.len() >= 32, "minix: short superblock")?;
        for endian in [Endian::Little, Endian::Big] {
            let format = match endian.u16(bytes, 16)? {
                0x137f => Some((Version::V1, 14)),
                0x138f => Some((Version::V1, 30)),
                0x2468 => Some((Version::V2, 14)),
                0x2478 => Some((Version::V2, 30)),
                _ => None,
            };
            let v3 = endian.u16(bytes, 24)? == 0x4d5a;
            require(
                !(v3 && format.is_some()),
                "minix: ambiguous superblock generations",
            )?;
            let (version, name_length) = if let Some(format) = format {
                format
            } else if v3 {
                (Version::V3, 60)
            } else {
                continue;
            };
            let read16 = |offset| endian.u16(bytes, offset).map(u32::from);
            let mut header = if version == Version::V3 {
                if bytes[30] != 0 {
                    return Err(FoldError::Unsupported("minix: V3 disk version"));
                }
                if endian.u16(bytes, 14)? & 0xff00 != 0 {
                    return Err(FoldError::Unsupported(
                        "minix: mandatory V3 filesystem flags",
                    ));
                }
                Self {
                    version,
                    endian,
                    name_length,
                    inodes: endian.u32(bytes, 0)?,
                    inode_map_blocks: read16(6)?,
                    zone_map_blocks: read16(8)?,
                    first_data_zone: read16(10)?,
                    log_zone_size: read16(12)?,
                    max_size: endian.u32(bytes, 16)?,
                    zones: endian.u32(bytes, 20)?,
                    block_size: read16(28)?,
                }
            } else {
                Self {
                    version,
                    endian,
                    name_length,
                    inodes: read16(0)?,
                    inode_map_blocks: read16(4)?,
                    zone_map_blocks: read16(6)?,
                    first_data_zone: read16(8)?,
                    log_zone_size: read16(10)?,
                    max_size: endian.u32(bytes, 12)?,
                    zones: if version == Version::V1 {
                        read16(2)?
                    } else {
                        endian.u32(bytes, 20)?
                    },
                    block_size: 1024,
                }
            };
            require(
                header.inodes > 0
                    && header.zones > 0
                    && header.inode_map_blocks > 0
                    && header.zone_map_blocks > 0
                    && matches!(header.block_size, 1024 | 2048 | 4096)
                    && header.log_zone_size <= 8
                    && header.max_size > 0,
                "minix: invalid superblock geometry",
            )?;
            let inode_table_end =
                header.inode_table_start() + u64::from(header.inodes) * header.inode_size();
            if version == Version::V3 && header.first_data_zone == 0 {
                header.first_data_zone =
                    u32::try_from(inode_table_end.div_ceil(header.zone_size()))
                        .map_err(|_| FoldError::Corrupt("minix: derived first zone overflow"))?;
            }
            require(
                header.first_data_zone > 0
                    && header.first_data_zone < header.zones
                    && inode_table_end <= u64::from(header.first_data_zone) * header.zone_size(),
                "minix: inode table/data zone overlap",
            )?;
            let bits_per_block = u64::from(header.block_size) * 8;
            require(
                u64::from(header.inode_map_blocks) * bits_per_block > u64::from(header.inodes)
                    && u64::from(header.zone_map_blocks) * bits_per_block
                        > u64::from(header.zones - header.first_data_zone),
                "minix: allocation bitmap capacity",
            )?;
            return Ok(header);
        }
        Err(FoldError::Corrupt("minix: unknown superblock magic"))
    }
    pub fn validate_source(&self, length: u64) -> Result<()> {
        require(
            u64::from(self.zones) * self.zone_size() <= length,
            "minix: volume exceeds source",
        )
    }
    pub fn zone_size(&self) -> u64 {
        u64::from(self.block_size) << self.log_zone_size
    }
    pub(crate) fn inode_size(&self) -> u64 {
        if self.version == Version::V1 {
            32
        } else {
            64
        }
    }
    pub(crate) fn inode_table_start(&self) -> u64 {
        (2 + u64::from(self.inode_map_blocks) + u64::from(self.zone_map_blocks))
            * u64::from(self.block_size)
    }
    pub(crate) fn pointer_size(&self) -> usize {
        if self.version == Version::V1 {
            2
        } else {
            4
        }
    }
    pub(crate) fn inode_field_size(&self) -> usize {
        if self.version == Version::V3 {
            4
        } else {
            2
        }
    }
    pub(crate) fn pointers_per_block(&self) -> u64 {
        u64::from(self.block_size) / self.pointer_size() as u64
    }
    pub(crate) fn file_capacity(&self) -> u64 {
        let pointers = self.pointers_per_block();
        (7 + pointers
            + pointers.pow(2)
            + if self.version == Version::V1 {
                0
            } else {
                pointers.pow(3)
            })
            * self.zone_size()
    }
}

#[derive(Clone)]
pub(crate) struct Inode {
    pub(crate) metadata: Metadata,
    pub(crate) times: FileTimes,
    pub(crate) zones: [u32; 10],
}
impl Inode {
    pub(crate) fn parse(bytes: &[u8], header: &Superblock) -> Result<Self> {
        require(
            bytes.len() as u64 == header.inode_size(),
            "minix: short inode",
        )?;
        let order = header.endian;
        let mode = order.u16(bytes, 0)?;
        let kind = match mode & 0xf000 {
            0x8000 => FileKind::Regular,
            0x4000 => FileKind::Directory,
            0xa000 => FileKind::Symlink,
            0x1000 | 0x2000 | 0x6000 | 0xc000 => FileKind::Other,
            _ => return Err(FoldError::Corrupt("minix: invalid allocated inode type")),
        };
        let size = u64::from(order.u32(bytes, if header.version == Version::V1 { 4 } else { 8 })?);
        require(
            size <= u64::from(header.max_size) && size <= header.file_capacity(),
            "minix: inode size exceeds format capacity",
        )?;
        let mut zones = [0; 10];
        let times = if header.version == Version::V1 {
            require(bytes[13] > 0, "minix: unlinked inode in live directory")?;
            for (index, zone) in zones.iter_mut().take(9).enumerate() {
                *zone = u32::from(order.u16(bytes, 14 + index * 2)?);
            }
            FileTimes {
                modified: Some((i64::from(order.u32(bytes, 8)?), 0)),
                ..FileTimes::default()
            }
        } else {
            require(
                order.u16(bytes, 2)? > 0,
                "minix: unlinked inode in live directory",
            )?;
            for (index, zone) in zones.iter_mut().enumerate() {
                *zone = order.u32(bytes, 24 + index * 4)?;
            }
            FileTimes {
                accessed: Some((i64::from(order.u32(bytes, 12)?), 0)),
                modified: Some((i64::from(order.u32(bytes, 16)?), 0)),
                changed: Some((i64::from(order.u32(bytes, 20)?), 0)),
                created: None,
            }
        };
        Ok(Self {
            metadata: Metadata {
                kind,
                size,
                mode: u32::from(mode),
            },
            times,
            zones,
        })
    }
}
