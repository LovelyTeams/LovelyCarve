use std::{collections::HashSet, ops::RangeInclusive};

use lamfold::{BlockSource, FoldError, Result};
use sha2::{Digest, Sha256};

use crate::{
    BLOCK_BYTES, BlockRef, DIRENT_VISIBLE, Inode, Kind, Order, display_name, format::blockset,
    require,
};

pub(crate) struct Volume<S> {
    source: S,
    order: Order,
    length: u64,
}

impl<S: BlockSource> Volume<S> {
    pub fn new(source: S, order: Order, length: u64) -> Self {
        Self {
            source,
            order,
            length,
        }
    }

    fn read_media(&mut self, reference: &BlockRef) -> Result<Vec<u8>> {
        let (at, size) = reference.physical()?;
        require(
            at.checked_add(size as u64)
                .is_some_and(|end| end <= self.length),
            "HAMMER2 allocation outside the volume",
        )?;
        let mut bytes = vec![0; size];
        self.source.read_at(at, &mut bytes)?;
        let valid = match reference.methods >> 4 {
            0 | 1 => true,
            2 => crc32c::crc32c(&bytes) == self.order.u32(&reference.check),
            3 => {
                xxhash_rust::xxh64::xxh64(&bytes, 0x4d61_7474_446c_6c6e)
                    == self.order.u64(&reference.check)
            }
            4 => {
                let mut digest = Sha256::digest(&bytes);
                for i in 16..24 {
                    digest[i] ^= digest[i + 8];
                }
                digest[..24] == reference.check[..24]
            }
            _ => return Err(FoldError::Unsupported("HAMMER2 block checksum method")),
        };
        require(valid, "HAMMER2 block checksum mismatch")?;
        Ok(bytes)
    }

    pub fn inode(&mut self, reference: &BlockRef) -> Result<Inode> {
        require(
            reference.kind == Kind::Inode && reference.key_bits == 0,
            "invalid HAMMER2 inode reference",
        )?;
        Inode::decode(&self.read_media(reference)?, self.order)
    }

    pub fn dirent(&mut self, reference: &BlockRef) -> Result<(String, u64, u8)> {
        let number = self.order.u64(&reference.embed);
        let length = usize::from(self.order.u16(&reference.embed[8..]));
        require(
            reference.kind == Kind::Dirent
                && reference.key_bits == 0
                && number > 0
                && number < DIRENT_VISIBLE
                && (1..=256).contains(&length),
            "invalid HAMMER2 directory record",
        )?;
        let raw = if length <= reference.check.len() {
            require(
                reference.data_offset == 0,
                "HAMMER2 embedded directory record has a data address",
            )?;
            reference.check[..length].to_vec()
        } else {
            let bytes = self.read_media(reference)?;
            bytes
                .get(..length)
                .ok_or(FoldError::Corrupt("truncated HAMMER2 directory name"))?
                .to_vec()
        };
        require(!raw.contains(&0), "HAMMER2 directory name contains NUL")?;
        Ok((display_name(&raw), number, reference.embed[10]))
    }

    pub fn leaves(
        &mut self,
        roots: &[BlockRef],
        wanted: RangeInclusive<u64>,
        kind: Kind,
    ) -> Result<Vec<BlockRef>> {
        self.walk(roots, wanted, |entry| entry == kind)
    }

    pub fn directory_leaves(&mut self, roots: &[BlockRef]) -> Result<Vec<BlockRef>> {
        self.walk(roots, DIRENT_VISIBLE..=u64::MAX, |entry| {
            matches!(entry, Kind::Inode | Kind::Dirent)
        })
    }

    fn walk(
        &mut self,
        roots: &[BlockRef],
        wanted: RangeInclusive<u64>,
        valid_leaf: impl Fn(Kind) -> bool,
    ) -> Result<Vec<BlockRef>> {
        let mut pending = checked_entries(roots.to_vec(), &(0..=u64::MAX))?;
        let mut visited = HashSet::new();
        let mut found = Vec::new();
        while let Some((entry, bounds)) = pending.pop() {
            if bounds.end() < wanted.start() || wanted.end() < bounds.start() {
                continue;
            }
            if entry.kind == Kind::Indirect {
                let physical = entry.physical()?.0;
                require(
                    visited.insert(physical),
                    "cyclic or reused HAMMER2 indirect block",
                )?;
                let bytes = self.read_media(&entry)?;
                let children = checked_entries(blockset(&bytes, self.order)?, &bounds)?;
                pending.extend(children.into_iter().rev());
            } else {
                require(
                    valid_leaf(entry.kind),
                    "HAMMER2 topology has an unexpected leaf kind",
                )?;
                found.push(entry);
            }
        }
        Ok(found)
    }

    pub fn read_file(
        &mut self,
        roots: &[BlockRef],
        file_size: u64,
        offset: u64,
        out: &mut [u8],
    ) -> Result<()> {
        let end = offset + out.len() as u64;
        let data = self.leaves(roots, offset..=end - 1, Kind::Data)?;
        out.fill(0);
        for reference in data {
            let range = reference.range()?;
            require(
                (10..=16).contains(&reference.key_bits),
                "invalid HAMMER2 data block logical size",
            )?;
            let logical_size = 1usize << reference.key_bits;
            let needed = (file_size - reference.key).min(logical_size as u64) as usize;
            let from = offset.max(reference.key);
            let until = (reference.key + needed as u64).min(end);
            let raw = self.read_media(&reference)?;
            let plain = match reference.methods & 15 {
                0 => raw,
                2 => {
                    let length = self.order.u32(&raw) as usize;
                    let data = raw
                        .get(4..4 + length)
                        .ok_or(FoldError::Corrupt("invalid HAMMER2 LZ4 block length"))?;
                    lz4_flex::block::decompress(data, logical_size)
                        .map_err(|_| FoldError::Decompress("HAMMER2 LZ4 block"))?
                }
                3 => miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&raw, logical_size)
                    .map_err(|_| FoldError::Decompress("HAMMER2 zlib block"))?,
                _ => return Err(FoldError::Unsupported("HAMMER2 compression method")),
            };
            require(
                needed <= plain.len()
                    && plain.len() <= logical_size
                    && plain.len() <= BLOCK_BYTES
                    && *range.start() <= from
                    && from < until,
                "HAMMER2 allocated block is shorter than its logical data",
            )?;
            let in_block = (from - reference.key) as usize;
            let in_output = (from - offset) as usize;
            let count = (until - from) as usize;
            out[in_output..in_output + count].copy_from_slice(&plain[in_block..in_block + count]);
        }
        Ok(())
    }
}

fn checked_entries(
    mut entries: Vec<BlockRef>,
    parent: &RangeInclusive<u64>,
) -> Result<Vec<(BlockRef, RangeInclusive<u64>)>> {
    entries.sort_unstable_by_key(|entry| entry.key);
    let mut previous_end = None;
    let mut checked = Vec::with_capacity(entries.len());
    for entry in entries {
        let bounds = entry.range()?;
        require(
            parent.contains(bounds.start())
                && parent.contains(bounds.end())
                && previous_end.is_none_or(|end| end < *bounds.start()),
            "overlapping or out-of-range HAMMER2 blockref keys",
        )?;
        previous_end = Some(*bounds.end());
        checked.push((entry, bounds));
    }
    Ok(checked)
}
