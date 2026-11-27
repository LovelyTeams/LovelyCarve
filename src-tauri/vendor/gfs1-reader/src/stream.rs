use std::collections::{BTreeMap, HashSet};

use lamfold::{BlockSource, FileKind, FoldError, Result};

use crate::allocation::Allocation;
use crate::format::{Header, INDIRECT_BYTES, INODE_BYTES, Inode, META_BYTES, meta, require, u64be};

pub(crate) struct Stream {
    size: u64,
    block_size: u64,
    journaled: bool,
    inline: Option<Vec<u8>>,
    allocated: BTreeMap<u64, u64>,
    pub owned: HashSet<u64>,
}

impl Stream {
    pub fn parse(
        source: &mut impl BlockSource,
        header: &Header,
        inode: &Inode,
        allocation: Option<&mut Allocation>,
    ) -> Result<Self> {
        let journaled = inode.flags & 1 != 0;
        let block_size =
            u64::from(header.block_size) - if journaled { META_BYTES as u64 } else { 0 };
        let mut stream = Self {
            size: inode.size,
            block_size,
            journaled,
            inline: None,
            allocated: BTreeMap::new(),
            owned: HashSet::from([inode.number.block]),
        };
        if inode.height == 0 {
            stream.inline =
                Some(inode.raw[INODE_BYTES..INODE_BYTES + inode.size as usize].to_vec());
        } else {
            Mapper {
                source,
                header,
                stream: &mut stream,
                allocation,
                dense: inode.kind == FileKind::Directory,
                max_blocks: inode.blocks,
            }
            .walk(
                &inode.raw[INODE_BYTES..],
                inode.height,
                0,
                inode.size.div_ceil(block_size),
            )?;
        }
        Ok(stream)
    }

    pub fn complete(&self) -> bool {
        self.inline.is_some() || self.allocated.len() as u64 == self.size.div_ceil(self.block_size)
    }

    pub fn read(
        &self,
        source: &mut impl BlockSource,
        header: &Header,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        let count = self.size.saturating_sub(offset).min(bytes.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        if let Some(inline) = &self.inline {
            bytes[..count].copy_from_slice(&inline[offset as usize..offset as usize + count]);
            return Ok(count);
        }
        let end = offset + count as u64;
        bytes[..count].fill(0);
        for (&logical, &physical) in self
            .allocated
            .range(offset / self.block_size..=(end - 1) / self.block_size)
        {
            let start = logical * self.block_size;
            let first = offset.max(start);
            let last = end.min(start + self.block_size);
            let output = &mut bytes[(first - offset) as usize..(last - offset) as usize];
            if self.journaled {
                let raw = header.read_block(source, physical)?;
                meta(&raw, 7, 700)?;
                let begin = META_BYTES + (first - start) as usize;
                output.copy_from_slice(&raw[begin..begin + output.len()]);
            } else {
                source.read_at(header.block_offset(physical)? + (first - start), output)?;
            }
        }
        Ok(count)
    }
}

struct Mapper<'a, S> {
    source: &'a mut S,
    header: &'a Header,
    stream: &'a mut Stream,
    allocation: Option<&'a mut Allocation>,
    dense: bool,
    max_blocks: u64,
}

impl<S: BlockSource> Mapper<'_, S> {
    fn walk(
        &mut self,
        raw: &[u8],
        height: u16,
        mut logical: u64,
        mut remaining: u64,
    ) -> Result<()> {
        let fanout = (u64::from(self.header.block_size) - INDIRECT_BYTES as u64) / 8;
        let span = fanout.saturating_pow(u32::from(height) - 1);
        require(
            remaining <= (raw.len() as u64 / 8).saturating_mul(span),
            "GFS1 file length exceeds pointer tree capacity",
        )?;
        for pointer in raw.as_chunks::<8>().0 {
            if remaining == 0 {
                break;
            }
            let take = remaining.min(span);
            let block = u64be(pointer, 0)?;
            if block != 0 {
                self.header.block_offset(block)?;
                require(
                    self.stream.owned.len() < 4_194_304
                        && (self.stream.owned.len() as u64) < self.max_blocks
                        && self.stream.owned.insert(block),
                    "duplicate or excessive GFS1 pointer blocks",
                )?;
                if let Some(allocation) = self.allocation.as_deref_mut() {
                    allocation.check(
                        self.source,
                        self.header,
                        block,
                        height > 1 || self.stream.journaled,
                    )?;
                }
                if height == 1 {
                    self.stream.allocated.insert(logical, block);
                } else {
                    let ind = self.header.read_block(self.source, block)?;
                    meta(&ind, 5, 500)?;
                    self.walk(&ind[INDIRECT_BYTES..], height - 1, logical, take)?;
                }
            } else if self.dense {
                return Err(FoldError::Corrupt("GFS1 directory table contains a hole"));
            }
            remaining -= take;
            logical += take;
        }
        Ok(())
    }
}
