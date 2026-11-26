//! Bootstrap the structural fileset, then resolve user inodes inside its list.

use std::collections::{HashMap, HashSet};

use lamfold::{BlockSource, FileKind, FoldError, Result};

use crate::allocation::Allocation;
use crate::{
    FILESET_HEADER, Header, INODE_LIST, INODE_SIZE, INODE_TYPE_MASK, Inode, ROOT, require,
};

const OLT_MAGIC: u32 = 0xa504fcf5;
const MAX_OLT_BYTES: u64 = 16 * 1024 * 1024;
const INODE_CACHE_LIMIT: usize = 128;

pub(crate) struct MappedInode {
    pub inode: Inode,
    pub allocation: Allocation,
}

pub struct Vxfs<S> {
    source: S,
    pub(crate) header: Header,
    primary: Option<MappedInode>,
    cached_inodes: HashMap<u32, Inode>,
    last_file_map: Option<(u64, Allocation)>,
}

impl<S: BlockSource> Vxfs<S> {
    pub fn open_volume(mut source: S) -> Result<Self> {
        let header = Header::read(&mut source)?;
        if !(2..=4).contains(&header.layout) {
            return Err(FoldError::Unsupported(
                "VxFS disk layout beyond versions 2–4",
            ));
        }
        let mut volume = Self {
            source,
            header,
            primary: None,
            cached_inodes: HashMap::new(),
            last_file_map: None,
        };
        let (fsh_number, first_inode_block) = volume.object_locations()?;
        let fsh = volume.bootstrap_inode(first_inode_block, fsh_number)?;
        let fsh = volume.map_metadata(fsh, FILESET_HEADER)?;
        let mut structural_head = [0; 64];
        let mut primary_head = [0; 64];
        volume.read_metadata(&fsh, 0, &mut structural_head)?;
        volume.read_metadata(&fsh, volume.header.block_size, &mut primary_head)?;
        let endian = volume.header.endian;
        let structural_list = endian.u32(&structural_head, 48);
        let primary_list = endian.u32(&primary_head, 48);
        require(
            structural_list != 0 && primary_list != 0,
            "VxFS missing fileset inode lists",
        )?;
        let structural = volume.bootstrap_inode(first_inode_block, structural_list)?;
        let structural = volume.map_metadata(structural, INODE_LIST)?;
        let mut raw = [0; INODE_SIZE];
        volume.read_metadata(&structural, inode_offset(primary_list), &mut raw)?;
        volume.primary = Some(volume.map_metadata(Inode::parse(&raw, endian)?, INODE_LIST)?);
        require(
            volume.user_inode(u64::from(ROOT))?.kind() == FileKind::Directory,
            "VxFS root is not a directory",
        )?;
        Ok(volume)
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    fn object_locations(&mut self) -> Result<(u32, u64)> {
        let endian = self.header.endian;
        let mut block = self.header.olt_start;
        let mut blocks = self.header.olt_length;
        let mut found_fileset = None;
        let mut found_list = None;
        let mut visited = HashSet::new();
        let mut read = 0;
        while block != 0 {
            self.check_blocks(block, blocks)?;
            require(
                blocks != 0 && visited.insert(block),
                "VxFS cyclic or empty object location extent",
            )?;
            read += blocks * self.header.block_size;
            require(
                read <= MAX_OLT_BYTES,
                "VxFS object location table work limit",
            )?;
            let bytes = self.read_blocks(block, blocks)?;
            require(
                bytes.len() >= 56 && endian.u32(&bytes, 0) == OLT_MAGIC,
                "VxFS object location table signature",
            )?;
            let mut cursor = endian.u32(&bytes, 4) as usize;
            require(
                (56..=bytes.len()).contains(&cursor) && cursor.is_multiple_of(4),
                "VxFS object location table header size",
            )?;
            while cursor < bytes.len() {
                let tail = &bytes[cursor..];
                if tail.iter().all(|&byte| byte == 0) {
                    break;
                }
                require(tail.len() >= 8, "VxFS truncated object location record")?;
                let kind = endian.u32(tail, 0);
                let length = endian.u32(tail, 4) as usize;
                require(
                    length >= 8 && length <= tail.len() && length.is_multiple_of(4),
                    "VxFS object location record size",
                )?;
                if matches!(kind, 2 | 4) {
                    require(length >= 16, "VxFS short fileset or inode list location")?;
                    let value = endian.u32(tail, 8);
                    let slot = if kind == 2 {
                        &mut found_fileset
                    } else {
                        &mut found_list
                    };
                    require(
                        value != 0 && slot.replace(value).is_none(),
                        "VxFS invalid or duplicate bootstrap location",
                    )?;
                }
                cursor += length;
            }
            block = u64::from(endian.u32(&bytes, 40));
            blocks = u64::from(endian.u32(&bytes, 48));
            require(
                (block == 0) == (blocks == 0),
                "VxFS incomplete object location extent link",
            )?;
        }
        found_fileset
            .zip(found_list.map(u64::from))
            .ok_or(FoldError::Corrupt(
                "VxFS object location table has no fileset or inode list",
            ))
    }

    fn bootstrap_inode(&mut self, block: u64, number: u32) -> Result<Inode> {
        self.check_blocks(block, 1)?;
        let volume_size = self.header.block_size * self.header.blocks;
        let start = block * self.header.block_size + inode_offset(number);
        require(
            start
                .checked_add(INODE_SIZE as u64)
                .is_some_and(|end| end <= volume_size),
            "VxFS bootstrap inode exceeds volume",
        )?;
        let mut raw = [0; INODE_SIZE];
        self.source.read_at(start, &mut raw)?;
        Inode::parse(&raw, self.header.endian)
    }

    fn map_metadata(&mut self, inode: Inode, kind: u32) -> Result<MappedInode> {
        require(
            inode.mode & INODE_TYPE_MASK == kind,
            "VxFS structural inode type mismatch",
        )?;
        let allocation = self.allocation(&inode)?;
        Ok(MappedInode { inode, allocation })
    }

    fn read_metadata(
        &mut self,
        mapped: &MappedInode,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<()> {
        mapped.allocation.read(
            &mut self.source,
            &mapped.inode,
            self.header.block_size,
            offset,
            buffer,
            false,
        )
    }

    pub(crate) fn user_inode(&mut self, number: u64) -> Result<Inode> {
        let number = u32::try_from(number)
            .ok()
            .filter(|&value| value >= ROOT)
            .ok_or(FoldError::NotFound)?;
        if let Some(inode) = self.cached_inodes.get(&number) {
            return Ok(inode.clone());
        }
        let mut raw = [0; INODE_SIZE];
        let primary = self.primary.as_ref().unwrap();
        primary.allocation.read(
            &mut self.source,
            &primary.inode,
            self.header.block_size,
            inode_offset(number),
            &mut raw,
            false,
        )?;
        let inode = Inode::parse(&raw, self.header.endian)?;
        require(
            inode.mode != 0 && inode.links != 0,
            "VxFS directory references an unallocated inode",
        )?;
        if self.cached_inodes.len() == INODE_CACHE_LIMIT {
            self.cached_inodes.clear();
        }
        self.cached_inodes.insert(number, inode.clone());
        Ok(inode)
    }

    pub(crate) fn read_user_bytes(
        &mut self,
        number: u64,
        inode: &Inode,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<()> {
        if self
            .last_file_map
            .as_ref()
            .is_none_or(|(node, _)| *node != number)
        {
            self.last_file_map = Some((number, self.allocation(inode)?));
        }
        let (_, allocation) = self.last_file_map.as_ref().unwrap();
        allocation.read(
            &mut self.source,
            inode,
            self.header.block_size,
            offset,
            buffer,
            inode.kind() == FileKind::Regular,
        )
    }

    pub(crate) fn check_blocks(&self, block: u64, count: u64) -> Result<()> {
        require(
            block
                .checked_add(count)
                .is_some_and(|end| end <= self.header.blocks),
            "VxFS block extent exceeds volume",
        )
    }

    pub(crate) fn read_blocks(&mut self, block: u64, count: u64) -> Result<Vec<u8>> {
        self.check_blocks(block, count)?;
        let size = self.header.block_size;
        let mut bytes = vec![0; (size * count) as usize];
        self.source.read_at(block * size, &mut bytes)?;
        Ok(bytes)
    }
}

fn inode_offset(number: u32) -> u64 {
    u64::from(number) * INODE_SIZE as u64
}
