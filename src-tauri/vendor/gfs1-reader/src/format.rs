use lamfold::{BlockSource, FileKind, FileTimes, FoldError, Result};

pub const SUPERBLOCK_OFFSET: u64 = 65536;
pub const SUPERBLOCK_BYTES: usize = 352;
pub(crate) const META_BYTES: usize = 24;
pub(crate) const INODE_BYTES: usize = 232;
pub(crate) const INDIRECT_BYTES: usize = 88;
pub(crate) const RESOURCE_GROUP_BYTES: usize = 128;
pub(crate) const RINDEX_FORMAT: u32 = 1100;
pub(crate) const JINDEX_FORMAT: u32 = 1000;
pub(crate) const DIRENT_FORMAT: u32 = 1200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Number {
    pub formal: u64,
    pub block: u64,
}

impl Number {
    pub fn parse(bytes: &[u8], at: usize) -> Result<Self> {
        Ok(Self {
            formal: u64be(bytes, at)?,
            block: u64be(bytes, at + 8)?,
        })
    }
}

#[derive(Debug)]
pub struct Header {
    pub block_size: u32,
    pub segment_blocks: u32,
    pub lock_protocol: String,
    pub lock_table: String,
    pub(crate) source_blocks: u64,
    pub(crate) root: Number,
    pub(crate) rindex: Number,
    pub(crate) jindex: Number,
}

impl Header {
    pub fn inspect(source: &mut impl BlockSource) -> Result<Self> {
        let mut raw = [0; SUPERBLOCK_BYTES];
        source.read_at(SUPERBLOCK_OFFSET, &mut raw)?;
        Self::inspect_prefix(&raw, source.len())
    }

    /// The supplied bytes begin at the superblock, 64 KiB into the volume.
    pub fn inspect_prefix(raw: &[u8], length: u64) -> Result<Self> {
        let raw = raw
            .get(..SUPERBLOCK_BYTES)
            .ok_or(FoldError::Corrupt("truncated GFS1 superblock"))?;
        meta(raw, 1, 100)?;
        if (u32be(raw, 24)?, u32be(raw, 28)?) != (1309, 1401) {
            return Err(FoldError::Unsupported(
                "GFS1 filesystem or multihost format",
            ));
        }
        let block_size = u32be(raw, 36)?;
        let segment_blocks = u32be(raw, 44)?;
        require(
            (512..=65536).contains(&block_size)
                && block_size.is_power_of_two()
                && block_size.trailing_zeros() == u32be(raw, 40)?
                && (2..=4 * 1024 * 1024 / block_size).contains(&segment_blocks)
                && length >= SUPERBLOCK_OFFSET + u64::from(block_size),
            "invalid GFS1 volume geometry",
        )?;
        let text = |at| -> Result<String> {
            let raw: &[u8] = &raw[at..at + 64];
            let end = raw
                .iter()
                .position(|byte| *byte == 0)
                .ok_or(FoldError::Corrupt("unterminated GFS1 lock field"))?;
            Ok(String::from_utf8_lossy(&raw[..end]).into_owned())
        };
        let header = Self {
            block_size,
            segment_blocks,
            root: Number::parse(raw, 80)?,
            rindex: Number::parse(raw, 64)?,
            jindex: Number::parse(raw, 48)?,
            lock_protocol: text(96)?,
            lock_table: text(160)?,
            source_blocks: length / u64::from(block_size),
        };
        let system = [header.root, header.rindex, header.jindex];
        for (index, number) in system.iter().enumerate() {
            header.block_offset(number.block)?;
            require(
                number.formal != 0
                    && system[..index]
                        .iter()
                        .all(|other| other.formal != number.formal && other.block != number.block),
                "invalid GFS1 superblock inode reference",
            )?;
        }
        Ok(header)
    }

    pub fn block_offset(&self, block: u64) -> Result<u64> {
        require(
            block > SUPERBLOCK_OFFSET / u64::from(self.block_size) && block < self.source_blocks,
            "GFS1 block outside volume bounds",
        )?;
        Ok(block * u64::from(self.block_size))
    }

    pub fn read_block(&self, source: &mut impl BlockSource, block: u64) -> Result<Vec<u8>> {
        let mut raw = vec![0; self.block_size as usize];
        source.read_at(self.block_offset(block)?, &mut raw)?;
        Ok(raw)
    }
}

pub(crate) struct Inode {
    pub raw: Vec<u8>,
    pub number: Number,
    pub mode: u32,
    pub size: u64,
    pub blocks: u64,
    pub times: FileTimes,
    pub flags: u32,
    pub payload_format: u32,
    pub kind: FileKind,
    pub raw_kind: u16,
    pub height: u16,
    pub depth: u16,
    pub entries: u32,
}

impl Inode {
    pub fn read(source: &mut impl BlockSource, header: &Header, block: u64) -> Result<Self> {
        let raw = header.read_block(source, block)?;
        meta(&raw, 4, 400)?;
        let number = Number::parse(&raw, 24)?;
        let mode = u32be(&raw, 40)?;
        let links = u32be(&raw, 52)?;
        let size = u64be(&raw, 56)?;
        let blocks = u64be(&raw, 64)?;
        let flags = u32be(&raw, 128)?;
        let raw_kind = u16be(&raw, 136)?;
        let kind = file_kind(raw_kind)?;
        let height = u16be(&raw, 138)?;
        let depth = u16be(&raw, 146)?;
        require(
            number.block == block
                && number.formal != 0
                && links != 0
                && flags & 4 == 0
                && blocks > 0
                && size <= i64::MAX as u64
                && height <= 10
                && depth <= 17
                && (height != 0 || size <= u64::from(header.block_size) - INODE_BYTES as u64),
            "invalid GFS1 inode identity, flags or geometry",
        )?;
        let times = FileTimes {
            accessed: Some((u64be(&raw, 72)? as i64, 0)),
            modified: Some((u64be(&raw, 80)? as i64, 0)),
            changed: Some((u64be(&raw, 88)? as i64, 0)),
            created: None,
        };
        let payload_format = u32be(&raw, 132)?;
        let entries = u32be(&raw, 148)?;
        Ok(Self {
            raw,
            number,
            mode,
            size,
            blocks,
            times,
            flags,
            payload_format,
            kind,
            raw_kind,
            height,
            depth,
            entries,
        })
    }
}

pub(crate) fn file_kind(kind: u16) -> Result<FileKind> {
    match kind {
        1 => Ok(FileKind::Regular),
        2 => Ok(FileKind::Directory),
        5 => Ok(FileKind::Symlink),
        7 | 8 | 101 | 102 => Ok(FileKind::Other),
        _ => Err(FoldError::Corrupt(
            "unknown GFS1 inode or directory entry type",
        )),
    }
}

pub(crate) fn require(condition: bool, error: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(error))
    }
}

fn field<const N: usize>(raw: &[u8], at: usize) -> Result<[u8; N]> {
    raw.get(at..)
        .and_then(|rest| rest.first_chunk())
        .copied()
        .ok_or(FoldError::Corrupt("truncated GFS1 field"))
}

pub(crate) fn u16be(raw: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(field(raw, at)?))
}
pub(crate) fn u32be(raw: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(field(raw, at)?))
}
pub(crate) fn u64be(raw: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(field(raw, at)?))
}

pub(crate) fn meta(raw: &[u8], kind: u32, format: u32) -> Result<()> {
    require(
        u32be(raw, 0)? == 0x0116_1970 && u32be(raw, 4)? == kind && u32be(raw, 16)? == format,
        "unexpected GFS1 metadata type or format",
    )
}
