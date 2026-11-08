use crate::{
    header::{Header, Record},
    le16, le32, le64, require,
};
use lamfold::{BlockSource, FileKind, FileTimes, FoldError, Result};

#[derive(Clone)]
pub(crate) struct Inode {
    pub address: u64,
    pub mode: u32,
    pub size: u64,
    pub clusters: u32,
    pub flags: u32,
    pub dynamic: u16,
    pub inline_attributes: usize,
    pub attribute_block: u64,
    pub directory_index: u64,
    pub refcount: u64,
    pub last_extent: u64,
    pub times: FileTimes,
    pub raw: Vec<u8>,
}
impl Inode {
    pub fn read(source: &mut impl BlockSource, header: &Header, address: u64) -> Result<Self> {
        let raw = header.record(source, address, Record::Inode)?;
        let flags = le32(&raw, 44)?;
        require(
            flags & 1 != 0 && flags & 0x2004 == 0 && le64(&raw, 72)? == 0,
            "ocfs2: invalid/deleted/orphaned inode",
        )?;
        let mode = le16(&raw, 40)? as u32;
        require(
            flags & 0x10 != 0
                || matches!(
                    mode & 0xf000,
                    0x1000 | 0x2000 | 0x4000 | 0x6000 | 0x8000 | 0xa000 | 0xc000
                ),
            "ocfs2: inode mode",
        )?;
        require(
            le16(&raw, 42)? != 0 || le16(&raw, 16)? != 0,
            "ocfs2: inode without live links",
        )?;
        let size = le64(&raw, 32)?;
        let clusters = le32(&raw, 20)?;
        require(
            size <= i64::MAX as u64 && clusters <= header.clusters,
            "ocfs2: inode length/allocation",
        )?;
        let dynamic = le16(&raw, 118)?;
        if dynamic & !0x1f != 0 {
            return Err(FoldError::Unsupported(
                "ocfs2: unknown inode dynamic features",
            ));
        }
        let inline_attributes = if dynamic & 4 != 0 {
            le16(&raw, 18)? as usize
        } else {
            0
        };
        require(
            inline_attributes == 0
                || header.incompatible & 0x0200 != 0
                    && inline_attributes >= 256
                    && inline_attributes.is_multiple_of(16)
                    && inline_attributes <= raw.len() - 192,
            "ocfs2: inline attribute layout",
        )?;
        require(
            dynamic & 1 == 0 || header.incompatible & 0x40 != 0,
            "ocfs2: inline-data feature mismatch",
        )?;
        require(
            dynamic & 8 == 0 || mode & 0xf000 == 0x4000 && header.incompatible & 0x400 != 0,
            "ocfs2: directory-index feature mismatch",
        )?;
        require(
            dynamic & 16 == 0 || header.incompatible & 0x1000 != 0,
            "ocfs2: refcount feature mismatch",
        )?;
        let time = |seconds: usize, nanos: usize| -> Result<(i64, u32)> {
            let nanos = le32(&raw, nanos)?;
            require(nanos < 1_000_000_000, "ocfs2: nanosecond field")?;
            Ok((le64(&raw, seconds)? as i64, nanos))
        };
        // i_ctime is POSIX change time despite the old header's "creation" comment.
        let times = FileTimes {
            accessed: Some(time(48, 100)?),
            changed: Some(time(56, 104)?),
            modified: Some(time(64, 108)?),
            created: None,
        };
        Ok(Self {
            address,
            mode,
            size,
            clusters,
            flags,
            dynamic,
            inline_attributes,
            attribute_block: le64(&raw, 120)?,
            directory_index: le64(&raw, 136)?,
            refcount: le64(&raw, 144)?,
            last_extent: le64(&raw, 88)?,
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
    pub fn data_end(&self) -> usize {
        self.raw.len() - self.inline_attributes
    }
    pub fn inline(&self) -> Result<Option<&[u8]>> {
        let end = self.data_end();
        if self.kind() == FileKind::Symlink && self.clusters == 0 {
            require(
                self.size < (end - 192) as u64 && self.raw[192 + self.size as usize] == 0,
                "ocfs2: fast symlink length/terminator",
            )?;
            return Ok(Some(&self.raw[192..192 + self.size as usize]));
        }
        if self.dynamic & 1 != 0 {
            let available = le16(&self.raw, 192)? as usize;
            require(
                available <= end - 200 && self.size <= available as u64 && self.clusters == 0,
                "ocfs2: inline data bounds",
            )?;
            return Ok(Some(&self.raw[200..200 + self.size as usize]));
        }
        Ok(None)
    }
}
