use crate::{exact, le16, le32, require};
use lamfold::{BlockSource, FoldError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    V35,
    V36,
}
#[derive(Clone, Debug)]
pub struct Header {
    pub format: Format,
    pub super_offset: u64,
    pub block_size: u32,
    pub blocks: u32,
    pub root: u32,
    pub height: u16,
    pub clean: bool,
    pub external_journal: bool,
    pub label: Vec<u8>,
    pub uuid: Option<[u8; 16]>,
    pub(crate) journal_start: u32,
    pub(crate) journal_blocks: u32,
    pub(crate) journal_on_source: bool,
}
impl Header {
    /// Geometry inspection also identifies dirty volumes; opening refuses them.
    pub fn inspect(bytes: &[u8], super_offset: u64) -> Result<Self> {
        require(
            matches!(super_offset, 8192 | 65536),
            "reiserfs: superblock offset",
        )?;
        let magic = bytes
            .get(52..62)
            .ok_or(FoldError::Corrupt("reiserfs: short superblock"))?;
        let external_journal = magic == b"ReIsEr3Fs\0";
        let format = match magic {
            b"ReIsErFs\0\0" => Format::V35,
            b"ReIsEr2Fs\0" => Format::V36,
            b"ReIsEr3Fs\0" => match le16(bytes, 72)? {
                0 => Format::V35,
                2 => Format::V36,
                _ => {
                    return Err(FoldError::Unsupported(
                        "reiserfs: unknown relocated-journal format",
                    ))
                }
            },
            _ => {
                return Err(FoldError::Unsupported(
                    "reiserfs: not a v3 filesystem (Reiser4 is separate)",
                ))
            }
        };
        let block_size = u32::from(le16(bytes, 44)?);
        require(
            matches!(block_size, 1024 | 2048 | 4096 | 8192),
            "reiserfs: block size",
        )?;
        let blocks = le32(bytes, 0)?;
        let root = le32(bytes, 8)?;
        let height = le16(bytes, 68)?;
        let bitmap_count = u64::from(blocks).div_ceil(u64::from(block_size) * 8);
        let recorded = u64::from(le16(bytes, 70)?);
        require(
            recorded == bitmap_count || (recorded == 0 && bitmap_count > u64::from(u16::MAX)),
            "reiserfs: bitmap count",
        )?;
        require(
            blocks > 0
                && le32(bytes, 4)? <= blocks
                && root > 0
                && root < blocks
                && (2..=6).contains(&height),
            "reiserfs: geometry/root/height",
        )?;
        require(
            super_offset + u64::from(block_size) < u64::from(blocks) * u64::from(block_size),
            "reiserfs: superblock beyond volume",
        )?;
        require(
            le16(bytes, 48)? <= le16(bytes, 46)?,
            "reiserfs: object-id map count",
        )?;
        let uuid = if format == Format::V36 {
            Some(
                bytes
                    .get(84..100)
                    .ok_or(FoldError::Corrupt("reiserfs: short UUID"))?
                    .try_into()
                    .map_err(|_| FoldError::Corrupt("reiserfs: UUID"))?,
            )
        } else {
            None
        };
        let label = if format == Format::V36 {
            let raw = bytes
                .get(100..116)
                .ok_or(FoldError::Corrupt("reiserfs: short label"))?;
            raw[..raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len())].to_vec()
        } else {
            Vec::new()
        };
        let state = le16(bytes, 50)?;
        require(matches!(state, 1 | 2), "reiserfs: invalid unmount state")?;
        let journal_start = le32(bytes, 12)?;
        let journal_blocks = le32(bytes, 20)?;
        let journal_on_source = !external_journal || le32(bytes, 16)? == 0;
        if journal_on_source {
            require(
                journal_start > 0
                    && journal_blocks > 0
                    && journal_start
                        .checked_add(journal_blocks)
                        .is_some_and(|end| end < blocks),
                "reiserfs: journal bounds",
            )?;
        }
        Ok(Self {
            format,
            super_offset,
            block_size,
            blocks,
            root,
            height,
            clean: state == 1,
            external_journal,
            label,
            uuid,
            journal_start,
            journal_blocks,
            journal_on_source,
        })
    }
    pub fn read(source: &mut impl BlockSource) -> Result<Self> {
        for offset in [8192, 65536] {
            if source.len() < offset + 116 {
                continue;
            }
            let mut bytes = [0; 116];
            exact(source, offset, &mut bytes)?;
            if matches!(
                &bytes[52..62],
                b"ReIsErFs\0\0" | b"ReIsEr2Fs\0" | b"ReIsEr3Fs\0"
            ) {
                let header = Self::inspect(&bytes, offset)?;
                require(
                    u64::from(header.blocks) * u64::from(header.block_size) <= source.len(),
                    "reiserfs: truncated volume",
                )?;
                if !header.clean {
                    return Err(FoldError::Unsupported(
                        "reiserfs: dirty volume requires journal replay; source left unchanged",
                    ));
                }
                return Ok(header);
            }
        }
        Err(FoldError::Unsupported("reiserfs: v3 superblock absent"))
    }
    pub(crate) fn bitmap(&self, index: u32) -> u32 {
        if self.super_offset == 8192 || index == 0 {
            (self.super_offset / u64::from(self.block_size)) as u32 + 1 + index
        } else {
            index * self.block_size * 8
        }
    }
    pub(crate) fn data_block(&self, block: u32) -> Result<()> {
        let bitmap_index = block / (self.block_size * 8);
        let old_bitmap_count = self.blocks.div_ceil(self.block_size * 8);
        let first_bitmap = self.bitmap(0);
        require(
            block > (self.super_offset / u64::from(self.block_size)) as u32
                && block < self.blocks
                && block != self.bitmap(bitmap_index)
                && !(self.super_offset == 8192
                    && block >= first_bitmap
                    && block < first_bitmap + old_bitmap_count)
                && (!self.journal_on_source
                    || block < self.journal_start
                    || block > self.journal_start + self.journal_blocks),
            "reiserfs: block overlaps reserved region/outside volume",
        )
    }
}
