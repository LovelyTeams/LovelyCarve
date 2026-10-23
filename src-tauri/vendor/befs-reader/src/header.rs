use crate::{exact, require};
use lamfold::{BlockSource, FoldError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}
impl Endian {
    pub(crate) fn u16(self, bytes: &[u8], offset: usize) -> Result<u16> {
        let raw: [u8; 2] = bytes
            .get(offset..offset + 2)
            .ok_or(FoldError::Corrupt("befs: short u16"))?
            .try_into()
            .map_err(|_| FoldError::Corrupt("befs: u16"))?;
        Ok(match self {
            Self::Little => u16::from_le_bytes(raw),
            Self::Big => u16::from_be_bytes(raw),
        })
    }
    pub(crate) fn u32(self, bytes: &[u8], offset: usize) -> Result<u32> {
        let raw: [u8; 4] = bytes
            .get(offset..offset + 4)
            .ok_or(FoldError::Corrupt("befs: short u32"))?
            .try_into()
            .map_err(|_| FoldError::Corrupt("befs: u32"))?;
        Ok(match self {
            Self::Little => u32::from_le_bytes(raw),
            Self::Big => u32::from_be_bytes(raw),
        })
    }
    pub(crate) fn u64(self, bytes: &[u8], offset: usize) -> Result<u64> {
        let raw: [u8; 8] = bytes
            .get(offset..offset + 8)
            .ok_or(FoldError::Corrupt("befs: short u64"))?
            .try_into()
            .map_err(|_| FoldError::Corrupt("befs: u64"))?;
        Ok(match self {
            Self::Little => u64::from_le_bytes(raw),
            Self::Big => u64::from_be_bytes(raw),
        })
    }
    pub(crate) fn nonnegative(self, bytes: &[u8], offset: usize) -> Result<u64> {
        let raw = self.u64(bytes, offset)?;
        require(raw <= i64::MAX as u64, "befs: negative length/offset")?;
        Ok(raw)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub group: u32,
    pub start: u16,
    pub length: u16,
}
impl Run {
    pub(crate) fn parse(endian: Endian, bytes: &[u8], offset: usize) -> Result<Self> {
        Ok(Self {
            group: endian.u32(bytes, offset)?,
            start: endian.u16(bytes, offset + 4)?,
            length: endian.u16(bytes, offset + 6)?,
        })
    }
    pub(crate) fn is_zero(self) -> bool {
        self.group == 0 && self.start == 0 && self.length == 0
    }
}
#[derive(Clone, Debug)]
pub struct Header {
    pub endian: Endian,
    pub block_size: u32,
    pub blocks: u64,
    pub used: u64,
    pub group_shift: u32,
    pub groups: u32,
    pub bitmap_blocks_per_group: u32,
    pub root: u64,
    pub indices: Option<u64>,
    pub clean: bool,
    pub label: String,
    pub superblock_offset: u64,
    pub(crate) bitmap_end: u64,
    pub(crate) log: std::ops::Range<u64>,
}
impl Header {
    /// Check both native superblock positions; recognized malformed copies
    /// cannot silently fall through to another geometry or source prefix.
    pub fn inspect(source: &mut impl BlockSource) -> Result<Self> {
        let mut found = None;
        for offset in [512, 0] {
            if offset + 132 > source.len() {
                continue;
            }
            let mut raw = [0; 132];
            exact(source, offset, &mut raw)?;
            let endian = match raw.get(32..36) {
                Some(b"1SFB") => Endian::Little,
                Some(b"BFS1") => Endian::Big,
                _ => continue,
            };
            let header = Self::parse(&raw, endian, offset, source.len())?;
            require(found.is_none(), "befs: ambiguous superblocks")?;
            found = Some(header);
        }
        found.ok_or(FoldError::Corrupt(
            "befs: no BFS superblock at byte 512 or 0",
        ))
    }
    /// Inspect one 132-byte superblock prefix against the actual volume span.
    /// This recognizes geometry without mounting or accepting dirty contents.
    pub fn inspect_at(raw: &[u8], superblock_offset: u64, source_length: u64) -> Result<Self> {
        require(
            raw.len() >= 132 && matches!(superblock_offset, 0 | 512),
            "befs: superblock prefix/offset",
        )?;
        let endian = match raw.get(32..36) {
            Some(b"1SFB") => Endian::Little,
            Some(b"BFS1") => Endian::Big,
            _ => return Err(FoldError::Corrupt("befs: superblock magic")),
        };
        Self::parse(&raw[..132], endian, superblock_offset, source_length)
    }
    fn parse(
        raw: &[u8],
        endian: Endian,
        superblock_offset: u64,
        source_length: u64,
    ) -> Result<Self> {
        require(
            endian.u32(raw, 36)? == 0x42494745
                && endian.u32(raw, 68)? == 0xdd121031
                && endian.u32(raw, 112)? == 0x15b6830e,
            "befs: superblock byte order/magic",
        )?;
        let block_size = endian.u32(raw, 40)?;
        let shift = endian.u32(raw, 44)?;
        require(
            (1024..=8192).contains(&block_size)
                && block_size.is_power_of_two()
                && shift == block_size.trailing_zeros(),
            "befs: unsupported block size/shift",
        )?;
        require(
            endian.u32(raw, 64)? == block_size,
            "befs: inode size differs from block size",
        )?;
        let blocks = endian.nonnegative(raw, 48)?;
        let used = endian.nonnegative(raw, 56)?;
        require(
            blocks >= 10
                && used <= blocks
                && blocks
                    .checked_mul(u64::from(block_size))
                    .is_some_and(|end| end <= source_length),
            "befs: volume/source length or used count",
        )?;
        let bitmap_blocks_per_group = endian.u32(raw, 72)?;
        let group_shift = endian.u32(raw, 76)?;
        let groups = endian.u32(raw, 80)?;
        require(
            (1..=16).contains(&group_shift)
                && groups > 0
                && groups <= i32::MAX as u32
                && bitmap_blocks_per_group > 0,
            "befs: allocation group geometry",
        )?;
        let group_size = 1u64 << group_shift;
        require(
            u64::from(groups) == blocks.div_ceil(group_size)
                && group_size == u64::from(bitmap_blocks_per_group) * u64::from(block_size) * 8,
            "befs: allocation bitmap/group coverage",
        )?;
        let bitmap_end = 1 + blocks.div_ceil(u64::from(block_size) * 8);
        require(
            bitmap_end < blocks && superblock_offset + 132 <= u64::from(block_size),
            "befs: superblock/bitmap overlap or bounds",
        )?;
        let flags = endian.u32(raw, 84)?;
        require(
            matches!(flags, 0x434c454e | 0x44495254),
            "befs: unknown clean/dirty state",
        )?;
        let mut header = Self {
            endian,
            block_size,
            blocks,
            used,
            group_shift,
            groups,
            bitmap_blocks_per_group,
            root: 0,
            indices: None,
            clean: false,
            label: String::new(),
            superblock_offset,
            bitmap_end,
            log: 0..0,
        };
        let (log, count) = header.run(Run::parse(endian, raw, 88)?)?;
        require(
            log >= bitmap_end && count > 0,
            "befs: log overlaps boot/bitmap",
        )?;
        header.log = log..log + count;
        let log_start = endian.nonnegative(raw, 96)?;
        let log_end = endian.nonnegative(raw, 104)?;
        require(
            header.log.contains(&log_start) && header.log.contains(&log_end),
            "befs: journal cursor outside log",
        )?;
        header.clean = flags == 0x434c454e && log_start == log_end;
        header.root = header.inode_run(Run::parse(endian, raw, 116)?)?;
        let indices = Run::parse(endian, raw, 124)?;
        if !indices.is_zero() {
            header.indices = Some(header.inode_run(indices)?);
        }
        let length = raw[..32].iter().position(|byte| *byte == 0).unwrap_or(32);
        header.label = String::from_utf8(raw[..length].to_vec())
            .map_err(|_| FoldError::Unsupported("befs: non-UTF-8 volume name"))?;
        Ok(header)
    }
    pub(crate) fn run(&self, run: Run) -> Result<(u64, u64)> {
        let group_size = 1u64 << self.group_shift;
        require(
            run.group < self.groups
                && run.length > 0
                && u64::from(run.start) + u64::from(run.length) <= group_size,
            "befs: run allocation group bounds",
        )?;
        let first = (u64::from(run.group) << self.group_shift) + u64::from(run.start);
        let count = u64::from(run.length);
        require(
            first
                .checked_add(count)
                .is_some_and(|end| end <= self.blocks),
            "befs: run outside volume",
        )?;
        Ok((first, count))
    }
    pub(crate) fn inode_run(&self, run: Run) -> Result<u64> {
        require(run.length == 1, "befs: inode run length is not one")?;
        let (first, _) = self.run(run)?;
        self.data_region(first, 1)?;
        Ok(first)
    }
    pub(crate) fn data_region(&self, first: u64, count: u64) -> Result<()> {
        require(
            first >= self.bitmap_end
                && count > 0
                && first.checked_add(count).is_some_and(|end| {
                    end <= self.blocks && (end <= self.log.start || first >= self.log.end)
                }),
            "befs: data overlaps reserved metadata or source bounds",
        )
    }
}
