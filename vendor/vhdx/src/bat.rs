use crate::bytes::le_u64;
use crate::error::{Result, VhdxError};
use crate::metadata::VhdxMetadata;

const PAYLOAD_BLOCK_NOT_PRESENT: u64 = 0;
const PAYLOAD_BLOCK_UNDEFINED: u64 = 1;
const PAYLOAD_BLOCK_ZERO: u64 = 2;
const PAYLOAD_BLOCK_UNMAPPED: u64 = 3;
const PAYLOAD_BLOCK_FULLY_PRESENT: u64 = 6;
const PAYLOAD_BLOCK_PARTIALLY_PRESENT: u64 = 7;
const SECTOR_BITMAP_BLOCK_PRESENT: u64 = 6;
const MIB: u64 = 0x0010_0000;

/// Where the bytes for a virtual offset actually live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadTarget {
    /// Served from this image at the given container offset.
    File(u64),
    /// This image does not describe the block: defer to the parent, or to
    /// zeros when there is no parent.
    Parent,
    /// This image describes the block as having no meaningful contents.
    /// Distinct from [`ReadTarget::Parent`] — the parent must not be consulted.
    Zero,
    /// Ownership is per logical sector; `bitmap_bit` selects this sector's bit
    /// within the byte at `bitmap_byte_file_offset` (bit 0 is the LSB).
    Partial {
        file_offset: u64,
        bitmap_byte_file_offset: u64,
        bitmap_bit: u8,
    },
}

#[derive(Debug, Clone)]
pub struct Bat {
    entries: Vec<u64>,
    meta: VhdxMetadata,
    #[allow(dead_code)]
    region_offset: u64,
}

impl Bat {
    pub fn parse(data: &[u8], bat_offset: u64, bat_len: u32, meta: VhdxMetadata) -> Result<Self> {
        let start = bat_offset as usize;
        let end = start + bat_len as usize;
        if data.len() < end {
            return Err(VhdxError::BatRegionMissing);
        }
        let bat_bytes = &data[start..end];
        let entry_count = bat_bytes.len() / 8;
        let mut entries = Vec::with_capacity(entry_count);
        for i in 0..entry_count {
            let e = le_u64(bat_bytes, i * 8);
            entries.push(e);
        }
        Ok(Self {
            entries,
            meta,
            region_offset: bat_offset,
        })
    }

    pub(crate) fn read_target_for_byte(&self, virtual_byte: u64) -> Result<ReadTarget> {
        if virtual_byte >= self.meta.virtual_disk_size {
            return Err(VhdxError::SectorOutOfRange {
                sector: virtual_byte / u64::from(self.meta.logical_sector_size),
                size: self.meta.virtual_disk_size,
            });
        }
        let block_size = u64::from(self.meta.block_size);
        let data_block_index = virtual_byte / block_size;
        let offset_within_block = virtual_byte % block_size;
        let chunk_ratio = self.meta.chunk_ratio();

        let bat_index = data_block_index + data_block_index / chunk_ratio;

        let bat_entry = *self
            .entries
            .get(bat_index as usize)
            .ok_or(VhdxError::BlockNotPresent(data_block_index))?;

        // Only PAYLOAD_BLOCK_NOT_PRESENT defers to the parent image. States 1-3
        // are this image asserting the block has no meaningful contents, so
        // reading the parent there would resurrect data the child replaced.
        // States 4 and 5 are reserved and therefore malformed (MS-VHDX 2.3.5).
        let state = bat_entry & 0b111;
        match state {
            PAYLOAD_BLOCK_NOT_PRESENT => return Ok(ReadTarget::Parent),
            PAYLOAD_BLOCK_UNDEFINED | PAYLOAD_BLOCK_ZERO | PAYLOAD_BLOCK_UNMAPPED => {
                return Ok(ReadTarget::Zero)
            }
            PAYLOAD_BLOCK_FULLY_PRESENT | PAYLOAD_BLOCK_PARTIALLY_PRESENT => {}
            _ => {
                return Err(VhdxError::InvalidPayloadBlockState {
                    block: data_block_index,
                    state: state as u8,
                })
            }
        }

        let file_offset = (bat_entry >> 20)
            .checked_mul(MIB)
            .and_then(|o| o.checked_add(offset_within_block))
            .ok_or(VhdxError::AddressOverflow)?;
        if state == PAYLOAD_BLOCK_FULLY_PRESENT {
            return Ok(ReadTarget::File(file_offset));
        }

        let chunk_index = data_block_index / chunk_ratio;
        let bitmap_bat_index = chunk_ratio
            .checked_add(1)
            .and_then(|entries_per_chunk| chunk_index.checked_mul(entries_per_chunk))
            .and_then(|index| index.checked_add(chunk_ratio))
            .ok_or(VhdxError::AddressOverflow)?;
        let bitmap_entry = *self
            .entries
            .get(bitmap_bat_index as usize)
            .ok_or(VhdxError::SectorBitmapNotPresent(data_block_index))?;
        if bitmap_entry & 0b111 != SECTOR_BITMAP_BLOCK_PRESENT {
            return Err(VhdxError::SectorBitmapNotPresent(data_block_index));
        }

        let logical_sector_size = u64::from(self.meta.logical_sector_size);
        let sectors_per_block = block_size / logical_sector_size;
        let sector_in_chunk = (data_block_index % chunk_ratio)
            .checked_mul(sectors_per_block)
            .and_then(|base| base.checked_add(offset_within_block / logical_sector_size))
            .ok_or(VhdxError::AddressOverflow)?;
        let bitmap_byte_file_offset = (bitmap_entry >> 20)
            .checked_mul(MIB)
            .and_then(|offset| offset.checked_add(sector_in_chunk / 8))
            .ok_or(VhdxError::AddressOverflow)?;
        let bitmap_bit = (sector_in_chunk % 8) as u8;

        Ok(ReadTarget::Partial {
            file_offset,
            bitmap_byte_file_offset,
            bitmap_bit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Bat, ReadTarget, MIB};
    use crate::error::VhdxError;
    use crate::metadata::VhdxMetadata;

    // The committed dfvfs fixture is small enough that every block lives in
    // chunk 0, which makes the chunk arithmetic multiply by zero. These tests
    // drive a synthetic BAT with the largest legal block size, giving a chunk
    // ratio of 16 so blocks land in chunk 1 and beyond.
    const SECTOR: u64 = 512;
    const BLOCK: u64 = 256 * MIB;
    const CHUNK_RATIO: u64 = 16;
    const SECTORS_PER_BLOCK: u64 = BLOCK / SECTOR;

    fn meta() -> VhdxMetadata {
        VhdxMetadata {
            block_size: BLOCK as u32,
            has_parent: true,
            virtual_disk_size: 20 * BLOCK,
            logical_sector_size: SECTOR as u32,
        }
    }

    fn bat(entries: Vec<u64>) -> Bat {
        Bat {
            entries,
            meta: meta(),
            region_offset: 0,
        }
    }

    fn entry(offset_mib: u64, state: u64) -> u64 {
        (offset_mib << 20) | state
    }

    fn target_of(entries: Vec<u64>, virtual_byte: u64) -> ReadTarget {
        match bat(entries).read_target_for_byte(virtual_byte) {
            Ok(target) => target,
            Err(e) => panic!("read target must resolve: {e}"),
        }
    }

    /// Chunk 1 payload slot 1 is BAT index 18, and chunk 1's sector bitmap is
    /// BAT index 33 — `chunk_index * (chunk_ratio + 1) + chunk_ratio`.
    #[test]
    fn partial_block_in_second_chunk_resolves_bitmap_position() {
        assert_eq!(meta().chunk_ratio(), CHUNK_RATIO);

        let mut entries = vec![0u64; 34];
        entries[18] = entry(100, super::PAYLOAD_BLOCK_PARTIALLY_PRESENT);
        entries[33] = entry(200, super::SECTOR_BITMAP_BLOCK_PRESENT);

        // Fourth sector of data block 17, the second block of chunk 1.
        let target = target_of(entries, 17 * BLOCK + 3 * SECTOR);

        let sector_in_chunk = SECTORS_PER_BLOCK + 3;
        assert_eq!(
            target,
            ReadTarget::Partial {
                file_offset: 100 * MIB + 3 * SECTOR,
                bitmap_byte_file_offset: 200 * MIB + sector_in_chunk / 8,
                bitmap_bit: (sector_in_chunk % 8) as u8,
            }
        );
    }

    /// A block described as zero is the child's assertion that the block is
    /// empty; consulting the parent there would resurrect replaced data.
    #[test]
    fn zero_state_does_not_defer_to_parent() {
        let mut entries = vec![0u64; 34];
        entries[18] = entry(100, super::PAYLOAD_BLOCK_ZERO);

        assert_eq!(target_of(entries, 17 * BLOCK), ReadTarget::Zero);
    }

    #[test]
    fn undefined_and_unmapped_states_do_not_defer_to_parent() {
        for state in [
            super::PAYLOAD_BLOCK_UNDEFINED,
            super::PAYLOAD_BLOCK_UNMAPPED,
        ] {
            let mut entries = vec![0u64; 34];
            entries[18] = entry(100, state);

            let target = target_of(entries, 17 * BLOCK);

            assert_eq!(target, ReadTarget::Zero, "state {state}");
        }
    }

    #[test]
    fn reserved_payload_states_are_errors() {
        for state in [4u64, 5] {
            let mut entries = vec![0u64; 34];
            entries[18] = entry(100, state);

            let target = bat(entries).read_target_for_byte(17 * BLOCK);

            assert!(matches!(
                target,
                Err(VhdxError::InvalidPayloadBlockState {
                    block: 17,
                    state: raw_state,
                }) if raw_state == state as u8
            ));
        }
    }

    #[test]
    fn not_present_state_defers_to_parent() {
        assert_eq!(target_of(vec![0u64; 34], 17 * BLOCK), ReadTarget::Parent);
    }

    #[test]
    fn short_bat_is_an_error() {
        let target = bat(vec![0u64; 4]).read_target_for_byte(17 * BLOCK);

        assert!(matches!(target, Err(VhdxError::BlockNotPresent(17))));
    }

    #[test]
    fn partial_block_without_present_bitmap_is_an_error() {
        let mut entries = vec![0u64; 34];
        entries[18] = entry(100, super::PAYLOAD_BLOCK_PARTIALLY_PRESENT);
        // Chunk 1's bitmap entry (index 33) is left not-present.

        let target = bat(entries).read_target_for_byte(17 * BLOCK);

        assert!(matches!(target, Err(VhdxError::SectorBitmapNotPresent(17))));
    }

    #[test]
    fn fully_present_block_in_second_chunk_resolves_file_offset() {
        let mut entries = vec![0u64; 34];
        entries[18] = entry(100, super::PAYLOAD_BLOCK_FULLY_PRESENT);

        let target = target_of(entries, 17 * BLOCK + 3 * SECTOR);

        assert_eq!(target, ReadTarget::File(100 * MIB + 3 * SECTOR));
    }
}
