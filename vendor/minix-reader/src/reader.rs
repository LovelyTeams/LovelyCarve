//! Positioned logical reads and directory traversal over the adapted layouts.
use crate::{exact, metadata::Inode, require, Superblock, Version};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use std::collections::{BTreeMap, BTreeSet};

/// Read-only Minix volume. Handles are native inode numbers; inode and metadata
/// caches are bounded and owned by the reader, with no global state or writes.
pub struct Minix<S: BlockSource> {
    source: S,
    header: Superblock,
    inodes: BTreeMap<u32, Inode>,
    blocks: BTreeMap<u64, Vec<u8>>,
}
impl<S: BlockSource> Minix<S> {
    fn metadata_block(&mut self, block: u64) -> Result<Vec<u8>> {
        if let Some(bytes) = self.blocks.get(&block) {
            return Ok(bytes.clone());
        }
        let mut bytes = vec![0; self.header.block_size as usize];
        let offset = block * u64::from(self.header.block_size);
        require(
            offset + bytes.len() as u64 <= u64::from(self.header.zones) * self.header.zone_size(),
            "minix: metadata outside volume",
        )?;
        exact(&mut self.source, offset, &mut bytes)?;
        if self.blocks.len() >= 256 {
            self.blocks.clear();
        }
        self.blocks.insert(block, bytes.clone());
        Ok(bytes)
    }
    fn allocated_at(&mut self, map_start: u64, bit: u64) -> Result<bool> {
        let byte = map_start + bit / 8;
        let block_size = u64::from(self.header.block_size);
        let block = self.metadata_block(byte / block_size)?;
        Ok(block[(byte % block_size) as usize] & (1 << (bit % 8)) != 0)
    }
    fn inode(&mut self, node: NodeId) -> Result<Inode> {
        let inode_number = u32::try_from(node).map_err(|_| FoldError::NotFound)?;
        require(
            inode_number > 0 && inode_number <= self.header.inodes,
            "minix: inode number outside table",
        )?;
        if let Some(inode) = self.inodes.get(&inode_number) {
            return Ok(inode.clone());
        }
        require(
            self.allocated_at(2 * u64::from(self.header.block_size), node)?,
            "minix: live directory references free inode",
        )?;
        let offset = self.header.inode_table_start() + (node - 1) * self.header.inode_size();
        let block_size = u64::from(self.header.block_size);
        let bytes = self.metadata_block(offset / block_size)?;
        let start = (offset % block_size) as usize;
        let inode = Inode::parse(
            bytes
                .get(start..start + self.header.inode_size() as usize)
                .ok_or(FoldError::Corrupt("minix: inode crosses metadata block"))?,
            &self.header,
        )?;
        if self.inodes.len() >= 4096 {
            self.inodes.clear();
        }
        self.inodes.insert(inode_number, inode.clone());
        Ok(inode)
    }
    fn zone_address(&mut self, zone: u32) -> Result<u64> {
        require(
            zone >= self.header.first_data_zone && zone < self.header.zones,
            "minix: allocated zone outside data area",
        )?;
        let map = (2 + u64::from(self.header.inode_map_blocks)) * u64::from(self.header.block_size);
        require(
            self.allocated_at(map, u64::from(zone - self.header.first_data_zone) + 1)?,
            "minix: allocated pointer references free zone",
        )?;
        Ok(u64::from(zone) * self.header.zone_size())
    }
    fn data_zone(&mut self, inode: &Inode, logical_zone: u64) -> Result<u32> {
        if logical_zone < 7 {
            let zone = inode.zones[logical_zone as usize];
            if zone != 0 {
                self.zone_address(zone)?;
            }
            return Ok(zone);
        }
        let pointers = self.header.pointers_per_block();
        let mut relative = logical_zone - 7;
        let levels = if self.header.version == Version::V1 {
            2
        } else {
            3
        };
        for level in 1..=levels {
            let span = pointers.pow(level);
            if relative >= span {
                relative -= span;
                continue;
            }
            let mut zone = inode.zones[6 + level as usize];
            let mut ancestors = BTreeSet::new();
            for remaining in (1..=level).rev() {
                if zone == 0 {
                    return Ok(0);
                }
                require(ancestors.insert(zone), "minix: indirect zone cycle")?;
                let address = self.zone_address(zone)?;
                // Only the first filesystem block of an indirect zone contains
                // pointers, even when a zone comprises multiple data blocks.
                let bytes = self.metadata_block(address / u64::from(self.header.block_size))?;
                let divisor = pointers.pow(remaining - 1);
                let index = (relative / divisor) as usize;
                relative %= divisor;
                zone = if self.header.version == Version::V1 {
                    u32::from(self.header.endian.u16(&bytes, index * 2)?)
                } else {
                    self.header.endian.u32(&bytes, index * 4)?
                };
            }
            if zone != 0 {
                require(
                    !ancestors.contains(&zone),
                    "minix: file data aliases indirect metadata",
                )?;
                self.zone_address(zone)?;
            }
            return Ok(zone);
        }
        Err(FoldError::Corrupt(
            "minix: logical zone exceeds inode addressing capacity",
        ))
    }
    fn content(&mut self, inode: &Inode, offset: u64, bytes: &mut [u8]) -> Result<usize> {
        let wanted = (bytes.len() as u64).min(inode.metadata.size.saturating_sub(offset)) as usize;
        let zone_size = self.header.zone_size();
        let mut produced = 0;
        while produced < wanted {
            let logical = offset + produced as u64;
            let within = logical % zone_size;
            let count = (zone_size - within).min((wanted - produced) as u64) as usize;
            let zone = self.data_zone(inode, logical / zone_size)?;
            if zone == 0 {
                bytes[produced..produced + count].fill(0);
            } else {
                exact(
                    &mut self.source,
                    u64::from(zone) * zone_size + within,
                    &mut bytes[produced..produced + count],
                )?;
            }
            produced += count;
        }
        Ok(produced)
    }
    fn directory(&mut self, node: NodeId) -> Result<Vec<DirEntry>> {
        let inode = self.inode(node)?;
        if inode.metadata.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let inode_width = self.header.inode_field_size();
        let stride = inode_width + self.header.name_length;
        require(
            inode.metadata.size <= 32 * 1024 * 1024
                && inode.metadata.size.is_multiple_of(stride as u64),
            "minix: directory size/alignment limit",
        )?;
        let mut bytes = vec![0; inode.metadata.size as usize];
        self.content(&inode, 0, &mut bytes)?;
        let mut result = Vec::new();
        let mut names = BTreeSet::new();
        let mut dot = false;
        let mut parent = false;
        for record in bytes.chunks_exact(stride) {
            let child = if inode_width == 2 {
                u32::from(self.header.endian.u16(record, 0)?)
            } else {
                self.header.endian.u32(record, 0)?
            };
            if child == 0 {
                continue;
            } // unused/deleted slot, not end of directory
            require(
                child <= self.header.inodes,
                "minix: directory inode out of bounds",
            )?;
            let raw_name = &record[inode_width..];
            let length = raw_name
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(raw_name.len());
            let name = std::str::from_utf8(&raw_name[..length])
                .map_err(|_| FoldError::Unsupported("minix: non-UTF-8 filename"))?;
            require(
                names.insert(name.to_owned()),
                "minix: duplicate live directory name",
            )?;
            if name == "." {
                require(u64::from(child) == node, "minix: incorrect dot entry")?;
                dot = true;
                continue;
            }
            if name == ".." {
                parent = true;
                continue;
            }
            require(
                !name.is_empty() && !name.contains(['/', '\\']),
                "minix: unsafe directory name",
            )?;
            require(result.len() < 262144, "minix: directory entry limit")?;
            let kind = self.inode(u64::from(child))?.metadata.kind;
            result.push(DirEntry {
                name: name.to_owned(),
                node: u64::from(child),
                kind,
            });
        }
        require(dot && parent, "minix: directory lacks dot/parent entries")?;
        Ok(result)
    }
}
impl<S: BlockSource> FoldFrontend<S> for Minix<S> {
    const TAG: &'static str = "minix";
    fn probe(source: &mut S) -> Result<bool> {
        if source.len() < 1056 {
            return Ok(false);
        }
        let mut bytes = [0; 32];
        exact(source, 1024, &mut bytes)?;
        Ok(Superblock::parse(&bytes)
            .is_ok_and(|header| header.validate_source(source.len()).is_ok()))
    }
    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut bytes = [0; 32];
        exact(&mut source, 1024, &mut bytes)?;
        let header = Superblock::parse(&bytes)?;
        header.validate_source(source.len())?;
        let mut reader = Self {
            source,
            header,
            inodes: BTreeMap::new(),
            blocks: BTreeMap::new(),
        };
        let root = reader.inode(1)?;
        require(
            root.metadata.kind == FileKind::Directory,
            "minix: root is not a directory",
        )?;
        require(
            root.metadata.size
                >= 2 * (reader.header.name_length + reader.header.inode_field_size()) as u64,
            "minix: root is too short",
        )?;
        Ok(reader)
    }
    fn root(&self) -> NodeId {
        1
    }
    fn lookup(
        &mut self,
        directory: NodeId,
        name: &str,
        _: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .directory(directory)?
            .into_iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.node))
    }
    fn read_dir(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.directory(node)
    }
    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        Ok(self.inode(node)?.metadata)
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
        require(
            matches!(inode.metadata.kind, FileKind::Regular | FileKind::Symlink),
            "minix: inode has no file content",
        )?;
        self.content(&inode, offset, bytes)
    }
    fn read_link(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let inode = self.inode(node)?;
        if inode.metadata.kind != FileKind::Symlink {
            return Ok(None);
        }
        require(
            inode.metadata.size <= 65536,
            "minix: symbolic link size limit",
        )?;
        let mut bytes = vec![0; inode.metadata.size as usize];
        self.content(&inode, 0, &mut bytes)?;
        Ok(Some(bytes))
    }
}
