use crate::{convert, exact, require, Source};
use amiga_ffs::{BlockSource as _, Variant, Volume};
use lamfold::{BlockSource, FoldError, Result};

/// Mount geometry supplied by a partition's DosEnvec. Units are filesystem
/// blocks here; the RDB adapter converts device-sector extents separately.
#[derive(Clone, Copy, Debug)]
pub struct Hints {
    pub block_size: u32,
    pub blocks: u64,
    pub reserved: u32,
    pub dos_type: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub block_size: u32,
    pub blocks: u64,
    pub reserved: u32,
    pub root: u64,
    pub variant: Variant,
    pub clean: bool,
}
impl Geometry {
    /// Inspect root structure without trusting a dirty allocation bitmap.
    pub fn inspect(source: &mut impl BlockSource, hints: Option<Hints>) -> Result<Self> {
        let mut boot = [0; 12];
        exact(source, 0, &mut boot)?;
        let dos_type = amiga_ffs::be32(&boot, 0);
        let expected = hints.and_then(|hint| Variant::from_dostype(hint.dos_type));
        if hints.is_some() && expected.is_none() {
            return Err(FoldError::Unsupported(
                "amiga: unsupported RDB DosType (not DOS0..DOS7)",
            ));
        }
        let variant = convert(amiga_ffs::resolve_variant(dos_type, expected))?;
        let candidates: [u32; 7] = [512, 1024, 2048, 4096, 8192, 16384, 32768];
        let mut selected = None;
        for block_size in candidates {
            if hints.is_some_and(|hint| hint.block_size != block_size)
                || (hints.is_none() && !source.len().is_multiple_of(u64::from(block_size)))
            {
                continue;
            }
            let blocks = hints.map_or(source.len() / u64::from(block_size), |hint| hint.blocks);
            let reserved = hints.map_or(2, |hint| hint.reserved);
            if reserved < 2
                || blocks > u64::from(u32::MAX)
                || blocks * u64::from(block_size) > source.len()
            {
                continue;
            }
            let Some(canonical) = amiga_ffs::canonical_root_lba(blocks, u64::from(reserved)) else {
                continue;
            };
            let recorded = amiga_ffs::be32(&boot, 8);
            let root = if recorded == 0 {
                canonical
            } else {
                u64::from(recorded)
            };
            if root < u64::from(reserved) || root >= blocks {
                continue;
            }
            let mut bytes = vec![0; block_size as usize];
            exact(source, root * u64::from(block_size), &mut bytes)?;
            if amiga_ffs::verify_root_block::<FoldError>(&bytes, root).is_err() {
                continue;
            }
            require(
                amiga_ffs::be32(&bytes, 4) == 0
                    && amiga_ffs::be32(&bytes, 12)
                        == amiga_ffs::hash_table_size(block_size as usize)
                    && amiga_ffs::be32(&bytes, block_size as usize - 12) == 0,
                "amiga: root identity/hash geometry/parent",
            )?;
            let bitmap_flag = amiga_ffs::be32(&bytes, block_size as usize - 200);
            require(
                matches!(bitmap_flag, 0 | u32::MAX),
                "amiga: invalid bitmap-valid flag",
            )?;
            let geometry = Self {
                block_size,
                blocks,
                reserved,
                root,
                variant,
                clean: bitmap_flag == u32::MAX,
            };
            if selected.is_some() {
                return Err(FoldError::Corrupt(
                    "amiga: ambiguous filesystem block geometry",
                ));
            }
            selected = Some(geometry);
        }
        selected.ok_or(FoldError::Corrupt(
            "amiga: no valid root for filesystem geometry",
        ))
    }
    pub(crate) fn open<S: BlockSource>(self, source: S) -> Result<Volume<Source<S>>> {
        if !self.clean {
            return Err(FoldError::Unsupported(
                "amiga: dirty allocation bitmap requires disk validation; source left unchanged",
            ));
        }
        let source = Source {
            source,
            geometry: self,
            remaining: 131072,
        };
        let mut volume = convert(Volume::open_at_root(
            source,
            self.variant,
            self.blocks,
            self.root,
        ))?;
        volume.set_reserved(u64::from(self.reserved));
        // Opening through open_at_root still checks the LNFS root's DosType.
        require(
            volume.source_mut().block_count() == Some(self.blocks),
            "amiga: source geometry mismatch",
        )?;
        Ok(volume)
    }
}
