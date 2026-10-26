use crate::{le16, le32, le64, require};
use lamfold::{FileKind, FileTimes, FoldError, Result};

#[derive(Clone)]
pub(crate) struct Inode {
    pub size: u64,
    pub blocks: u64,
    pub mode: u32,
    pub raw: Vec<u8>,
    pub times: FileTimes,
}
impl Inode {
    pub fn parse(raw: &[u8]) -> Result<Self> {
        require(raw.len() >= 128, "nilfs2: short inode")?;
        let mode = le16(raw, 48)? as u32;
        require(
            le16(raw, 50)? != 0
                && matches!(
                    mode & 0xf000,
                    0x1000 | 0x2000 | 0x4000 | 0x6000 | 0x8000 | 0xa000 | 0xc000
                ),
            "nilfs2: deleted inode/invalid mode",
        )?;
        let size = le64(raw, 8)?;
        require(
            size <= i64::MAX as u64
                && le32(raw, 32)? < 1_000_000_000
                && le32(raw, 36)? < 1_000_000_000,
            "nilfs2: file size/nanosecond fields",
        )?;
        if le64(raw, 112)? != 0 {
            return Err(FoldError::Unsupported(
                "nilfs2: unsupported extended-attribute reference",
            ));
        }
        // Native NILFS has no stored atime; the kernel synthesizes it from mtime.
        // i_ctime is POSIX change time, despite the old on-disk header comment.
        let times = FileTimes {
            accessed: None,
            created: None,
            changed: Some((le64(raw, 16)? as i64, le32(raw, 32)?)),
            modified: Some((le64(raw, 24)? as i64, le32(raw, 36)?)),
        };
        Ok(Self {
            size,
            blocks: le64(raw, 0)?,
            mode,
            raw: raw.to_vec(),
            times,
        })
    }
    pub fn kind(&self) -> FileKind {
        match self.mode & 0xf000 {
            0x8000 => FileKind::Regular,
            0x4000 => FileKind::Directory,
            0xa000 => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }
}
