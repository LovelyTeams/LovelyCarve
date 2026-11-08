//! Physical log proofs, DAT translation and bounded seekable metadata access.
use crate::{
    allocation::Position,
    crc,
    header::Header,
    inode::Inode,
    le16, le32, le64,
    logs::{Logs, Payload},
    mapping::{Node, Root},
    require,
};
use lamfold::{BlockSource, FileKind, FoldError, Result};
use std::collections::{HashSet, VecDeque};

pub(crate) struct Storage<S: BlockSource> {
    pub source: S,
    pub header: Header,
    dat: Inode,
    logs: Logs,
    cache: VecDeque<(u64, Vec<u8>)>,
    checked_groups: VecDeque<(u64, u64, u64)>,
    usage: Option<Inode>,
    checked_segments: VecDeque<u64>,
}
impl<S: BlockSource> Storage<S> {
    pub fn open(mut source: S, header: Header) -> Result<(Self, Inode, Inode)> {
        let mut logs = Logs::new();
        let root = logs.latest_root(&mut source, &header)?;
        let raw = header.read_block(&mut source, root)?;
        let bytes = le16(&raw, 4)? as usize;
        require(
            bytes == 16 + header.inode_size * 3 && le16(&raw, 6)? == 0,
            "nilfs2: super-root size/flags",
        )?;
        require(
            crc(header.crc_seed, &raw[4..bytes]) == le32(&raw, 0)?,
            "nilfs2: super-root CRC mismatch",
        )?;
        let mut inodes = Vec::new();
        for index in 0..3 {
            let start = 16 + index * header.inode_size;
            let inode = Inode::parse(&raw[start..start + header.inode_size])?;
            require(
                inode.kind() == FileKind::Regular,
                "nilfs2: root metadata inode kind",
            )?;
            inodes.push(inode);
        }
        let usage = inodes
            .pop()
            .ok_or(FoldError::Corrupt("nilfs2: missing sufile"))?;
        let checkpoints = inodes
            .pop()
            .ok_or(FoldError::Corrupt("nilfs2: missing cpfile"))?;
        let dat = inodes
            .pop()
            .ok_or(FoldError::Corrupt("nilfs2: missing DAT"))?;
        Ok((
            Self {
                source,
                header,
                dat,
                logs,
                cache: VecDeque::new(),
                checked_groups: VecDeque::new(),
                usage: Some(usage.clone()),
                checked_segments: VecDeque::new(),
            },
            checkpoints,
            usage,
        ))
    }
    fn raw(&mut self, block: u64, identity: Payload) -> Result<Vec<u8>> {
        require(
            self.logs.identity(&mut self.source, &self.header, block)? == identity,
            "nilfs2: log payload inode/virtual/logical identity mismatch",
        )?;
        let segment = block / self.header.blocks_per_segment as u64;
        if !self.checked_segments.contains(&segment) {
            // The sufile is itself addressed through DAT. Temporarily taking it
            // bounds the bootstrap recursion; its bytes still require log proofs.
            if let Some(usage) = self.usage.take() {
                let checked = self.segment(&usage, segment);
                self.usage = Some(usage);
                checked?;
                if self.checked_segments.len() == 128 {
                    self.checked_segments.pop_front();
                }
                self.checked_segments.push_back(segment);
            }
        }
        if let Some((_, bytes)) = self.cache.iter().find(|entry| entry.0 == block) {
            return Ok(bytes.clone());
        }
        let raw = self.header.read_block(&mut self.source, block)?;
        if self.cache.len() == 64 {
            self.cache.pop_front();
        }
        self.cache.push_back((block, raw.clone()));
        Ok(raw)
    }
    fn segment(&mut self, usage: &Inode, segment: u64) -> Result<()> {
        let size = self.header.segment_usage_size;
        let index = segment
            .checked_add(24usize.div_ceil(size) as u64)
            .ok_or(FoldError::Corrupt("nilfs2: segment usage index overflow"))?;
        let per_block = self.header.block_size as u64 / size as u64;
        let raw = self.required_block(usage, 5, index / per_block, self.header.last_checkpoint)?;
        let offset = (index % per_block) as usize * size;
        let blocks = le32(&raw, offset + 8)?;
        let flags = le32(&raw, offset + 12)?;
        require(
            flags & !7 == 0
                && flags & 2 != 0
                && flags & 4 == 0
                && blocks != 0
                && blocks <= self.header.blocks_per_segment,
            "nilfs2: referenced segment is free, erroneous or has invalid usage",
        )
    }
    fn translate(&mut self, virtual_block: u64, checkpoint: u64) -> Result<u64> {
        require(virtual_block != 0, "nilfs2: invalid virtual block zero")?;
        let dat = self.dat.clone();
        let entry = self.entry(
            &dat,
            3,
            virtual_block,
            self.header.dat_entry_size,
            self.header.last_checkpoint,
        )?;
        let physical = le64(&entry, 0)?;
        let first = le64(&entry, 8)?;
        let end = le64(&entry, 16)?;
        require(
            first != 0 && first <= checkpoint && checkpoint < end,
            "nilfs2: DAT checkpoint lifetime mismatch",
        )?;
        self.header.block_offset(physical)?;
        Ok(physical)
    }
    fn physical(&mut self, owner: u64, pointer: u64, checkpoint: u64) -> Result<u64> {
        if owner == 3 {
            self.header.block_offset(pointer)?;
            Ok(pointer)
        } else {
            self.translate(pointer, checkpoint)
        }
    }
    fn pointer(
        &mut self,
        inode: &Inode,
        owner: u64,
        logical: u64,
        checkpoint: u64,
    ) -> Result<Option<u64>> {
        match Root::parse(&inode.raw)? {
            Root::Direct(pointers) => Ok(if logical < 6 {
                (pointers[logical as usize] != 0).then_some(pointers[logical as usize])
            } else {
                None
            }),
            Root::Tree(mut node) => {
                let mut visited = HashSet::new();
                loop {
                    let Some((lower, pointer, upper)) = node.child(logical) else {
                        return Ok(None);
                    };
                    if node.level == 1 {
                        return Ok((lower == logical).then_some(pointer));
                    }
                    let block = self.physical(owner, pointer, checkpoint)?;
                    require(visited.insert(block), "nilfs2: B-tree pointer cycle")?;
                    let level = node.level - 1;
                    let expected = if owner == 3 {
                        Payload::DatNode { key: lower, level }
                    } else {
                        Payload::Node {
                            owner,
                            virtual_block: pointer,
                        }
                    };
                    let raw = self.raw(block, expected)?;
                    node = Node::parse(&raw, false, Some(level))?;
                    require(
                        node.keys.first() == Some(&lower)
                            && upper.is_none_or(|upper| {
                                node.keys.last().is_some_and(|last| *last < upper)
                            }),
                        "nilfs2: B-tree parent separator bounds",
                    )?;
                }
            }
        }
    }
    pub fn block(
        &mut self,
        inode: &Inode,
        owner: u64,
        logical: u64,
        checkpoint: u64,
    ) -> Result<Option<Vec<u8>>> {
        let Some(pointer) = self.pointer(inode, owner, logical, checkpoint)? else {
            return Ok(None);
        };
        let physical = self.physical(owner, pointer, checkpoint)?;
        let expected = if owner == 3 {
            Payload::DatData { logical }
        } else {
            Payload::Data {
                owner,
                virtual_block: pointer,
                logical,
            }
        };
        Ok(Some(self.raw(physical, expected)?))
    }
    pub fn required_block(
        &mut self,
        inode: &Inode,
        owner: u64,
        logical: u64,
        checkpoint: u64,
    ) -> Result<Vec<u8>> {
        self.block(inode, owner, logical, checkpoint)?
            .ok_or(FoldError::Corrupt(
                "nilfs2: missing allocated metadata/directory block",
            ))
    }
    pub fn entry(
        &mut self,
        inode: &Inode,
        owner: u64,
        number: u64,
        entry_size: usize,
        checkpoint: u64,
    ) -> Result<Vec<u8>> {
        let position = Position::of(number, self.header.block_size, entry_size)?;
        let bitmap = self.required_block(inode, owner, position.bitmap, checkpoint)?;
        require(
            bitmap[position.bit / 8] & (1 << (position.bit % 8)) != 0,
            "nilfs2: unallocated inode/virtual entry",
        )?;
        let group = number / (self.header.block_size as u64 * 8);
        let cache_key = (owner, checkpoint, group);
        if !self.checked_groups.contains(&cache_key) {
            let descriptor = self.required_block(inode, owner, position.descriptor, checkpoint)?;
            let allocated: u32 = bitmap.iter().map(|byte| byte.count_ones()).sum();
            require(
                le32(&descriptor, position.descriptor_offset)? as u64 + allocated as u64
                    == self.header.block_size as u64 * 8,
                "nilfs2: allocator bitmap/free-count mismatch",
            )?;
            if self.checked_groups.len() == 128 {
                self.checked_groups.pop_front();
            }
            self.checked_groups.push_back(cache_key);
        }
        let raw = self.required_block(inode, owner, position.entry, checkpoint)?;
        Ok(raw[position.entry_offset..position.entry_offset + entry_size].to_vec())
    }
    pub fn read(
        &mut self,
        inode: &Inode,
        owner: u64,
        checkpoint: u64,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize> {
        require(
            matches!(
                inode.kind(),
                FileKind::Regular | FileKind::Symlink | FileKind::Directory
            ),
            "nilfs2: content read on special inode",
        )?;
        let size = output
            .len()
            .min(usize::try_from(inode.size.saturating_sub(offset)).unwrap_or(usize::MAX));
        let mut done = 0;
        while done < size {
            let position = offset
                .checked_add(done as u64)
                .ok_or(FoldError::Corrupt("nilfs2: logical read overflow"))?;
            let logical = position / self.header.block_size as u64;
            let within = (position % self.header.block_size as u64) as usize;
            let take = (size - done).min(self.header.block_size as usize - within);
            match self.block(inode, owner, logical, checkpoint)? {
                Some(raw) => output[done..done + take].copy_from_slice(&raw[within..within + take]),
                None => {
                    require(
                        inode.kind() == FileKind::Regular,
                        "nilfs2: sparse directory/symlink",
                    )?;
                    output[done..done + take].fill(0);
                }
            }
            done += take;
        }
        Ok(done)
    }
}
