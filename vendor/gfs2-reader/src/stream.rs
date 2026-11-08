//! A constant-height pointer tree: zero pointers are format-defined holes.
//! All referenced metadata remains positioned; ranges never skip file prefixes.
use crate::{exact, header::Header, inode::Inode, meta, require, u64be};
use lamfold::{BlockSource, FileKind, FoldError, Result};
use std::collections::HashSet;

pub(crate) struct Stream {
    pub owner: u64,
    pub size: u64,
    pub blocks: Vec<Option<u64>>,
    pub metadata: HashSet<u64>,
    stuffed: Option<Vec<u8>>,
    directory: bool,
}
impl Stream {
    pub fn parse(source: &mut impl BlockSource, header: &Header, inode: &Inode) -> Result<Self> {
        let mut stream = Self {
            owner: inode.number.address,
            size: inode.size,
            blocks: Vec::new(),
            metadata: HashSet::from([inode.number.address]),
            stuffed: None,
            directory: inode.kind() == FileKind::Directory,
        };
        if inode.height == 0 {
            stream.stuffed = Some(inode.raw[232..232 + inode.size as usize].to_vec());
            return Ok(stream);
        }
        let payload = inode.payload_size(header);
        let count = inode.size.div_ceil(payload);
        require(count <= 4_194_304, "gfs2: logical map exceeds work limit")?;
        let fanout = (header.block_size as u64 - 24) / 8;
        let capacity = ((header.block_size as u64 - 232) / 8)
            .checked_mul(
                fanout
                    .checked_pow(inode.height as u32 - 1)
                    .ok_or(FoldError::Corrupt("gfs2: tree capacity overflow"))?,
            )
            .ok_or(FoldError::Corrupt("gfs2: tree capacity overflow"))?;
        require(
            count <= capacity,
            "gfs2: logical length exceeds tree capacity",
        )?;
        stream.walk(
            source,
            header,
            &inode.raw[232..],
            inode.height,
            count,
            fanout,
        )?;
        require(
            stream.blocks.len() as u64 == count,
            "gfs2: incomplete pointer coverage",
        )?;
        let mut used = stream.metadata.clone();
        for block in stream.blocks.iter().flatten() {
            require(
                used.insert(*block),
                "gfs2: overlapping/cyclic block ownership",
            )?;
        }
        require(
            used.len() as u64 <= inode.blocks,
            "gfs2: allocated map exceeds inode block count",
        )?;
        Ok(stream)
    }
    fn walk(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        pointers: &[u8],
        level: u16,
        count: u64,
        fanout: u64,
    ) -> Result<()> {
        let span = fanout
            .checked_pow(level as u32 - 1)
            .ok_or(FoldError::Corrupt("gfs2: subtree span overflow"))?;
        let mut remaining = count;
        for pointer in pointers.as_chunks::<8>().0 {
            if remaining == 0 {
                break;
            }
            let block = u64be(pointer, 0)?;
            let take = remaining.min(span);
            if block == 0 {
                require(!self.directory, "gfs2: hole in directory data")?;
                self.blocks.resize(self.blocks.len() + take as usize, None);
            } else {
                header.block_offset(block)?;
                if level == 1 {
                    self.blocks.push(Some(block));
                } else {
                    require(
                        self.metadata.len() < 131072 && self.metadata.insert(block),
                        "gfs2: duplicate/excessive pointer metadata",
                    )?;
                    let raw = header.read_block(source, block)?;
                    meta(&raw, 5, 500)?;
                    self.walk(source, header, &raw[24..], level - 1, take, fanout)?;
                }
            }
            remaining -= take;
        }
        require(remaining == 0, "gfs2: pointer array too short")
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
        if let Some(stuffed) = &self.stuffed {
            bytes[..count].copy_from_slice(&stuffed[offset as usize..offset as usize + count]);
            return Ok(count);
        }
        let payload = header.block_size as u64 - if self.directory { 24 } else { 0 };
        let mut copied = 0;
        while copied < count {
            let logical = offset + copied as u64;
            let index = (logical / payload) as usize;
            let within = logical % payload;
            let take = (payload - within).min((count - copied) as u64) as usize;
            match self
                .blocks
                .get(index)
                .ok_or(FoldError::Corrupt("gfs2: missing logical block"))?
            {
                None => bytes[copied..copied + take].fill(0),
                Some(block) => {
                    if self.directory {
                        let raw = header.read_block(source, *block)?;
                        meta(&raw, 7, 700)?;
                        bytes[copied..copied + take].copy_from_slice(
                            &raw[24 + within as usize..24 + within as usize + take],
                        );
                    } else {
                        exact(
                            source,
                            header.block_offset(*block)? + within,
                            &mut bytes[copied..copied + take],
                        )?;
                    }
                }
            }
            copied += take;
        }
        Ok(count)
    }
}
