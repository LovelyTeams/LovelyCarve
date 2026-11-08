use crate::{
    exact,
    metadata::{Extents, Header},
    require,
};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::BTreeMap;

pub(crate) type Overflow = BTreeMap<(u32, u8, u16), Extents>;
#[derive(Clone, Debug)]
pub(crate) struct Fork {
    pub logical: u32,
    pub physical: u32,
    pub extents: Extents,
}
#[derive(Default)]
pub(crate) struct Map {
    pub extents: Vec<(u16, u16)>,
    pub blocks: u32,
    occupied: Vec<u8>,
}
impl Map {
    pub fn extend(&mut self, header: &Header, bitmap: &[u8], extents: Extents) -> Result<()> {
        let previous = self.blocks;
        self.occupied
            .resize(usize::from(header.block_count).div_ceil(8), 0);
        for (start, count) in extents.into_iter().filter(|(_, count)| *count != 0) {
            let end = u32::from(start) + u32::from(count);
            require(
                end <= u32::from(header.block_count),
                "hfs: extent beyond allocation area",
            )?;
            for block in u32::from(start)..end {
                require(
                    bitmap
                        .get(block as usize / 8)
                        .is_some_and(|byte| byte & (0x80 >> (block % 8)) != 0),
                    "hfs: extent references free allocation block",
                )?;
                let mask = 0x80 >> (block % 8);
                let occupied = &mut self.occupied[block as usize / 8];
                require(*occupied & mask == 0, "hfs: overlapping fork extents")?;
                *occupied |= mask;
            }
            self.blocks += u32::from(count);
            require(
                self.blocks <= u32::from(header.block_count),
                "hfs: fork block count",
            )?;
            self.extents.push((start, count));
        }
        require(
            self.blocks > previous,
            "hfs: empty/non-progressing extent record",
        )
    }
    pub fn build(
        header: &Header,
        bitmap: &[u8],
        fork: &Fork,
        id: u32,
        kind: u8,
        overflow: &Overflow,
    ) -> Result<Self> {
        require(
            fork.logical <= fork.physical && fork.physical.is_multiple_of(header.block_size),
            "hfs: logical/physical fork length",
        )?;
        let mut map = Self::default();
        if fork.physical == 0 {
            require(
                fork.extents.iter().all(|extent| *extent == (0, 0)),
                "hfs: extents on unallocated fork",
            )?;
            return Ok(map);
        }
        map.extend(header, bitmap, fork.extents)?;
        let required = fork.physical / header.block_size;
        while map.blocks < required {
            let start = u16::try_from(map.blocks)
                .map_err(|_| FoldError::Corrupt("hfs: overflow start block"))?;
            let record = overflow
                .get(&(id, kind, start))
                .ok_or(FoldError::Corrupt("hfs: missing overflow extent record"))?;
            map.extend(header, bitmap, *record)?;
        }
        require(
            map.blocks == required,
            "hfs: physical length/extent count mismatch",
        )?;
        Ok(map)
    }
    pub fn read(
        &self,
        source: &mut impl BlockSource,
        header: &Header,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<()> {
        require(
            offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= u64::from(self.blocks) * u64::from(header.block_size)),
            "hfs: incomplete fork map",
        )?;
        let mut logical = 0;
        let mut completed = 0;
        for (start, count) in &self.extents {
            let length = u64::from(*count) * u64::from(header.block_size);
            if offset + completed as u64 >= logical + length {
                logical += length;
                continue;
            }
            let inside = (offset + completed as u64).saturating_sub(logical);
            let take = (length - inside).min((bytes.len() - completed) as u64) as usize;
            let physical =
                header.allocation_base + u64::from(*start) * u64::from(header.block_size) + inside;
            exact(source, physical, &mut bytes[completed..completed + take])?;
            completed += take;
            logical += length;
            if completed == bytes.len() {
                break;
            }
        }
        require(completed == bytes.len(), "hfs: short mapped fork read")
    }
}
