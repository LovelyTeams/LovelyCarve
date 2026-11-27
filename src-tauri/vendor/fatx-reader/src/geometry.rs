use lamfold::{BlockSource, FoldError, Result};

use crate::require;

pub const HEADER_BYTES: usize = 16;
pub const SECTOR_BYTES: u64 = 512;
pub const FAT_START: u64 = 4096;
const FAT_ALIGNMENT: u64 = 4096;
const FAT16_RESERVED: u64 = 0xfff0;
const FAT32_RESERVED: u64 = 0xfffffff0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Fatx,
    Xtaf,
}

impl Format {
    pub(crate) fn word(self, bytes: &[u8]) -> u16 {
        let bytes = bytes[..2].try_into().unwrap();
        match self {
            Self::Fatx => u16::from_le_bytes(bytes),
            Self::Xtaf => u16::from_be_bytes(bytes),
        }
    }

    pub(crate) fn dword(self, bytes: &[u8]) -> u32 {
        let bytes = bytes[..4].try_into().unwrap();
        match self {
            Self::Fatx => u32::from_le_bytes(bytes),
            Self::Xtaf => u32::from_be_bytes(bytes),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub format: Format,
    pub volume_id: u32,
    pub cluster_size: u64,
    root: u32,
}

impl Header {
    pub fn inspect(bytes: &[u8], size: u64) -> Result<Option<Self>> {
        let format = match bytes.get(..4) {
            Some(b"FATX") => Format::Fatx,
            Some(b"XTAF") => Format::Xtaf,
            _ => return Ok(None),
        };
        require(
            bytes.len() >= HEADER_BYTES && size >= 2 * FAT_START + SECTOR_BYTES,
            "FATX truncated volume",
        )?;
        let sectors = format.dword(&bytes[8..]);
        let maximum = if format == Format::Fatx { 1024 } else { 128 };
        require(
            sectors.is_power_of_two() && sectors <= maximum,
            "FATX invalid sectors per cluster",
        )?;
        let root = format.dword(&bytes[12..]);
        require(root != 0, "FATX missing root cluster")?;
        Ok(Some(Self {
            format,
            volume_id: format.dword(&bytes[4..]),
            cluster_size: u64::from(sectors) * SECTOR_BYTES,
            root,
        }))
    }

    pub(crate) fn geometry(self, source: &mut impl BlockSource) -> Result<Geometry> {
        let normal = Geometry::new(self, source.len(), false);
        // Some XTAF image writers store a big-endian 16-bit 1 in the 32-bit
        // root field and size the FAT from the space after the superblock.
        if self.format == Format::Xtaf && self.root == 0x10000 {
            if let Ok(candidate) = normal {
                let mut bytes = [0; 4];
                source.read_at(
                    candidate.fat_offset(candidate.root),
                    &mut bytes[..candidate.entry_size],
                )?;
                if candidate.decode_entry(&bytes) != 0 {
                    return Ok(candidate);
                }
            }
            return Geometry::new(self, source.len(), true);
        }
        normal
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Geometry {
    pub format: Format,
    pub root: u32,
    pub cluster_size: u64,
    pub entry_size: usize,
    compact: bool,
    data_start: u64,
    clusters: u32,
}

impl Geometry {
    fn new(header: Header, size: u64, compact: bool) -> Result<Self> {
        let count = if compact {
            (size - FAT_START) / header.cluster_size
        } else {
            size / header.cluster_size + 1
        };
        let fat16 = if compact {
            ((size - FAT_START) / SECTOR_BYTES).saturating_sub(260)
                / (header.cluster_size / SECTOR_BYTES)
                < 65525
        } else {
            count < FAT16_RESERVED
        };
        let entry_size = if fat16 { 2 } else { 4 };
        require(
            count < FAT32_RESERVED,
            "FATX cluster count exceeds address space",
        )?;
        let fat_size = (count * entry_size as u64).next_multiple_of(FAT_ALIGNMENT);
        let data_start = FAT_START + fat_size;
        require(size >= data_start, "FATX allocation table outside volume")?;
        let clusters = ((size - data_start) / header.cluster_size) as u32;
        let root = if compact { 1 } else { header.root };
        require(
            (1..=clusters).contains(&root) && u64::from(clusters) < fat_size / entry_size as u64,
            "FATX invalid root cluster or undersized allocation table",
        )?;
        Ok(Self {
            format: header.format,
            root,
            cluster_size: header.cluster_size,
            entry_size,
            compact,
            data_start,
            clusters,
        })
    }

    pub fn cluster_offset(self, cluster: u32) -> Result<u64> {
        if (1..=self.clusters).contains(&cluster) {
            Ok(self.data_start + (u64::from(cluster) - 1) * self.cluster_size)
        } else {
            Err(FoldError::Corrupt("FATX cluster outside volume"))
        }
    }

    pub fn fat_offset(self, cluster: u32) -> u64 {
        FAT_START + u64::from(cluster) * self.entry_size as u64
    }

    pub fn decode_entry(self, bytes: &[u8]) -> u32 {
        if self.entry_size == 2 {
            u32::from(self.format.word(bytes))
        } else {
            self.format.dword(bytes)
        }
    }

    pub fn end_of_chain(self, value: u32) -> bool {
        value
            >= match (self.entry_size, self.compact) {
                (2, _) => 0xfff8,
                (_, true) => 0x0ffffff8,
                _ => 0xfffffff8,
            }
    }

    pub fn valid_media(self, value: u32) -> bool {
        self.end_of_chain(value) || (self.compact && value == 0)
    }
}
