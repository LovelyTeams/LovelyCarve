use std::collections::{BTreeMap, HashSet};

use lamfold::{BlockSource, FoldError, Result};

use crate::{SECTOR_SIZE, le32, require};

pub const HEADER_OFFSET: u64 = 16 * SECTOR_SIZE as u64;
pub const HEADER_LENGTH: usize = 2 * SECTOR_SIZE;
const SUPER_MAGIC: [u8; 8] = [0x49, 0xe8, 0x95, 0xf9, 0xc5, 0xe9, 0x53, 0xfa];
const SPARE_MAGIC: [u8; 8] = [0x49, 0x18, 0x91, 0xf9, 0xc5, 0x29, 0x52, 0xfa];

#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub version: u8,
    pub functional_version: u8,
    pub sectors: u32,
    pub root: u32,
    pub clean: bool,
    hotfix_directory: u32,
    hotfix_allocated: u32,
    pub hotfix_used: u32,
    pub(crate) code_page_directory: u32,
    pub(crate) code_pages: u32,
}

impl Header {
    pub fn inspect(bytes: &[u8], source_length: u64) -> Result<Option<Self>> {
        if bytes.get(..8) != Some(&SUPER_MAGIC) {
            return Ok(None);
        }
        let bytes = bytes
            .get(..HEADER_LENGTH)
            .ok_or(FoldError::Corrupt("HPFS short headers"))?;
        let (superblock, spare) = bytes.split_at(SECTOR_SIZE);
        require(spare[..8] == SPARE_MAGIC, "HPFS spare block signature")?;
        let sectors = le32(superblock, 16);
        let root = le32(superblock, 12);
        let band_length = le32(superblock, 48);
        let band_start = le32(superblock, 52);
        let band_last = le32(superblock, 56);
        require(
            sectors > 20
                && u64::from(sectors) * SECTOR_SIZE as u64 <= source_length
                && (20..sectors).contains(&root)
                && band_length > 0
                && band_start >= 20
                && band_start.checked_add(band_length - 1) == Some(band_last)
                && band_last < sectors,
            "HPFS volume geometry",
        )?;
        let hotfix_directory = le32(spare, 12);
        let hotfix_used = le32(spare, 16);
        let hotfix_allocated = le32(spare, 20);
        require(
            hotfix_used <= hotfix_allocated && hotfix_allocated <= 256,
            "HPFS hotfix counts",
        )?;
        if hotfix_used != 0 {
            require(
                hotfix_directory >= 20
                    && hotfix_directory
                        .checked_add(4)
                        .is_some_and(|end| end <= sectors),
                "HPFS hotfix table address",
            )?;
        }
        Ok(Some(Self {
            version: superblock[8],
            functional_version: superblock[9],
            sectors,
            root,
            clean: spare[8] & 1 == 0,
            hotfix_directory,
            hotfix_allocated,
            hotfix_used,
            code_page_directory: le32(spare, 32),
            code_pages: le32(spare, 36),
        }))
    }

    pub(crate) fn read(source: &mut impl BlockSource) -> Result<Option<Self>> {
        if source.len() < HEADER_OFFSET + HEADER_LENGTH as u64 {
            return Ok(None);
        }
        let mut bytes = [0; HEADER_LENGTH];
        source.read_at(HEADER_OFFSET, &mut bytes)?;
        Self::inspect(&bytes, source.len())
    }

    pub(crate) fn read_hotfixes(
        &self,
        source: &mut impl BlockSource,
    ) -> Result<BTreeMap<u32, u32>> {
        let mut hotfixes = BTreeMap::new();
        if self.hotfix_used == 0 {
            return Ok(hotfixes);
        }
        let mut bytes = [0; 4 * SECTOR_SIZE];
        source.read_at(
            u64::from(self.hotfix_directory) * SECTOR_SIZE as u64,
            &mut bytes,
        )?;
        let mut seen_targets = HashSet::new();
        for index in 0..self.hotfix_used as usize {
            let from = le32(&bytes, index * 4);
            let to = le32(&bytes, (self.hotfix_allocated as usize + index) * 4);
            require(
                (20..self.sectors).contains(&from)
                    && (20..self.sectors).contains(&to)
                    && hotfixes.insert(from, to).is_none()
                    && seen_targets.insert(to),
                "HPFS conflicting hotfix addresses",
            )?;
        }
        require(
            !hotfixes.keys().any(|from| seen_targets.contains(from)),
            "HPFS chained hotfix addresses",
        )?;
        Ok(hotfixes)
    }
}
