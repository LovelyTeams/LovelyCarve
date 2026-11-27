use lamfold::{BlockSource, FoldError, Result};

use crate::format::{BUFFER_BYTES, BUFFER_OFFSET_MASK, Header, Order};
use crate::require;

const UNDO_MAP: usize = 264 + 3 * 40;
const UNDO_ARRAY: usize = 264 + 16 * 40;
const BIGBLOCK: u64 = 1 << 23;
const ALIGN: u64 = 8;
const PAD: u16 = 0x8040;
const HEAD: u16 = 0xc84e;
const TAIL: u16 = 0xc74f;
const RECOVERY_REQUIRED: FoldError =
    FoldError::Unsupported("HAMMER volume requires undo or redo recovery");

#[derive(Clone)]
pub(crate) struct Undo {
    first: u64,
    next: u64,
    length: u64,
    blocks: Vec<u64>,
}

impl Undo {
    pub fn parse(bytes: &[u8], order: Order) -> Result<Self> {
        let offset = |at| {
            let encoded = order.u64(&bytes[at..]);
            require(encoded >> 52 == 0x300, "invalid HAMMER undo offset zone")?;
            Ok(encoded & BUFFER_OFFSET_MASK)
        };
        let first = offset(UNDO_MAP + 8)?;
        let next = offset(UNDO_MAP + 16)?;
        let length = offset(UNDO_MAP + 24)?;
        require(
            length > 0
                && length <= 128 * BIGBLOCK
                && length.is_multiple_of(BIGBLOCK)
                && first <= length
                && next <= length
                && first.is_multiple_of(ALIGN)
                && next.is_multiple_of(ALIGN),
            "invalid HAMMER undo ring geometry",
        )?;
        let blocks = bytes[UNDO_ARRAY..]
            .as_chunks::<8>()
            .0
            .iter()
            .take((length / BIGBLOCK) as usize)
            .map(|address| order.u64(address))
            .collect();
        Ok(Self {
            first,
            next,
            length,
            blocks,
        })
    }

    pub fn validate_clean(&self, header: &Header, source: &mut impl BlockSource) -> Result<()> {
        if self.first % self.length != self.next % self.length {
            return Err(RECOVERY_REQUIRED);
        }
        if header.version >= 4 {
            // The next index can lag newly written undo records. The sequence
            // numbers on either side of the first index establish the boundary.
            let previous = self.nearest_sequence(header, source, true)?;
            let next = self.nearest_sequence(header, source, false)?;
            if previous.wrapping_add(1) == next {
                return Err(RECOVERY_REQUIRED);
            }
        }
        Ok(())
    }

    fn nearest_sequence(
        &self,
        header: &Header,
        source: &mut impl BlockSource,
        reverse: bool,
    ) -> Result<u32> {
        let mut cursor = self.first % self.length;
        let mut buffer = [0; BUFFER_BYTES as usize];
        let mut buffer_at = None;
        // Padding bridges alignment gaps. An entire run of padding cannot
        // establish the boundary and is treated as an unreadable log.
        for _ in 0..128 {
            let record_at = if reverse {
                (cursor + self.length - ALIGN) % self.length
            } else {
                cursor
            };
            let aligned = record_at / BUFFER_BYTES * BUFFER_BYTES;
            if buffer_at != Some(aligned) {
                let raw = self.blocks[(aligned / BIGBLOCK) as usize];
                let base = header.physical(raw, BIGBLOCK as usize, &[2], BIGBLOCK)?;
                source.read_at(base + aligned % BIGBLOCK, &mut buffer)?;
                buffer_at = Some(aligned);
            }
            let index = (record_at - aligned) as usize;
            let order = header.order;
            let length = order.u32(&buffer[index + 4..]) as usize;
            require(
                (ALIGN as usize..=buffer.len()).contains(&length)
                    && length.is_multiple_of(ALIGN as usize),
                "invalid HAMMER undo record length",
            )?;
            let start = if reverse {
                (index + ALIGN as usize)
                    .checked_sub(length)
                    .ok_or(FoldError::Corrupt("HAMMER undo crosses a buffer boundary"))?
            } else {
                index
            };
            let record = buffer
                .get(start..start + length)
                .ok_or(FoldError::Corrupt("HAMMER undo crosses a buffer boundary"))?;
            let kind = order.u16(&record[2..]);
            require(
                order.u16(record) == HEAD && order.u32(&record[4..]) as usize == length,
                "invalid HAMMER undo record header",
            )?;
            if kind != PAD || length != ALIGN as usize {
                let tail = &record[length - ALIGN as usize..];
                require(
                    order.u16(tail) == TAIL
                        && order.u16(&tail[2..]) == kind
                        && order.u32(&tail[4..]) as usize == length,
                    "invalid HAMMER undo record footer",
                )?;
            }
            if kind != PAD {
                require(
                    length >= 24
                        && header.checksum().valid_split(
                            order.u32(&record[12..]),
                            &record[..12],
                            &record[16..],
                        ),
                    "HAMMER undo record CRC mismatch",
                )?;
                return Ok(order.u32(&record[8..]));
            }
            let boundary = if reverse { start } else { start + length };
            cursor = (aligned + boundary as u64) % self.length;
        }
        Err(FoldError::Corrupt(
            "HAMMER undo ring does not have a sequence boundary",
        ))
    }
}
