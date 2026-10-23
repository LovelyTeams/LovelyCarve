//! YAFFS1's dispersed eight-byte tags and 512-byte NAND ECC.
//! Format facts: Aleph One 474b3acb927d, yaffs_guts.h/tagscompat.
use crate::ByteOrder;
use anyhow::{Result, ensure};

#[derive(Clone, Copy)]
pub(crate) struct Tags {
    pub object: u32,
    pub chunk: u32,
    pub count: u32,
    pub serial: u8,
}
pub(crate) enum Page {
    Erased,
    Deleted,
    Used(Tags),
}
pub(crate) fn parse(spare: &[u8], order: ByteOrder) -> Result<Page> {
    ensure!(spare.len() == 16, "YAFFS1 truncated spare");
    ensure!(spare[5] == 255, "YAFFS1 marked bad block is unsupported");
    if spare.iter().all(|byte| *byte == 255) {
        return Ok(Page::Erased);
    }
    if spare[4] == 0 {
        return Ok(Page::Deleted);
    }
    ensure!(spare[4] == 255, "YAFFS1 uncertain page deletion marker");
    let mut bytes = [0; 8];
    for (destination, source) in [0, 1, 2, 3, 6, 7, 11, 12].into_iter().enumerate() {
        bytes[destination] = spare[source];
    }
    let first = order.word(&bytes, 0)?;
    let second = order.word(&bytes, 4)?;
    let (object, chunk, count, serial, shift, upper_count) = match order {
        ByteOrder::Little => (
            second & 0x3ffff,
            first & 0xfffff,
            first >> 22,
            (first >> 20) & 3,
            18,
            second >> 30,
        ),
        ByteOrder::Big => (
            second >> 14,
            first >> 12,
            first & 1023,
            (first >> 10) & 3,
            2,
            second & 3,
        ),
    };
    ensure!(upper_count == 3, "YAFFS1 unsupported extended page count");
    let stored = (second >> shift) & 4095;
    let cleared = second & !(4095 << shift);
    bytes[4..].copy_from_slice(&match order {
        ByteOrder::Little => cleared.to_le_bytes(),
        ByteOrder::Big => cleared.to_be_bytes(),
    });
    let mut parity = 0;
    for bit in 0..64 {
        if bytes[bit / 8] & (1 << (bit % 8)) != 0 {
            parity ^= bit as u32 + 1;
        }
    }
    ensure!(
        parity == stored,
        "YAFFS1 tag ECC mismatch (correction is not enabled)"
    );
    ensure!(
        (1..0x3ffff).contains(&object),
        "YAFFS1 invalid object identity"
    );
    ensure!(
        object > 4 || chunk == 0,
        "YAFFS1 reserved directory has data"
    );
    ensure!(
        chunk == 0 || (1..=512).contains(&count),
        "YAFFS1 invalid data byte count"
    );
    Ok(Page::Used(Tags {
        object,
        chunk,
        count,
        serial: serial as u8,
    }))
}

/// Encode the parity matrix directly: column pairs for each bit coordinate,
/// row pairs for each byte coordinate. No table or error correction is used.
pub(crate) fn data_ecc(bytes: &[u8]) -> [u8; 3] {
    let mut pairs = [0u8; 3];
    for (row, byte) in bytes.iter().copied().enumerate() {
        for column in 0..8 {
            if byte & (1 << column) == 0 {
                continue;
            }
            for axis in 0..3 {
                pairs[2] ^= 1 << (2 + axis * 2 + ((column >> axis) & 1));
            }
            for axis in 0..8 {
                pairs[axis / 4] ^= 1 << ((axis % 4) * 2 + ((row >> axis) & 1));
            }
        }
    }
    [!pairs[0], !pairs[1], !pairs[2] | 3]
}
pub(crate) fn verify_page(page: &[u8]) -> Result<()> {
    ensure!(page.len() == 528, "YAFFS1 truncated page/spare");
    for (start, positions) in [(0, [520, 521, 522]), (256, [525, 526, 527])] {
        let stored = positions.map(|at| page[at]);
        if stored != [255; 3] {
            ensure!(
                data_ecc(&page[start..start + 256]) == stored,
                "YAFFS1 data ECC mismatch (correction is not enabled)"
            );
        }
    }
    Ok(())
}
