use std::collections::HashSet;

use lamfold::{BlockSource, FoldError, Result};

use crate::format::{DISK_VERSION, MAGIC, Superblock, Tag, TagType};
use crate::metadata::{Block, slots};
use crate::{Pair, normalized, require};

pub const PROBE_BYTES: usize = 4096;
const MIN_BLOCK: u32 = 128;
const MAX_BLOCK: u32 = 65_536;
const ANCHOR: Pair = [0, 1];

pub struct Header {
    pub superblock: Superblock,
    pub anchor_offset: u64,
}

pub(crate) struct Geometry {
    pub superblock: Superblock,
    pub root: Pair,
    pub metadata_blocks: HashSet<u32>,
    pub pending_move: Option<(Pair, usize)>,
}

impl Header {
    pub fn locate(
        head: &[u8],
        length: u64,
        mut read: impl FnMut(u64, &mut [u8]) -> Result<()>,
    ) -> Result<Option<Self>> {
        let mut candidates = candidate_sizes(head, 0, length);
        if candidates.is_empty() {
            let mut alternate = [0; 48];
            for size in [4096, 8192, 16384, 32768, 65536] {
                if length >= 2 * size && read(size, &mut alternate).is_ok() {
                    candidates.extend(candidate_sizes(&alternate, size, length));
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        for size in candidates {
            let mut blocks = vec![0; 2 * size as usize];
            read(0, &mut blocks)?;
            let (a, b) = blocks.split_at(size as usize);
            let Some((active, block)) = Block::select(a, b) else {
                continue;
            };
            if let Some(superblock) = superblock(&block)? {
                validate(superblock, size, length)?;
                return Ok(Some(Self {
                    superblock,
                    anchor_offset: active as u64 * u64::from(size),
                }));
            }
        }
        Ok(None)
    }

    pub(crate) fn open(self, source: &mut impl BlockSource) -> Result<Geometry> {
        let length = source.len();
        let size = self.superblock.block_size;
        let mut superblock = self.superblock;
        let mut root = ANCHOR;
        let mut next = Some(ANCHOR);
        let mut metadata_blocks = HashSet::new();
        let mut global = [0; 3];
        let mut blocks = vec![0; 2 * size as usize];
        while let Some(pair) = next {
            require(
                metadata_blocks.insert(pair[0]) && metadata_blocks.insert(pair[1]),
                "LittleFS global metadata chain cycles or reuses a block",
            )?;
            let (_, block) = read_pair(source, size, superblock.block_count, pair, &mut blocks)?;
            if let Some(current) = self::superblock(&block)? {
                validate(current, size, length)?;
                superblock = current;
                root = pair;
            }
            if let Some(state) = block.latest(|kind| kind == TagType::MoveState)
                && !state.tag.is_special_length()
            {
                require(state.body.len() == 12, "invalid LittleFS move state")?;
                for (part, delta) in global.iter_mut().zip(state.body.as_chunks::<4>().0) {
                    *part ^= u32::from_le_bytes(*delta);
                }
            }
            next = block.tail()?.map(|(pair, _)| pair);
        }
        require(
            metadata_blocks
                .iter()
                .all(|&index| index < superblock.block_count),
            "LittleFS metadata outside declared block count",
        )?;
        let pending_move = if global[0] & 0x7ff0_0000 != 0 {
            let tag = Tag(global[0]);
            require(
                tag.tag_type() == TagType::Delete && tag.id() < 1023,
                "invalid pending LittleFS move",
            )?;
            let pair = normalized([global[1], global[2]]);
            require(
                pair[0] != pair[1] && pair.iter().all(|index| metadata_blocks.contains(index)),
                "invalid LittleFS pending move source",
            )?;
            Some((pair, usize::from(tag.id())))
        } else {
            None
        };
        Ok(Geometry {
            superblock,
            root,
            metadata_blocks,
            pending_move,
        })
    }
}

fn candidate_sizes(head: &[u8], base: u64, length: u64) -> Vec<u32> {
    head.windows(8 + 4 + 24)
        .enumerate()
        .filter(|(_, bytes)| bytes.starts_with(MAGIC))
        .filter_map(|(at, bytes)| {
            let sb = Superblock::from_bytes(&bytes[12..])?;
            let size = u64::from(sb.block_size);
            let at = base + at as u64;
            ((MIN_BLOCK..=MAX_BLOCK).contains(&sb.block_size)
                && length >= size * 2
                && at < size * 2)
                .then_some(sb.block_size)
        })
        .collect()
}

fn superblock(block: &Block<'_>) -> Result<Option<Superblock>> {
    if !block
        .tags
        .iter()
        .any(|entry| entry.tag.tag_type() == TagType::Superblock)
    {
        return Ok(None);
    }
    let slots = slots(block)?;
    let Some(first) = slots.first() else {
        return Ok(None);
    };
    if first.name != Some((TagType::Superblock, MAGIC)) {
        return Ok(None);
    }
    let Some((TagType::InlineStruct, bytes)) = first.layout else {
        return Err(FoldError::Corrupt(
            "LittleFS superblock has no inline structure",
        ));
    };
    Superblock::from_bytes(bytes)
        .map(Some)
        .ok_or(FoldError::Corrupt("invalid LittleFS superblock length"))
}

fn validate(sb: Superblock, size: u32, length: u64) -> Result<()> {
    if sb.version >> 16 != 2 || sb.version > DISK_VERSION {
        return Err(FoldError::Unsupported(
            "LittleFS disk version outside 2.0–2.1",
        ));
    }
    require(
        sb.block_size == size
            && sb.block_count >= 2
            && u64::from(size) * u64::from(sb.block_count) <= length
            && sb.name_max <= 1022
            && sb.file_max <= i32::MAX as u32
            && sb.attr_max <= 1022,
        "invalid LittleFS superblock geometry or limits",
    )
}

pub(crate) fn read_pair<'a>(
    source: &mut impl BlockSource,
    size: u32,
    count: u32,
    pair: Pair,
    blocks: &'a mut [u8],
) -> Result<(u32, Block<'a>)> {
    require(
        pair[0] != pair[1] && pair.iter().all(|&at| at < count),
        "LittleFS metadata pair exceeds declared geometry",
    )?;
    let (a, b) = blocks.split_at_mut(size as usize);
    source.read_at(u64::from(size) * u64::from(pair[0]), a)?;
    source.read_at(u64::from(size) * u64::from(pair[1]), b)?;
    Block::select(a, b)
        .map(|(active, block)| (pair[active], block))
        .ok_or(FoldError::Corrupt(
            "neither LittleFS metadata block has a verified commit",
        ))
}
