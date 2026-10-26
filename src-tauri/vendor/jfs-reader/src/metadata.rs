use crate::{exact, le16, le32, le64, require};
use lamfold::{BlockSource, FoldError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderCopy {
    Primary,
    Secondary,
}

/// Source-relative aggregate geometry. The mapped area excludes reserved
/// inline log/fsck tails; those tails remain part of the original source.
#[derive(Clone, Debug)]
pub struct Header {
    pub version: u32,
    pub block_size: u32,
    pub blocks: u64,
    pub flags: u32,
    pub state: u32,
    pub label: Vec<u8>,
    pub uuid: [u8; 16],
    pub selected_copy: HeaderCopy,
    reserved: [Physical; 4],
}
impl Header {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        require(
            bytes.len() >= 184 && bytes.get(..4) == Some(b"JFS1"),
            "jfs: superblock signature/size",
        )?;
        let flags = le32(bytes, 36)?;
        if flags & 0xf0000000 != 0x10000000 || flags & !0x103f0bff != 0 || le32(bytes, 44)? != 0 {
            return Err(FoldError::Unsupported("jfs: platform/features/compression"));
        }
        let state = le32(bytes, 40)?;
        if state != 0 {
            return Err(FoldError::Unsupported(
                "jfs: dirty/mounted/recovery/extend state requires replay",
            ));
        }
        Self::inspect(bytes)
    }
    /// Structural inspection also recognizes unsupported/dirty Linux JFS so
    /// callers can identify the filesystem before reporting its read limit.
    pub fn inspect(bytes: &[u8]) -> Result<Self> {
        require(
            bytes.len() >= 184 && bytes.get(..4) == Some(b"JFS1"),
            "jfs: superblock signature/size",
        )?;
        let version = le32(bytes, 4)?;
        if !matches!(version, 1 | 2) {
            return Err(FoldError::Unsupported("jfs: superblock version"));
        }
        let flags = le32(bytes, 36)?;
        let state = le32(bytes, 40)?;
        let block_size = le32(bytes, 16)?;
        let physical_size = le32(bytes, 24)?;
        require(
            block_size.is_power_of_two()
                && (512..=4096).contains(&block_size)
                && physical_size.is_power_of_two()
                && (512..=block_size).contains(&physical_size),
            "jfs: block geometry",
        )?;
        require(
            u32::from(le16(bytes, 20)?) == block_size.ilog2()
                && u32::from(le16(bytes, 28)?) == physical_size.ilog2()
                && u32::from(le16(bytes, 22)?) == (block_size / physical_size).ilog2(),
            "jfs: block geometry logarithms",
        )?;
        let physical_blocks = le64(bytes, 8)?;
        require(
            physical_blocks > 0 && physical_blocks <= i64::MAX as u64,
            "jfs: signed aggregate size",
        )?;
        let size = physical_blocks
            .checked_mul(u64::from(physical_size))
            .ok_or(FoldError::Corrupt("jfs: aggregate size overflow"))?;
        require(
            size.is_multiple_of(u64::from(block_size)) && size >= 65536,
            "jfs: aggregate size alignment",
        )?;
        let blocks = size / u64::from(block_size);
        require(
            blocks <= 1u64 << 40,
            "jfs: aggregate exceeds 40-bit block address space",
        )?;
        let agsize = le32(bytes, 32)?;
        require(
            agsize.is_power_of_two() && u64::from(agsize) >= 8192,
            "jfs: allocation group size",
        )?;
        // Linux JFS is little endian even on big-endian hosts. OS/2's casefold
        // namespace and AIX/DFS layouts need their own explicit implementations.
        let mut uuid = [0; 16];
        uuid.copy_from_slice(&bytes[136..152]);
        let label = bytes[152..168]
            .split(|byte| *byte == 0)
            .next()
            .ok_or(FoldError::Corrupt("jfs: label"))?
            .to_vec();
        let reserved = [
            Physical::decode(bytes, 48)?,
            Physical::decode(bytes, 56)?,
            Physical::decode(bytes, 72)?,
            Physical::decode(bytes, 80)?,
        ];
        Ok(Self {
            version,
            block_size,
            blocks,
            flags,
            state,
            label,
            uuid,
            selected_copy: HeaderCopy::Primary,
            reserved,
        })
    }
    pub fn mapped_bytes(&self) -> u64 {
        self.blocks * u64::from(self.block_size)
    }
    pub fn validate_source(&self, length: u64) -> Result<()> {
        require(
            self.mapped_bytes() <= length,
            "jfs: mapped volume exceeds source",
        )?;
        for extent in self.reserved {
            require(
                (extent.start + u64::from(extent.count)) * u64::from(self.block_size) <= length,
                "jfs: reserved/mirror/log extent exceeds source",
            )?;
        }
        Ok(())
    }
    /// Use the primary superblock when valid. The secondary is at 61440 bytes,
    /// directly after the fixed primary AIT (not at 8 MiB).
    pub fn read(source: &mut impl BlockSource) -> Result<Self> {
        let mut first_error = None;
        for (offset, copy) in [(32768, HeaderCopy::Primary), (61440, HeaderCopy::Secondary)] {
            let mut bytes = [0; 512];
            let result = exact(source, offset, &mut bytes)
                .and_then(|()| Self::parse(&bytes))
                .and_then(|mut header| {
                    header.validate_source(source.len())?;
                    header.selected_copy = copy;
                    Ok(header)
                });
            match result {
                Ok(header) => return Ok(header),
                Err(error @ FoldError::Unsupported(_)) => return Err(error),
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        Err(first_error.ok_or(FoldError::Corrupt("jfs: no superblock"))?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Physical {
    pub start: u64,
    pub count: u32,
}
impl Physical {
    fn decode(bytes: &[u8], offset: usize) -> Result<Self> {
        let word = le32(bytes, offset)?;
        let count = word & 0xffffff;
        let start = (u64::from(word >> 24) << 32) | u64::from(le32(bytes, offset + 4)?);
        require(
            count != 0 || start == 0,
            "jfs: nonzero empty physical extent",
        )?;
        Ok(Self { start, count })
    }
    pub fn parse(bytes: &[u8], offset: usize, header: &Header) -> Result<Self> {
        let extent = Self::decode(bytes, offset)?;
        require(
            extent.start + u64::from(extent.count) <= header.blocks,
            "jfs: physical extent outside mapped volume",
        )?;
        Ok(extent)
    }
    pub fn byte(&self, header: &Header) -> u64 {
        self.start * u64::from(header.block_size)
    }
    pub fn length(&self, header: &Header) -> u64 {
        u64::from(self.count) * u64::from(header.block_size)
    }
}
