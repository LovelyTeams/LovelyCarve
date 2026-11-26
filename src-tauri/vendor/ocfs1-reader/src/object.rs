use crate::{
    format::{DIRECTORY_BYTES, SECTOR_BYTES},
    le16, le32, le64, require,
};
use lamfold::{FileKind, FileTimes, FoldError, Metadata, Result};

const VALID: u32 = 1;
const CHANGE: u32 = 2;
const DELETED: u32 = 4 | 8;

pub(crate) struct DirectoryNode {
    pub(crate) next: Option<u64>,
    pub(crate) first: bool,
    pub(crate) offsets: Vec<u64>,
}

impl DirectoryNode {
    pub(crate) fn parse(bytes: &[u8; SECTOR_BYTES], address: u64) -> Result<Self> {
        require(
            &bytes[48..56] == b"DIRNV20\0" && le64(bytes, 76)? == address,
            "ocfs1: invalid directory node identity",
        )?;
        let node_flags = bytes[121];
        let capacity = usize::from(bytes[118]);
        let used = usize::from(bytes[120]);
        require(
            node_flags & !1 == 0 && bytes[379] == 0 && capacity > 0 && used <= capacity,
            "ocfs1: invalid directory index state",
        )?;
        let mut seen = [false; 255];
        let mut offsets = Vec::with_capacity(used);
        for &slot in &bytes[123..123 + used] {
            let slot = usize::from(slot);
            require(
                slot < capacity && !seen[slot],
                "ocfs1: invalid or repeated directory slot",
            )?;
            seen[slot] = true;
            let delta = (slot as u64 + 1) * SECTOR_BYTES as u64;
            require(
                delta < DIRECTORY_BYTES,
                "ocfs1: directory slot outside its node",
            )?;
            offsets.push(
                address
                    .checked_add(delta)
                    .ok_or(FoldError::Corrupt("ocfs1: directory slot offset overflow"))?,
            );
        }
        let next = match le64(bytes, 84)? {
            u64::MAX => None,
            address => Some(address),
        };
        Ok(Self {
            next,
            first: node_flags & 1 != 0,
            offsets,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Extent {
    pub(crate) logical: u64,
    pub(crate) bytes: u64,
    pub(crate) physical: u64,
}

impl Extent {
    pub(crate) fn parse(bytes: &[u8], offset: usize, count: usize) -> Result<Vec<Self>> {
        (0..count)
            .map(|index| {
                let offset = offset + index * 24;
                Ok(Self {
                    logical: le64(bytes, offset)?,
                    bytes: le64(bytes, offset + 8)?,
                    physical: le64(bytes, offset + 16)?,
                })
            })
            .collect()
    }

    pub(crate) fn end(self) -> Result<u64> {
        self.logical
            .checked_add(self.bytes)
            .ok_or(FoldError::Corrupt("ocfs1: logical extent overflow"))
    }
}

#[derive(Clone)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) metadata: Metadata,
    pub(crate) times: FileTimes,
    pub(crate) parent_node: u64,
    pub(crate) allocation: u64,
    pub(crate) last_leaf: u64,
    pub(crate) depth: Option<u8>,
    pub(crate) extents: Vec<Extent>,
}

impl Entry {
    pub(crate) fn parse(bytes: &[u8; SECTOR_BYTES], address: u64) -> Result<Option<Self>> {
        let flags = le32(bytes, 456)?;
        if flags & VALID == 0 || flags & DELETED != 0 {
            return Ok(None);
        }
        require(
            &bytes[48..52] == b"FIL\0" && le64(bytes, 440)? == address,
            "ocfs1: invalid file entry identity",
        )?;
        if flags & CHANGE != 0 || flags & !(VALID | CHANGE | DELETED) != 0 {
            return Err(FoldError::Unsupported("ocfs1: unstable file entry"));
        }
        let name_len = usize::from(le16(bytes, 324)?);
        require((1..=255).contains(&name_len), "ocfs1: invalid name length")?;
        let name = &bytes[68..68 + name_len];
        require(
            !name.contains(&0) && !name.contains(&b'/') && name != b"." && name != b"..",
            "ocfs1: invalid filename",
        )?;
        let name = std::str::from_utf8(name)
            .map_err(|_| FoldError::Unsupported("ocfs1: non-UTF-8 filename"))?
            .to_owned();
        let (kind, mode_type) = match le32(bytes, 464)? {
            0x1 => (FileKind::Directory, 0o040000),
            0x40 => (FileKind::Regular, 0o100000),
            0x100 => (FileKind::Symlink, 0o120000),
            0x10 => (FileKind::Other, 0o020000),
            0x20 => (FileKind::Other, 0o060000),
            0x80 => (FileKind::Other, 0o010000),
            0x200 => (FileKind::Other, 0o140000),
            _ => return Err(FoldError::Unsupported("ocfs1: file entry attribute")),
        };
        let size = le64(bytes, 328)?;
        let allocation = le64(bytes, 336)?;
        require(
            kind == FileKind::Directory || size <= allocation,
            "ocfs1: file size exceeds allocation",
        )?;
        let count = if kind == FileKind::Directory {
            1
        } else {
            usize::from(bytes[60])
        };
        require(count <= 3, "ocfs1: too many inode extents")?;
        let level = le32(bytes, 64)?;
        let depth = match (le32(bytes, 56)?, level) {
            (1, u32::MAX) => None,
            (0, 0..=4) => Some(level as u8),
            _ => return Err(FoldError::Unsupported("ocfs1: extent tree layout")),
        };
        let times = FileTimes {
            modified: Some((le64(bytes, 352)? as i64, 0)),
            created: Some((le64(bytes, 344)? as i64, 0)),
            ..FileTimes::default()
        };
        let metadata = Metadata {
            kind,
            size,
            mode: mode_type | (le32(bytes, 468)? & 0o7777),
        };
        Ok(Some(Self {
            name,
            metadata,
            times,
            parent_node: le64(bytes, 432)?,
            allocation,
            last_leaf: le64(bytes, 448)?,
            depth,
            extents: Extent::parse(bytes, 360, count)?,
        }))
    }

    pub(crate) fn directory_head(&self) -> Result<u64> {
        if self.metadata.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        require(
            self.depth.is_none()
                && self.extents.len() == 1
                && self.extents[0].logical == 0
                && self.allocation >= DIRECTORY_BYTES
                && self.extents[0].physical != 0,
            "ocfs1: invalid directory head mapping",
        )?;
        Ok(self.extents[0].physical)
    }
}
