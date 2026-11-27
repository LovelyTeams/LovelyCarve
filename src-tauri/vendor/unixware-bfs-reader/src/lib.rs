//! SCO UnixWare's flattened, contiguous boot filesystem.
#![forbid(unsafe_code)]

mod directory;
#[cfg(test)]
mod tests;

use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

pub const HEADER_SIZE: usize = 512;
const BLOCK: u64 = 512;
const MAGIC: u32 = 0x1badface;
const INODE_SIZE: usize = 64;
const ROOT: u16 = 2;

#[derive(Clone, Debug)]
pub struct Header {
    pub data_start: u64,
    pub volume_size: u64,
    pub inode_count: usize,
    pub name: String,
    pub volume: String,
    pub interrupted_compaction: bool,
}

impl Header {
    pub fn inspect(bytes: &[u8], source_size: u64) -> Result<Option<Self>> {
        if bytes.len() < 4 || le32(bytes, 0) != MAGIC {
            return Ok(None);
        }
        require(
            bytes.len() >= HEADER_SIZE,
            "UnixWare BFS truncated superblock",
        )?;
        let data_start = u64::from(le32(bytes, 4));
        let volume_size = u64::from(le32(bytes, 8)) + 1;
        require(
            data_start >= 2 * BLOCK
                && data_start.is_multiple_of(BLOCK)
                && volume_size <= source_size
                && volume_size.is_multiple_of(BLOCK)
                && data_start < volume_size,
            "UnixWare BFS invalid data area",
        )?;
        let inode_count = (data_start - BLOCK) / INODE_SIZE as u64;
        require(
            inode_count <= u64::from(u16::MAX - ROOT + 1),
            "UnixWare BFS inode table exceeds identifier space",
        )?;
        let label = |data: &[u8]| {
            String::from_utf8_lossy(data.split(|&byte| byte == 0).next().unwrap_or_default())
                .trim()
                .to_owned()
        };
        Ok(Some(Self {
            data_start,
            volume_size,
            inode_count: inode_count as usize,
            name: label(&bytes[28..34]),
            volume: label(&bytes[34..40]),
            interrupted_compaction: [12, 16, 20, 24]
                .into_iter()
                .any(|offset| le32(bytes, offset) != u32::MAX),
        }))
    }

    fn read(source: &mut impl BlockSource) -> Result<Option<Self>> {
        let len = source.len();
        if len < HEADER_SIZE as u64 {
            return Ok(None);
        }
        let mut bytes = [0; HEADER_SIZE];
        source.read_at(0, &mut bytes)?;
        Self::inspect(&bytes, len)
    }
}

#[derive(Clone, Copy)]
struct Inode {
    number: u16,
    kind: FileKind,
    mode: u32,
    offset: u64,
    size: u64,
    allocated_end: u64,
    times: FileTimes,
}

impl Inode {
    fn parse(bytes: &[u8; INODE_SIZE], index: usize, header: &Header) -> Result<Option<Self>> {
        let number = le16(bytes, 0);
        if number == 0 {
            return Ok(None);
        }
        require(
            usize::from(number) == index + usize::from(ROOT),
            "UnixWare BFS inode number mismatches slot",
        )?;
        let kind = match (number, le32(bytes, 16)) {
            (ROOT, 2) => FileKind::Directory,
            (other, 1) if other != ROOT => FileKind::Regular,
            _ => return Err(FoldError::Corrupt("UnixWare BFS unexpected vnode type")),
        };
        let start = u64::from(le32(bytes, 4)) * BLOCK;
        let last_block = le32(bytes, 8);
        let eof = le32(bytes, 12);
        let (size, allocated_end) = if start == 0 {
            require(
                kind == FileKind::Regular && last_block == 0 && matches!(eof, 0 | u32::MAX),
                "UnixWare BFS data extent missing",
            )?;
            (0, 0)
        } else {
            let end = (u64::from(last_block) + 1) * BLOCK;
            let eof = u64::from(eof) + 1;
            require(
                start >= header.data_start
                    && start < end
                    && end <= header.volume_size
                    && (start..=end).contains(&eof),
                "UnixWare BFS contiguous extent or logical EOF outside data area",
            )?;
            (eof - start, end)
        };
        let mode = le32(bytes, 20) & 0o7777;
        let type_bits = if kind == FileKind::Directory {
            0o040000
        } else {
            0o100000
        };
        let time = |offset| Some((i64::from(le32(bytes, offset)), 0));
        Ok(Some(Self {
            number,
            kind,
            mode: type_bits | mode,
            offset: start,
            size,
            allocated_end,
            times: FileTimes {
                accessed: time(36),
                modified: time(40),
                changed: time(44),
                created: None,
            },
        }))
    }
}

pub struct UnixwareBfs<S> {
    source: S,
    header: Header,
    inodes: Vec<Option<Inode>>,
}

impl<S: BlockSource> UnixwareBfs<S> {
    pub fn header(&self) -> &Header {
        &self.header
    }

    fn inode(&self, id: NodeId) -> Result<Inode> {
        let index = u16::try_from(id)
            .ok()
            .and_then(|number| number.checked_sub(ROOT))
            .ok_or(FoldError::NotFound)?;
        self.inodes
            .get(usize::from(index))
            .copied()
            .flatten()
            .ok_or(FoldError::NotFound)
    }

    fn read_inode(&mut self, inode: Inode, offset: u64, bytes: &mut [u8]) -> Result<()> {
        require(
            offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= inode.size),
            "UnixWare BFS read exceeds logical EOF",
        )?;
        if !bytes.is_empty() {
            self.source.read_at(inode.offset + offset, bytes)?;
        }
        Ok(())
    }
}

impl<S: BlockSource> FoldFrontend<S> for UnixwareBfs<S> {
    const TAG: &'static str = "unixwarebfs";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(Header::read(source)?.is_some())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header = Header::read(&mut source)?
            .ok_or(FoldError::Corrupt("UnixWare BFS signature absent"))?;
        if header.interrupted_compaction {
            return Err(FoldError::Unsupported(
                "UnixWare BFS interrupted compaction",
            ));
        }
        let mut table = vec![0; header.inode_count * INODE_SIZE];
        source.read_at(BLOCK, &mut table)?;
        let inodes = table
            .as_chunks::<INODE_SIZE>()
            .0
            .iter()
            .enumerate()
            .map(|(index, bytes)| Inode::parse(bytes, index, &header))
            .collect::<Result<Vec<_>>>()?;
        let mut extents = inodes
            .iter()
            .flatten()
            .filter(|inode| inode.offset != 0)
            .collect::<Vec<_>>();
        extents.sort_unstable_by_key(|inode| inode.offset);
        require(
            extents
                .windows(2)
                .all(|pair| pair[0].allocated_end <= pair[1].offset),
            "UnixWare BFS overlapping inode allocations",
        )?;
        let fs = Self {
            source,
            header,
            inodes,
        };
        let root = fs.inode(u64::from(ROOT))?;
        require(
            root.kind == FileKind::Directory && root.size >= 32 && root.size.is_multiple_of(16),
            "UnixWare BFS invalid root directory size",
        )?;
        Ok(fs)
    }

    fn root(&self) -> NodeId {
        u64::from(ROOT)
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        _: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .directory(dir)?
            .into_iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.node))
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.directory(dir)
    }

    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.inode(node)?;
        Ok(Metadata {
            size: inode.size,
            kind: inode.kind,
            mode: inode.mode,
        })
    }

    fn timestamps(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        Ok(self.inode(node)?.times)
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.inode(node)?;
        if inode.kind == FileKind::Directory {
            return Err(FoldError::IsDirectory);
        }
        let count = inode.size.saturating_sub(offset).min(bytes.len() as u64) as usize;
        if count != 0 {
            self.read_inode(inode, offset, &mut bytes[..count])?;
        }
        Ok(count)
    }
}

fn require(condition: bool, detail: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(detail))
    }
}

fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
