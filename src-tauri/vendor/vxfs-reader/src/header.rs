use lamfold::{BlockSource, FoldError, Result};

use crate::require;

const MAGIC: u32 = 0xa501fcf5;
pub const HEADER_SIZE: usize = 512;
pub const SUPERBLOCKS: [(u64, Endian); 2] = [(1024, Endian::Little), (8192, Endian::Big)];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endian {
    Little,
    Big,
}

impl Endian {
    pub(crate) fn u16(self, bytes: &[u8], offset: usize) -> u16 {
        let word = bytes[offset..offset + 2].try_into().unwrap();
        match self {
            Self::Little => u16::from_le_bytes(word),
            Self::Big => u16::from_be_bytes(word),
        }
    }

    pub(crate) fn u32(self, bytes: &[u8], offset: usize) -> u32 {
        let word = bytes[offset..offset + 4].try_into().unwrap();
        match self {
            Self::Little => u32::from_le_bytes(word),
            Self::Big => u32::from_be_bytes(word),
        }
    }

    pub(crate) fn u64(self, bytes: &[u8], offset: usize) -> u64 {
        let word = bytes[offset..offset + 8].try_into().unwrap();
        match self {
            Self::Little => u64::from_le_bytes(word),
            Self::Big => u64::from_be_bytes(word),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Header {
    pub endian: Endian,
    pub superblock_offset: u64,
    pub layout: u32,
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub clean: bool,
    pub label: String,
    pub(crate) olt_start: u64,
    pub(crate) olt_length: u64,
}

impl Header {
    pub fn inspect(
        bytes: &[u8],
        offset: u64,
        endian: Endian,
        source_length: u64,
    ) -> Result<Option<Self>> {
        if bytes.len() < 4 || endian.u32(bytes, 0) != MAGIC {
            return Ok(None);
        }
        require(bytes.len() >= HEADER_SIZE, "VxFS truncated superblock")?;
        let layout = endian.u32(bytes, 4);
        let block_size = u64::from(endian.u32(bytes, 32));
        let blocks = u64::from(endian.u32(bytes, 36));
        let free_blocks = u64::from(endian.u32(bytes, 192));
        let volume_size = blocks * block_size;
        require(
            (1024..=65536).contains(&block_size)
                && block_size.is_power_of_two()
                && blocks > 0
                && volume_size <= source_length
                && offset
                    .checked_add(HEADER_SIZE as u64)
                    .is_some_and(|end| end <= volume_size)
                && free_blocks <= blocks,
            "VxFS invalid volume geometry",
        )?;
        let olt_start = u64::from(endian.u32(bytes, 368));
        let olt_length = u64::from(endian.u32(bytes, 376));
        if (2..=4).contains(&layout) {
            require(
                olt_start != 0 && olt_length != 0 && olt_start + olt_length <= blocks,
                "VxFS object location table exceeds volume",
            )?;
            require(
                matches!(endian.u32(bytes, 388), 0 | 256),
                "VxFS unsupported inode size",
            )?;
        }
        let label = &bytes[348..354];
        let label = label.split(|&byte| byte == 0).next().unwrap_or_default();
        Ok(Some(Self {
            endian,
            superblock_offset: offset,
            layout,
            block_size,
            blocks,
            free_blocks,
            clean: bytes[333] == 0x5a,
            label: String::from_utf8_lossy(label).trim().to_owned(),
            olt_start,
            olt_length,
        }))
    }

    pub(crate) fn read(source: &mut impl BlockSource) -> Result<Self> {
        for (offset, endian) in SUPERBLOCKS {
            if offset + HEADER_SIZE as u64 > source.len() {
                continue;
            }
            let mut bytes = [0; HEADER_SIZE];
            source.read_at(offset, &mut bytes)?;
            if let Some(header) = Self::inspect(&bytes, offset, endian, source.len())? {
                return Ok(header);
            }
        }
        Err(FoldError::Corrupt("VxFS superblock signature absent"))
    }
}
