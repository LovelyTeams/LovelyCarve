//! GFS2 records POSIX atime/mtime/ctime with nanoseconds and no birth time.
use crate::{header::Header, meta, require, u16be, u32be, u64be, Inum};
use lamfold::{BlockSource, FileKind, FileTimes, FoldError, Result};

#[derive(Clone)]
pub(crate) struct Inode {
    pub number: Inum,
    pub mode: u32,
    pub size: u64,
    pub blocks: u64,
    pub flags: u32,
    pub height: u16,
    pub depth: u16,
    pub entries: u32,
    pub eattr: u64,
    pub times: FileTimes,
    pub raw: Vec<u8>,
}
impl Inode {
    pub fn read(source: &mut impl BlockSource, header: &Header, address: u64) -> Result<Self> {
        let raw = header.read_block(source, address)?;
        meta(&raw, 4, 400)?;
        let number = Inum::parse(&raw, 24)?;
        require(
            number.address == address && number.formal != 0,
            "gfs2: inode identity",
        )?;
        require(u32be(&raw, 52)? != 0, "gfs2: unlinked inode")?;
        let mode = u32be(&raw, 40)?;
        require(
            matches!(
                mode & 0xf000,
                0x1000 | 0x2000 | 0x4000 | 0x6000 | 0x8000 | 0xa000 | 0xc000
            ),
            "gfs2: inode mode",
        )?;
        let size = u64be(&raw, 56)?;
        let blocks = u64be(&raw, 64)?;
        require(
            size <= i64::MAX as u64 && blocks != 0 && blocks <= header.source_blocks,
            "gfs2: inode size/blocks",
        )?;
        let flags = u32be(&raw, 128)?;
        if flags & 0x2000_0004 != 0 {
            return Err(FoldError::Unsupported(
                "gfs2: GFS1/resumable-truncation inode",
            ));
        }
        let height = u16be(&raw, 138)?;
        let depth = u16be(&raw, 146)?;
        require(height <= 10 && depth <= 17, "gfs2: inode tree height/depth")?;
        let directory = mode & 0xf000 == 0x4000;
        require(
            directory || flags & 2 == 0 && depth == 0,
            "gfs2: directory flags on non-directory",
        )?;
        if height == 0 {
            require(
                size <= (header.block_size - 232) as u64,
                "gfs2: stuffed size exceeds inode",
            )?;
        }
        let time = |at: usize, nanos: usize| -> Result<(i64, u32)> {
            let nanos = u32be(&raw, nanos)?;
            require(nanos < 1_000_000_000, "gfs2: timestamp nanoseconds")?;
            Ok((u64be(&raw, at)? as i64, nanos))
        };
        let times = FileTimes {
            accessed: Some(time(72, 176)?),
            modified: Some(time(80, 180)?),
            changed: Some(time(88, 184)?),
            created: None,
        };
        Ok(Self {
            number,
            mode,
            size,
            blocks,
            flags,
            height,
            depth,
            entries: u32be(&raw, 148)?,
            eattr: u64be(&raw, 168)?,
            times,
            raw,
        })
    }
    pub fn kind(&self) -> FileKind {
        match self.mode & 0xf000 {
            0x4000 => FileKind::Directory,
            0x8000 => FileKind::Regular,
            0xa000 => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }
    pub fn payload_size(&self, header: &Header) -> u64 {
        header.block_size as u64
            - if self.kind() == FileKind::Directory {
                24
            } else {
                0
            }
    }
}
