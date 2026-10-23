//! Superblock prefix positions adapted from unearth's MIT detector. Full
//! checksums, both copies, geometry and feature/state policy are checked here.
use crate::{crc, exact, field, le16, le32, le64, require};
use lamfold::{BlockSource, FoldError, Result};

#[derive(Clone, Debug)]
pub struct Header {
    pub block_size: u32,
    pub device_size: u64,
    pub segments: u64,
    pub blocks_per_segment: u32,
    pub first_data_block: u64,
    pub last_checkpoint: u64,
    pub last_partial_segment: u64,
    pub last_sequence: u64,
    pub inode_size: usize,
    pub first_inode: u32,
    pub dat_entry_size: usize,
    pub checkpoint_size: usize,
    pub segment_usage_size: usize,
    pub crc_seed: u32,
    pub uuid: [u8; 16],
    pub label: String,
    pub selected_super_offset: u64,
    pub block_count_feature: bool,
    state: u16,
    incompatible: u64,
    readonly_compatible: u64,
}
impl Header {
    pub fn inspect(source: &mut impl BlockSource) -> Result<Self> {
        require(source.len() >= 20 * 1024, "nilfs2: source too short")?;
        let mut raw = [0; 1024];
        exact(source, 1024, &mut raw)?;
        let primary = Self::inspect_block(&raw, source.len(), 1024).ok();
        let mut positions = vec![(source.len() / 4096 - 1) * 4096];
        if let Some(header) = &primary {
            let backup = (header.device_size / 4096 - 1) * 4096;
            if !positions.contains(&backup) {
                positions.push(backup);
            }
        }
        let mut copies: Vec<_> = primary.into_iter().collect();
        for position in positions {
            if copies.first().is_some_and(|header| {
                position
                    < header.segments * header.blocks_per_segment as u64 * header.block_size as u64
            }) {
                continue;
            }
            exact(source, position, &mut raw)?;
            if let Ok(header) = Self::inspect_block(&raw, source.len(), position) {
                if let Some(primary) = copies.first() {
                    require(
                        header.uuid == primary.uuid
                            && header.device_size == primary.device_size
                            && header.block_size == primary.block_size
                            && header.segments == primary.segments
                            && header.blocks_per_segment == primary.blocks_per_segment,
                        "nilfs2: conflicting superblock identities/geometries",
                    )?;
                }
                copies.push(header);
            }
        }
        // As in the native reader, greatest checkpoint wins; primary wins ties.
        copies.sort_by_key(|header| (header.last_checkpoint, header.selected_super_offset == 1024));
        let header = copies
            .pop()
            .ok_or(FoldError::Unsupported("nilfs2: no valid superblock"))?;
        if header.incompatible != 0 || header.readonly_compatible & !1 != 0 {
            return Err(FoldError::Unsupported(
                "nilfs2: unknown filesystem features",
            ));
        }
        if header.state != 1 {
            return Err(FoldError::Unsupported(
                "nilfs2: dirty/error/resize state requires native recovery; source unchanged",
            ));
        }
        Ok(header)
    }
    /// A full 1024-byte superblock; `at` is 1024 or the volume's backup offset.
    pub fn inspect_block(raw: &[u8], source_length: u64, at: u64) -> Result<Self> {
        require(
            raw.len() == 1024
                && le32(raw, 0)? == 2
                && le16(raw, 4)? == 0
                && le16(raw, 6)? == 0x3434,
            "nilfs2: revision/superblock signature",
        )?;
        let bytes = le16(raw, 8)? as usize;
        require(
            (280..=1024).contains(&bytes),
            "nilfs2: superblock checksum span",
        )?;
        let seed = le32(raw, 12)?;
        let mut checked = raw[..bytes].to_vec();
        checked[16..20].fill(0);
        require(
            crc(seed, &checked) == le32(raw, 16)?,
            "nilfs2: superblock CRC mismatch",
        )?;
        let shift = le32(raw, 20)?;
        require(shift <= 6, "nilfs2: block shift")?;
        let block_size = 1024u32 << shift;
        let device_size = le64(raw, 32)?;
        require(
            device_size >= 20 * 1024 && device_size <= source_length,
            "nilfs2: volume/source geometry",
        )?;
        let backup = (device_size / 4096 - 1) * 4096;
        require(
            at == 1024 || at == backup,
            "nilfs2: backup superblock position",
        )?;
        let segments = le64(raw, 24)?;
        let blocks_per_segment = le32(raw, 48)?;
        let first_data_block = le64(raw, 40)?;
        require(
            segments != 0
                && blocks_per_segment >= 16
                && segments
                    .checked_mul(blocks_per_segment as u64)
                    .and_then(|blocks| blocks.checked_mul(block_size as u64))
                    .is_some_and(|end| end <= device_size && (at == 1024 || at >= end))
                && first_data_block >= 4096u64.div_ceil(block_size as u64)
                && first_data_block < blocks_per_segment as u64,
            "nilfs2: segment geometry",
        )?;
        let inode_size = le16(raw, 144)? as usize;
        let dat_entry_size = le16(raw, 146)? as usize;
        let checkpoint_size = le16(raw, 148)? as usize;
        let segment_usage_size = le16(raw, 150)? as usize;
        require(
            inode_size >= 128
                && 16 + 3 * inode_size <= block_size as usize
                && dat_entry_size >= 32
                && dat_entry_size <= block_size as usize
                && checkpoint_size >= 64 + inode_size
                && checkpoint_size <= block_size as usize
                && segment_usage_size >= 16
                && segment_usage_size <= block_size as usize,
            "nilfs2: metadata record sizes",
        )?;
        require(
            le32(raw, 140)? >= 11
                && le64(raw, 56)? != 0
                && (1..=99).contains(&le32(raw, 52)?)
                && le64(raw, 80)? <= segments * blocks_per_segment as u64,
            "nilfs2: reserved inode/checkpoint/free-block fields",
        )?;
        let label = &raw[168..248];
        let end = label
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(label.len());
        let label = String::from_utf8(label[..end].to_vec())
            .map_err(|_| FoldError::Unsupported("nilfs2: non-UTF-8 label"))?;
        let readonly_compatible = le64(raw, 264)?;
        let header = Self {
            block_size,
            device_size,
            segments,
            blocks_per_segment,
            first_data_block,
            last_checkpoint: le64(raw, 56)?,
            last_partial_segment: le64(raw, 64)?,
            last_sequence: le64(raw, 72)?,
            inode_size,
            first_inode: le32(raw, 140)?,
            dat_entry_size,
            checkpoint_size,
            segment_usage_size,
            crc_seed: seed,
            uuid: field(raw, 152)?,
            label,
            selected_super_offset: at,
            block_count_feature: readonly_compatible & 1 != 0,
            state: le16(raw, 116)?,
            incompatible: le64(raw, 272)?,
            readonly_compatible,
        };
        header.block_offset(header.last_partial_segment)?;
        Ok(header)
    }
    pub(crate) fn block_offset(&self, block: u64) -> Result<u64> {
        require(
            block >= self.first_data_block
                && block < self.segments * self.blocks_per_segment as u64,
            "nilfs2: block outside segment span",
        )?;
        Ok(block * self.block_size as u64)
    }
    pub(crate) fn read_block(&self, source: &mut impl BlockSource, block: u64) -> Result<Vec<u8>> {
        let mut raw = vec![0; self.block_size as usize];
        exact(source, self.block_offset(block)?, &mut raw)?;
        Ok(raw)
    }
}
