//! All journals must have a verified latest clean-unmount header. No replay
//! is performed, and no success is inferred merely from one old clean record.
use crate::{disk_hash, header::Header, inode::Inode, meta, require, stream::Stream, u32be, u64be};
use lamfold::{BlockSource, FoldError, Result};

pub(crate) fn check(
    source: &mut impl BlockSource,
    header: &Header,
    inode: &Inode,
    stream: &Stream,
) -> Result<()> {
    require(
        inode.height != 0 && inode.size != 0 && inode.size.is_multiple_of(header.block_size as u64),
        "gfs2: journal geometry",
    )?;
    require(
        stream.blocks.iter().all(Option::is_some),
        "gfs2: journal holes",
    )?;
    require(
        (8 * 1024 * 1024..=1024 * 1024 * 1024).contains(&inode.size),
        "gfs2: native journal size limits",
    )?;
    let mut buffer = vec![0; 1024 * 1024];
    let mut offset = 0;
    let mut highest: Option<(u64, u32)> = None;
    let mut lowest = u64::MAX;
    while offset < inode.size {
        let count = (inode.size - offset).min(buffer.len() as u64) as usize;
        require(
            stream.read(source, header, offset, &mut buffer[..count])? == count,
            "gfs2: short journal read",
        )?;
        for (index, raw) in buffer[..count]
            .chunks_exact(header.block_size as usize)
            .enumerate()
        {
            if u32be(raw, 0)? != 0x0116_1970 || u32be(raw, 4)? != 8 {
                continue;
            }
            meta(raw, 8, 800)?;
            let logical = offset / header.block_size as u64 + index as u64;
            require(
                u32be(raw, 40)? as u64 == logical,
                "gfs2: journal header block identity",
            )?;
            let mut prefix = raw[..48].to_vec();
            prefix[44..48].fill(0);
            require(
                disk_hash(&prefix) == u32be(raw, 44)?,
                "gfs2: journal header CRC32",
            )?;
            let crc = u32be(raw, 48)?;
            if crc != 0 {
                let mut computed = u32::MAX;
                for byte in &raw[52..] {
                    computed ^= *byte as u32;
                    for _ in 0..8 {
                        computed =
                            (computed >> 1) ^ (0x82f6_3b78 & 0u32.wrapping_sub(computed & 1));
                    }
                }
                require(computed == crc, "gfs2: journal header CRC32C")?;
                require(
                    u64be(raw, 64)?
                        == stream.blocks[logical as usize]
                            .ok_or(FoldError::Corrupt("gfs2: journal hole"))?
                        && u64be(raw, 72)? == inode.number.address,
                    "gfs2: journal owner/physical identity",
                )?;
                require(
                    u32be(raw, 52)? < 1_000_000_000,
                    "gfs2: journal timestamp nanoseconds",
                )?;
            }
            let sequence = u64be(raw, 24)?;
            let flags = u32be(raw, 32)?;
            lowest = lowest.min(sequence);
            if let Some((recorded, recorded_flags)) = highest {
                require(
                    sequence != recorded || flags == recorded_flags,
                    "gfs2: conflicting journal head sequence",
                )?;
            }
            if highest.is_none_or(|(recorded, _)| sequence > recorded) {
                highest = Some((sequence, flags));
            }
        }
        offset += count as u64;
    }
    let (sequence, flags) =
        highest.ok_or(FoldError::Corrupt("gfs2: journal has no valid header"))?;
    require(
        sequence - lowest <= i64::MAX as u64,
        "gfs2: journal sequence wrap/ambiguity",
    )?;
    if flags & 1 == 0 {
        return Err(FoldError::Unsupported(
            "gfs2: nonempty journal requires replay; source unchanged",
        ));
    }
    Ok(())
}
