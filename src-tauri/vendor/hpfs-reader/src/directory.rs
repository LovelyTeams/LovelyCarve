use std::collections::HashSet;

use lamfold::{BlockSource, DirEntry, FileKind, FoldError, NodeId, Result};

use crate::{DIRECTORY_SIZE, Hpfs, ROOT, le16, le32, require};

const DNODE_MAGIC: u32 = 0x77e40aae;
const NODE_OFFSET_BITS: u32 = 11;
const FIRST_RECORD: usize = 20;
const MAX_TREE_DEPTH: usize = 32;

#[derive(Clone, Copy)]
pub(crate) struct Record {
    pub fnode: u32,
    pub size: u32,
    pub modified: u32,
    pub accessed: u32,
    pub created: u32,
    attributes: u8,
}

impl Record {
    pub fn kind(&self) -> FileKind {
        if self.attributes & 0x10 != 0 {
            FileKind::Directory
        } else {
            FileKind::Regular
        }
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            fnode: le32(bytes, 4),
            size: le32(bytes, 12),
            modified: le32(bytes, 8),
            accessed: le32(bytes, 16),
            created: le32(bytes, 20),
            attributes: bytes[3],
        }
    }
}

struct DirectorySlot<'a> {
    offset: usize,
    raw: &'a [u8],
    name: &'a [u8],
    child: Option<u32>,
}

impl DirectorySlot<'_> {
    fn special(&self) -> bool {
        self.raw[2] & 0x09 != 0
    }
}

fn directory_slots(bytes: &[u8; DIRECTORY_SIZE], sector: u32) -> Result<Vec<DirectorySlot<'_>>> {
    require(
        le32(bytes, 0) == DNODE_MAGIC && le32(bytes, 16) == sector,
        "HPFS dnode signature or self pointer",
    )?;
    let free = le32(bytes, 4) as usize;
    require(
        (FIRST_RECORD + 32..=DIRECTORY_SIZE).contains(&free) && free.is_multiple_of(4),
        "HPFS dnode record region",
    )?;
    let mut slots = Vec::new();
    let mut offset = FIRST_RECORD;
    while offset < free {
        let header = bytes
            .get(offset..free)
            .filter(|data| data.len() >= 31)
            .ok_or(FoldError::Corrupt("HPFS truncated directory record"))?;
        let length = usize::from(le16(header, 0));
        let end = offset + length;
        let flags = header[2];
        let child_pointer = flags & 4 != 0;
        let name_end = 31 + usize::from(header[30]);
        let required = name_end.next_multiple_of(4) + usize::from(child_pointer) * 4;
        require(
            length >= required && length.is_multiple_of(4) && end <= free && name_end > 31,
            "HPFS directory record length",
        )?;
        let raw = &bytes[offset..end];
        let name = &raw[31..name_end];
        if flags & 8 != 0 {
            require(
                end == free && name == [0xff],
                "HPFS last directory sentinel",
            )?;
        } else if flags & 1 != 0 {
            require(
                offset == FIRST_RECORD && name == [1, 1],
                "HPFS first directory sentinel",
            )?;
        }
        slots.push(DirectorySlot {
            offset,
            raw,
            name,
            child: child_pointer.then(|| le32(raw, length - 4)),
        });
        offset = end;
    }
    require(
        slots.last().is_some_and(|slot| slot.raw[2] & 8 != 0),
        "HPFS missing last directory sentinel",
    )?;
    Ok(slots)
}

impl<S: BlockSource> Hpfs<S> {
    pub(crate) fn root_dnode(&mut self, node: NodeId) -> Result<(u32, u32)> {
        let fnode_sector = if node == ROOT {
            self.header.root
        } else {
            let record = self.record(node)?;
            if record.kind() != FileKind::Directory {
                return Err(FoldError::NotDirectory);
            }
            record.fnode
        };
        let fnode = self.read_fnode(fnode_sector)?;
        require(
            le16(&fnode, 54) & 0x100 != 0 && fnode[56] & 0x80 == 0,
            "HPFS directory fnode does not reference a dnode",
        )?;
        let sector = le32(&fnode, 72);
        require(sector.is_multiple_of(4), "HPFS unaligned directory node")?;
        Ok((sector, fnode_sector))
    }

    pub(crate) fn record(&mut self, node: NodeId) -> Result<Record> {
        if node == ROOT {
            let (mut sector, mut parent) = self.root_dnode(ROOT)?;
            for level in 0..MAX_TREE_DEPTH {
                let bytes = self.sector::<DIRECTORY_SIZE>(sector)?;
                check_parent(&bytes, parent, level == 0)?;
                let slots = directory_slots(&bytes, sector)?;
                let first = &slots[0];
                if first.raw[2] & 1 != 0 {
                    let record = Record::from_bytes(first.raw);
                    require(
                        record.fnode == self.header.root && record.kind() == FileKind::Directory,
                        "HPFS root sentinel identity",
                    )?;
                    return Ok(record);
                }
                parent = sector;
                sector = first.child.ok_or(FoldError::Corrupt(
                    "HPFS root directory has no first record",
                ))?;
            }
            return Err(FoldError::Corrupt("HPFS root directory depth"));
        }
        let sector = u32::try_from(node >> NODE_OFFSET_BITS).map_err(|_| FoldError::NotFound)?;
        let offset = node as usize & (DIRECTORY_SIZE - 1);
        let bytes = self.sector::<DIRECTORY_SIZE>(sector)?;
        let slots = directory_slots(&bytes, sector)?;
        let slot = slots
            .iter()
            .find(|slot| slot.offset == offset && !slot.special())
            .ok_or(FoldError::NotFound)?;
        let record = Record::from_bytes(slot.raw);
        require(
            (20..self.header.sectors).contains(&record.fnode),
            "HPFS directory entry fnode exceeds volume",
        )?;
        Ok(record)
    }

    pub(crate) fn directory(&mut self, node: NodeId) -> Result<Vec<DirEntry>> {
        let (sector, parent) = self.root_dnode(node)?;
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        self.walk_dnodes(sector, parent, 0, &mut visited, &mut result)?;
        let mut names = HashSet::new();
        require(
            result.iter().all(|entry| names.insert(entry.name.as_str())),
            "HPFS duplicate exact directory name",
        )?;
        Ok(result)
    }

    fn walk_dnodes(
        &mut self,
        sector: u32,
        parent: u32,
        level: usize,
        visited: &mut HashSet<u32>,
        result: &mut Vec<DirEntry>,
    ) -> Result<()> {
        require(
            level < MAX_TREE_DEPTH && sector.is_multiple_of(4) && visited.insert(sector),
            "HPFS cyclic, unaligned or excessively deep directory tree",
        )?;
        let bytes = self.sector::<DIRECTORY_SIZE>(sector)?;
        check_parent(&bytes, parent, level == 0)?;
        for slot in directory_slots(&bytes, sector)? {
            if let Some(child) = slot.child {
                self.walk_dnodes(child, sector, level + 1, visited, result)?;
            }
            if slot.special() {
                continue;
            }
            let record = Record::from_bytes(slot.raw);
            require(
                (20..self.header.sectors).contains(&record.fnode),
                "HPFS directory entry fnode exceeds volume",
            )?;
            let name = self.decode_name(slot.name, slot.raw[29])?;
            result.push(DirEntry {
                name,
                kind: record.kind(),
                node: (u64::from(sector) << NODE_OFFSET_BITS) | slot.offset as u64,
            });
        }
        Ok(())
    }
}

fn check_parent(bytes: &[u8; DIRECTORY_SIZE], parent: u32, root: bool) -> Result<()> {
    require(
        (bytes[8] & 1 != 0) == root && le32(bytes, 12) == parent,
        "HPFS dnode parent pointer",
    )
}
