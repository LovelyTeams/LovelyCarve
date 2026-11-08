//! Superblock prefix adapted from unearth's MIT GFS2 detector, with strict
//! source geometry and separate 1801/1802 support. This is not GFS1.
use crate::{bytes, exact, meta, require, u32be, u64be};
use lamfold::{BlockSource, FoldError, Result};

pub const SUPERBLOCK_OFFSET: u64 = 65536;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inum {
    pub formal: u64,
    pub address: u64,
}
impl Inum {
    pub(crate) fn parse(raw: &[u8], at: usize) -> Result<Self> {
        Ok(Self {
            formal: u64be(raw, at)?,
            address: u64be(raw, at + 8)?,
        })
    }
}
#[derive(Clone, Debug)]
pub struct Header {
    pub format: u32,
    pub block_size: u32,
    pub master: Inum,
    pub root: Inum,
    pub lock_protocol: String,
    pub lock_table: String,
    pub uuid: [u8; 16],
    pub(crate) source_blocks: u64,
}
impl Header {
    pub fn inspect(source: &mut impl BlockSource) -> Result<Self> {
        let mut raw = [0; 272];
        exact(source, SUPERBLOCK_OFFSET, &mut raw)?;
        Self::inspect_prefix(&raw, source.len())
    }
    /// `raw` begins at byte 65536 of the actual volume, not at byte zero.
    /// A source span is only an upper bound; resource groups define allocation.
    pub fn inspect_prefix(raw: &[u8], source_length: u64) -> Result<Self> {
        meta(raw, 1, 100)?;
        let format = u32be(raw, 24)?;
        if !matches!(format, 1801 | 1802) || u32be(raw, 28)? != 1900 {
            return Err(FoldError::Unsupported(
                "gfs2: filesystem/multihost format (GFS1 is separate)",
            ));
        }
        let block_size = u32be(raw, 36)?;
        require(
            (512..=65536).contains(&block_size) && block_size.is_power_of_two(),
            "gfs2: block size",
        )?;
        require(
            u32be(raw, 40)? == block_size.trailing_zeros(),
            "gfs2: block-size shift",
        )?;
        require(
            source_length >= SUPERBLOCK_OFFSET + block_size as u64,
            "gfs2: truncated superblock block",
        )?;
        let root = Inum::parse(raw, 80)?;
        let master = Inum::parse(raw, 48)?;
        let source_blocks = source_length / block_size as u64;
        for inode in [root, master] {
            require(
                inode.formal != 0
                    && inode.address > SUPERBLOCK_OFFSET / block_size as u64
                    && inode.address < source_blocks,
                "gfs2: root/master inode reference",
            )?;
        }
        require(
            root.address != master.address && root.formal != master.formal,
            "gfs2: root/master alias",
        )?;
        let text = |at: usize| -> Result<String> {
            let field = raw
                .get(at..at + 64)
                .ok_or(FoldError::Corrupt("gfs2: short lock name"))?;
            let length = field
                .iter()
                .position(|byte| *byte == 0)
                .ok_or(FoldError::Corrupt("gfs2: unterminated lock name"))?;
            String::from_utf8(field[..length].to_vec())
                .map_err(|_| FoldError::Unsupported("gfs2: non-UTF-8 lock name"))
        };
        Ok(Self {
            format,
            block_size,
            master,
            root,
            lock_protocol: text(96)?,
            lock_table: text(160)?,
            uuid: bytes(raw, 256)?,
            source_blocks,
        })
    }
    pub(crate) fn block_offset(&self, block: u64) -> Result<u64> {
        require(
            block > SUPERBLOCK_OFFSET / self.block_size as u64 && block < self.source_blocks,
            "gfs2: block outside data span",
        )?;
        block
            .checked_mul(self.block_size as u64)
            .ok_or(FoldError::Corrupt("gfs2: block offset overflow"))
    }
    pub(crate) fn read_block(&self, source: &mut impl BlockSource, block: u64) -> Result<Vec<u8>> {
        let mut raw = vec![0; self.block_size as usize];
        exact(source, self.block_offset(block)?, &mut raw)?;
        Ok(raw)
    }
}
