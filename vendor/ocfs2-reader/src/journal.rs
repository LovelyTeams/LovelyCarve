//! OCFS2 slot flags and the JBD2 superblock both have to indicate clean state.
//! Journal contents are never replayed and active cluster slots are refused.
use crate::{extents::Stream, header::Header, inode::Inode, le32, require};
use lamfold::{BlockSource, FoldError, Result};

fn be32(raw: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(crate::field(raw, offset)?))
}
pub(crate) fn check(
    source: &mut impl BlockSource,
    header: &Header,
    inode: &Inode,
    stream: &Stream,
) -> Result<()> {
    require(
        inode.flags & 0x110 == 0x110
            && inode.size >= 4 * 1024 * 1024
            && inode.size.is_multiple_of(header.block_size as u64),
        "ocfs2: journal inode flags/geometry",
    )?;
    if le32(&inode.raw, 184)? != 0 {
        return Err(FoldError::Unsupported(
            "ocfs2: dirty slot journal requires recovery; source unchanged",
        ));
    }
    let mut raw = [0; 1024];
    require(
        stream.read(source, 0, &mut raw)? == raw.len(),
        "ocfs2: short journal superblock",
    )?;
    require(
        be32(&raw, 0)? == 0xc03b3998 && matches!(be32(&raw, 4)?, 3 | 4),
        "ocfs2: JBD2 superblock identity",
    )?;
    require(
        be32(&raw, 12)? == header.block_size
            && be32(&raw, 16)? as u64 == inode.size / header.block_size as u64,
        "ocfs2: JBD2 geometry",
    )?;
    let first = be32(&raw, 20)?;
    require(
        first >= 1024u32.div_ceil(header.block_size) && first < be32(&raw, 16)?,
        "ocfs2: journal first log block",
    )?;
    if be32(&raw, 28)? != 0 || be32(&raw, 32)? != 0 {
        return Err(FoldError::Unsupported("ocfs2: JBD2 recovery/error state"));
    }
    if be32(&raw, 4)? == 4 {
        let compatible = be32(&raw, 36)?;
        let incompatible = be32(&raw, 40)?;
        if compatible & !1 != 0
            || incompatible & !0x1f != 0
            || be32(&raw, 44)? != 0
            || incompatible & 0x18 == 0x18
        {
            return Err(FoldError::Unsupported(
                "ocfs2: unknown/inconsistent JBD2 features",
            ));
        }
        if incompatible & 0x18 != 0 {
            require(raw[80] == 4, "ocfs2: JBD2 checksum type")?;
            let expected = be32(&raw, 252)?;
            raw[252..256].fill(0);
            let mut crc = u32::MAX;
            for byte in raw {
                crc ^= byte as u32;
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0x82f6_3b78 & 0u32.wrapping_sub(crc & 1));
                }
            }
            require(crc == expected, "ocfs2: JBD2 superblock CRC32C")?;
        }
    }
    Ok(())
}
