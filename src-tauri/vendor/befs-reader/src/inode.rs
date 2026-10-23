use crate::{allocation::Allocation, exact, require, Header, Run};
use lamfold::{BlockSource, FileKind, FoldError, Result};

#[derive(Clone, Debug)]
pub(crate) struct Small {
    pub name: String,
    pub type_code: u32,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub(crate) struct Inode {
    pub number: u64,
    pub parent: u64,
    pub attributes: Option<u64>,
    pub mode: u32,
    pub flags: u32,
    pub created: i64,
    pub modified: i64,
    pub status_or_reserved: i64,
    pub type_code: u32,
    pub stream: [u8; 144],
    pub small: Vec<Small>,
}
impl Inode {
    pub fn read(
        source: &mut impl BlockSource,
        header: &Header,
        allocation: &mut Allocation,
        number: u64,
    ) -> Result<Self> {
        header.data_region(number, 1)?;
        allocation.check(source, header, number, 1)?;
        let mut raw = vec![0; header.block_size as usize];
        exact(source, number * u64::from(header.block_size), &mut raw)?;
        let endian = header.endian;
        require(
            endian.u32(&raw, 0)? == 0x3bbe0ad9
                && header.inode_run(Run::parse(endian, &raw, 4)?)? == number,
            "befs: inode magic/identity",
        )?;
        require(
            endian.u32(&raw, 64)? == header.block_size,
            "befs: per-inode size mismatch",
        )?;
        let flags = endian.u32(&raw, 24)?;
        require(
            flags & 1 != 0 && flags & (0x10 | 0x20) == 0,
            "befs: inode unallocated/deleted/not ready",
        )?;
        let parent = header.inode_run(Run::parse(endian, &raw, 44)?)?;
        let attr = Run::parse(endian, &raw, 52)?;
        let attributes = if attr.is_zero() {
            None
        } else {
            Some(header.inode_run(attr)?)
        };
        let stream = raw[72..216]
            .try_into()
            .map_err(|_| FoldError::Corrupt("befs: short stream header"))?;
        let mut small = Vec::new();
        let mut offset = 232;
        let mut names = std::collections::HashSet::new();
        while offset + 8 <= raw.len() {
            let name_length = usize::from(endian.u16(&raw, offset + 4)?);
            if name_length == 0 {
                break;
            }
            let length = usize::from(endian.u16(&raw, offset + 6)?);
            let end = offset + 8 + name_length + 3 + length + 1;
            require(end <= raw.len(), "befs: small-data record bounds")?;
            let name = &raw[offset + 8..offset + 8 + name_length];
            require(
                !name.contains(&0) && names.insert(name.to_vec()),
                "befs: small-data name NUL/duplicate",
            )?;
            let type_code = endian.u32(&raw, offset)?;
            if name != [0x13] {
                small.push(Small {
                    name: String::from_utf8(name.to_vec())
                        .map_err(|_| FoldError::Unsupported("befs: non-UTF-8 attribute name"))?,
                    type_code,
                    bytes: raw[end - length - 1..end - 1].to_vec(),
                });
            } else {
                require(type_code == 0x43535452, "befs: filename small-data type")?;
            }
            offset = end;
        }
        Ok(Self {
            number,
            parent,
            attributes,
            mode: endian.u32(&raw, 20)?,
            flags,
            created: endian.u64(&raw, 28)? as i64,
            modified: endian.u64(&raw, 36)? as i64,
            status_or_reserved: endian.u64(&raw, 216)? as i64,
            type_code: endian.u32(&raw, 60)?,
            stream,
            small,
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
    pub fn inline_link(&self) -> Result<Option<Vec<u8>>> {
        if self.kind() != FileKind::Symlink || self.flags & 0x40 != 0 {
            return Ok(None);
        }
        let end = self
            .stream
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(FoldError::Corrupt("befs: unterminated inline symlink"))?;
        require(end > 0, "befs: empty inline symlink")?;
        Ok(Some(self.stream[..end].to_vec()))
    }
}
