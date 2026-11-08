use anyhow::{Context, bail, ensure};
use uuid::Uuid;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VdiHeader {
    pub text: [u8; 0x40],
    pub signature: u32,
    pub version: u32,
    pub header_size: u32,
    pub image_type: u32,
    pub image_flags: u32,
    pub description: [u8; 0x100],
    pub block_offsets_offset: u32,
    pub data_offset: u32,
    pub cylinders: u32, // disk geometry, unused here
    pub heads: u32,     // disk geometry, unused here
    pub sectors: u32,   // disk geometry, unused here
    pub sector_size: u32,
    pub unused1: u32,
    pub disk_size: u64,
    pub block_size: u32,
    pub block_extra: u32, // unused here
    pub blocks_in_image: u32,
    pub blocks_allocated: u32,
    pub uuid_image: Uuid,
    pub uuid_last_snap: Uuid,
    pub uuid_link: Uuid,
    pub uuid_parent: Uuid,
}

impl VdiHeader {
    pub const VERSION: u32 = 0x00010001;
    pub const SIGNATURE: u32 = 0xBEDA107F;
    pub const ENCODED_SIZE: usize = 456;
    pub const MINIMUM_BODY_SIZE: u32 = 384;

    pub(crate) fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        ensure!(
            bytes.len() >= Self::ENCODED_SIZE,
            "VDI header is truncated: expected at least {} bytes, got {}",
            Self::ENCODED_SIZE,
            bytes.len()
        );

        fn array<const N: usize>(bytes: &[u8], offset: usize) -> anyhow::Result<[u8; N]> {
            let end = offset
                .checked_add(N)
                .context("VDI header field offset overflow")?;
            let Some(field) = bytes.get(offset..end) else {
                bail!("VDI header field at offset {offset} is truncated");
            };
            field
                .try_into()
                .context("VDI header field has an unexpected length")
        }

        fn u32_at(bytes: &[u8], offset: usize) -> anyhow::Result<u32> {
            Ok(u32::from_le_bytes(array(bytes, offset)?))
        }

        fn u64_at(bytes: &[u8], offset: usize) -> anyhow::Result<u64> {
            Ok(u64::from_le_bytes(array(bytes, offset)?))
        }

        Ok(Self {
            text: array(bytes, 0)?,
            signature: u32_at(bytes, 64)?,
            version: u32_at(bytes, 68)?,
            header_size: u32_at(bytes, 72)?,
            image_type: u32_at(bytes, 76)?,
            image_flags: u32_at(bytes, 80)?,
            description: array(bytes, 84)?,
            block_offsets_offset: u32_at(bytes, 340)?,
            data_offset: u32_at(bytes, 344)?,
            cylinders: u32_at(bytes, 348)?,
            heads: u32_at(bytes, 352)?,
            sectors: u32_at(bytes, 356)?,
            sector_size: u32_at(bytes, 360)?,
            unused1: u32_at(bytes, 364)?,
            disk_size: u64_at(bytes, 368)?,
            block_size: u32_at(bytes, 376)?,
            block_extra: u32_at(bytes, 380)?,
            blocks_in_image: u32_at(bytes, 384)?,
            blocks_allocated: u32_at(bytes, 388)?,
            uuid_image: Uuid::from_bytes(array(bytes, 392)?),
            uuid_last_snap: Uuid::from_bytes(array(bytes, 408)?),
            uuid_link: Uuid::from_bytes(array(bytes, 424)?),
            uuid_parent: Uuid::from_bytes(array(bytes, 440)?),
        })
    }
}
