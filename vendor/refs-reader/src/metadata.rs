// Ported from refsprogs librefs/node.c (Erik Larsson, GPL-2.0-or-later).
use crate::bytes::{slice, u32_at, u64_at};
use crate::{corrupt, BootSector, ReadAt, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeReference {
    pub blocks: [u64; 4],
}

impl NodeReference {
    pub(crate) fn parse(bytes: &[u8], offset: usize, modern: bool) -> Result<Self> {
        let mut blocks = [0; 4];
        let count = if modern { 4 } else { 1 };
        for (index, block) in blocks.iter_mut().take(count).enumerate() {
            *block = u64_at(bytes, offset + index * 8)?;
        }
        if blocks[0] == 0 {
            return Err(corrupt("null metadata node reference"));
        }
        Ok(Self { blocks })
    }

    pub(crate) fn contiguous(first: u64) -> Result<Self> {
        let mut blocks = [0; 4];
        for (index, block) in blocks.iter_mut().enumerate() {
            *block = first
                .checked_add(index as u64)
                .ok_or_else(|| corrupt("node reference overflow"))?;
        }
        Ok(Self { blocks })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub block: u64,
    pub sequence: u64,
    pub roots: Vec<NodeReference>,
}

pub(crate) fn checkpoints(superblock: &[u8], boot: &BootSector) -> Result<Vec<u64>> {
    if boot.modern() && slice(superblock, 0, 4)? != b"SUPB" {
        return Err(corrupt("invalid superblock signature"));
    }
    let base = if boot.modern() { 0x70 } else { 0x50 };
    let offset = u32_at(superblock, base)? as usize;
    let count = u32_at(superblock, base + 4)? as usize;
    if count < 2 {
        return Err(corrupt("superblock has fewer than two checkpoints"));
    }
    let bytes = slice(
        superblock,
        offset,
        count
            .checked_mul(8)
            .ok_or_else(|| corrupt("checkpoint list overflow"))?,
    )?;
    bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|value| u64_at(value, 0))
        .collect()
}

impl Checkpoint {
    pub(crate) fn parse(bytes: &[u8], block: u64, boot: &BootSector) -> Result<Self> {
        validate_header(bytes, block, boot, b"CHKP")?;
        let header = if boot.modern() { 0x20 } else { 0 };
        let sequence = u64_at(bytes, header + 0x40)?;
        let list = if boot.modern() { 0x90 } else { 0x58 };
        let count = u32_at(bytes, list)? as usize;
        // 3.14 stores the offsets-array position explicitly. SHA-256 node
        // references have larger trailers than CRC64 references, so a fixed
        // five-word inset (used by C) incorrectly reads the 64 KiB fixture.
        let offsets_start = if (boot.major, boot.minor) >= (3, 14) {
            u32_at(bytes, list + 4)? as usize
        } else {
            list + 4
        };
        if offsets_start < list + 4 {
            return Err(corrupt("checkpoint offsets overlap root count"));
        }
        let offsets = slice(
            bytes,
            offsets_start,
            count
                .checked_mul(4)
                .ok_or_else(|| corrupt("checkpoint root count overflow"))?,
        )?;
        let mut roots = Vec::with_capacity(count);
        for value in offsets.as_chunks::<4>().0 {
            let offset = u32_at(value, 0)? as usize;
            roots.push(NodeReference::parse(bytes, offset, boot.modern())?);
        }
        if roots.is_empty() {
            return Err(corrupt("empty checkpoint root list"));
        }
        Ok(Self {
            block,
            sequence,
            roots,
        })
    }
}

pub(crate) fn validate_header(
    bytes: &[u8],
    block: u64,
    boot: &BootSector,
    signature: &[u8; 4],
) -> Result<u64> {
    let header = if boot.modern() { 0x20 } else { 0 };
    if boot.modern() && slice(bytes, 0, 4)? != signature {
        return Err(corrupt(format!(
            "invalid metadata signature at block {block}"
        )));
    }
    if u64_at(bytes, header)? != block {
        return Err(corrupt(format!(
            "metadata block identity mismatch at {block}"
        )));
    }
    u64_at(bytes, if boot.modern() { 0x48 } else { 0x18 })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BlockRange {
    pub start: u64,
    pub length: u64,
}

pub(crate) fn physical_block(
    boot: &BootSector,
    mappings: &[BlockRange],
    logical: u64,
) -> Result<u64> {
    let (linear, chunk) = if boot.cluster_size == 4096 {
        (65536, 16384)
    } else {
        (4096, 1024)
    };
    if !boot.modern() || logical < linear {
        return Ok(logical);
    }
    let index = (logical / chunk / 2)
        .checked_sub(2)
        .and_then(|value| value.checked_mul(chunk))
        .ok_or_else(|| corrupt("virtual block index overflow"))?;
    let mut end = 0u64;
    for range in mappings {
        end = end
            .checked_add(range.length)
            .ok_or_else(|| corrupt("mapping range overflow"))?;
        if index < end {
            return Ok(range.start | (logical & (chunk - 1)));
        }
    }
    Err(corrupt(format!("unmapped virtual block {logical:#x}")))
}

pub(crate) fn read_node(
    source: &dyn ReadAt,
    boot: &BootSector,
    mappings: &[BlockRange],
    reference: NodeReference,
) -> Result<Vec<u8>> {
    read_node_addressed(source, boot, reference, Addressing::Virtual(mappings))
}

pub(crate) fn read_physical_node(
    source: &dyn ReadAt,
    boot: &BootSector,
    reference: NodeReference,
) -> Result<Vec<u8>> {
    read_node_addressed(source, boot, reference, Addressing::Physical)
}

enum Addressing<'a> {
    Physical,
    Virtual(&'a [BlockRange]),
}

fn read_node_addressed(
    source: &dyn ReadAt,
    boot: &BootSector,
    reference: NodeReference,
    addressing: Addressing<'_>,
) -> Result<Vec<u8>> {
    let mut blocks = reference.blocks;
    let mut bytes = vec![0; boot.node_size()];
    let piece_size = if boot.modern() {
        (boot.cluster_size as usize).min(bytes.len())
    } else {
        bytes.len()
    };
    for index in 0..bytes.len() / piece_size {
        if blocks[index] == 0 {
            if !boot.modern() || index == 0 {
                return Err(corrupt("missing node block"));
            }
            blocks[index] = u64_at(&bytes, 0x20 + index * 8)?;
            if blocks[index] == 0 {
                return Err(corrupt("null continuation block"));
            }
        }
        let physical = match addressing {
            Addressing::Physical => blocks[index],
            Addressing::Virtual(mappings) => physical_block(boot, mappings, blocks[index])?,
        };
        let offset = physical
            .checked_mul(boot.block_unit())
            .ok_or_else(|| corrupt("metadata offset overflow"))?;
        let end = offset
            .checked_add(piece_size as u64)
            .ok_or_else(|| corrupt("metadata range overflow"))?;
        if end > boot.volume_size {
            return Err(corrupt("metadata read exceeds volume"));
        }
        source.read_exact_at(
            offset,
            &mut bytes[index * piece_size..(index + 1) * piece_size],
        )?;
    }
    Ok(bytes)
}
