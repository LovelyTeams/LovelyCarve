use crate::{be16, be32, names, require};
use lamfold::Result;

pub(crate) type Extents = [(u16, u16); 3];
pub(crate) fn extents(bytes: &[u8], offset: usize) -> Result<Extents> {
    let mut result = [(0, 0); 3];
    let mut terminated = false;
    for (index, extent) in result.iter_mut().enumerate() {
        *extent = (
            be16(bytes, offset + index * 4)?,
            be16(bytes, offset + index * 4 + 2)?,
        );
        require(extent.1 != 0 || extent.0 == 0, "hfs: nonzero empty extent")?;
        require(!terminated || extent.1 == 0, "hfs: extent after terminator")?;
        terminated |= extent.1 == 0;
    }
    Ok(result)
}

/// Primary master directory block; all offsets remain relative to the source
/// volume. Timestamps are local wall-clock seconds from 1904, without an offset.
#[derive(Clone, Debug)]
pub struct Header {
    pub block_size: u32,
    pub block_count: u16,
    pub allocation_base: u64,
    pub bitmap_offset: u64,
    pub volume_name: String,
    pub created_local: u32,
    pub modified_local: u32,
    pub backup_local: u32,
    pub(crate) extents_size: u32,
    pub(crate) extents: Extents,
    pub(crate) catalog_size: u32,
    pub(crate) catalog: Extents,
}
impl Header {
    /// Parse the first 512 bytes of the MDB (source offset 1024). HFS+ wrappers,
    /// MFS and HFS+ are deliberately rejected rather than read as classic HFS.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        require(bytes.len() >= 512, "hfs: short MDB")?;
        require(be16(bytes, 0)? == 0x4244, "hfs: missing BD signature")?;
        require(
            !matches!(be16(bytes, 124)?, 0x482b | 0x4858),
            "hfs: embedded HFS+/HFSX wrapper unsupported",
        )?;
        let block_size = be32(bytes, 20)?;
        let block_count = be16(bytes, 18)?;
        let allocation_base = u64::from(be16(bytes, 28)?) * 512;
        let bitmap_offset = u64::from(be16(bytes, 14)?) * 512;
        require(
            block_size >= 512 && block_size.is_multiple_of(512) && block_count > 0,
            "hfs: allocation geometry",
        )?;
        require(
            bitmap_offset >= 1536
                && bitmap_offset + u64::from(block_count).div_ceil(8) <= allocation_base,
            "hfs: bitmap/allocation area overlap",
        )?;
        require(be16(bytes, 34)? <= block_count, "hfs: free block count")?;
        let length = usize::from(bytes[36]);
        require((1..=27).contains(&length), "hfs: volume name length")?;
        let name = &bytes[37..37 + length];
        require(
            !name.contains(&0) && !name.contains(&b':'),
            "hfs: volume name",
        )?;
        let result = Self {
            block_size,
            block_count,
            allocation_base,
            bitmap_offset,
            volume_name: names::decode(name),
            created_local: be32(bytes, 2)?,
            modified_local: be32(bytes, 6)?,
            backup_local: be32(bytes, 64)?,
            extents_size: be32(bytes, 130)?,
            extents: extents(bytes, 134)?,
            catalog_size: be32(bytes, 146)?,
            catalog: extents(bytes, 150)?,
        };
        for size in [result.extents_size, result.catalog_size] {
            require(
                size >= 512 && size.is_multiple_of(512) && size <= 64 * 1024 * 1024,
                "hfs: metadata tree size/64 MiB limit",
            )?;
        }
        Ok(result)
    }
    pub fn validate_source(&self, size: u64) -> Result<()> {
        require(
            self.allocation_base + u64::from(self.block_count) * u64::from(self.block_size) + 1024
                <= size,
            "hfs: allocation area/truncated volume",
        )
    }
}
