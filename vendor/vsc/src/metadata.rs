//! Bounded store metadata traversal, raw descriptors and range enumeration.
use crate::block::{BlockDescriptor, StoreBlockRange, BLOCK_DESCRIPTOR_LEN, STORE_BLOCK_RANGE_LEN};
use crate::catalog::BLOCK_SIZE;
use crate::store::{StoreBlockHeader, STORE_BLOCK_HEADER_LEN};
use crate::{VssError, VssVolume};
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
const BLOCK_BYTES: u64 = BLOCK_SIZE as u64;
const BLOCK_LIST_RECORD_TYPE: u32 = 3;
const BITMAP_RECORD_TYPE: u32 = 6;
const STORE_BLOCK_VERSION: u32 = 1;
// Bound each store to 64 MiB of metadata instead of allowing 16 GiB allocations.
const MAX_STORE_BLOCKS: usize = 4096;

/// Walks one store's metadata chains, rejecting any block read twice across
/// them (as a cycle or cross-linked chain would).
pub(crate) struct ChainReader<'a, R> {
    reader: &'a mut R,
    volume_size: u64,
    store: usize,
    visited: HashSet<u64>,
    block: Vec<u8>,
}

impl<'a, R: Read + Seek> ChainReader<'a, R> {
    pub(crate) fn new(reader: &'a mut R, volume_size: u64, store: usize) -> Self {
        Self {
            reader,
            volume_size,
            store,
            visited: HashSet::new(),
            block: vec![0; BLOCK_SIZE],
        }
    }

    fn corrupt(&self, offset: u64, reason: &'static str) -> VssError {
        VssError::CorruptStoreMetadata {
            index: self.store,
            offset,
            reason,
        }
    }

    /// Visit the payload of every block of the chain starting at `first`.
    fn walk(
        &mut self,
        first: u64,
        record_type: u32,
        mut visit: impl FnMut(&[u8]) -> Result<(), &'static str>,
    ) -> Result<(), VssError> {
        let mut next = first;
        while next != 0 {
            if self.visited.len() == MAX_STORE_BLOCKS {
                return Err(self.corrupt(next, "metadata chain exceeds the block limit"));
            }
            if !self.visited.insert(next) {
                return Err(self.corrupt(next, "metadata block is reached twice"));
            }
            if next
                .checked_add(BLOCK_BYTES)
                .is_none_or(|end| end > self.volume_size)
            {
                return Err(self.corrupt(next, "metadata block runs past the volume"));
            }
            self.reader.seek(SeekFrom::Start(next))?;
            self.reader.read_exact(&mut self.block)?;
            let header = StoreBlockHeader::parse(&self.block);
            if !header.has_vss_identifier || header.version != STORE_BLOCK_VERSION {
                return Err(self.corrupt(next, "not a version-1 VSS store block"));
            }
            if header.record_type != record_type {
                return Err(self.corrupt(next, "unexpected store block record type"));
            }
            if let Err(reason) = visit(&self.block[STORE_BLOCK_HEADER_LEN..]) {
                return Err(self.corrupt(next, reason));
            }
            next = header.next_offset;
        }
        Ok(())
    }

    /// Concatenate a bitmap chain's payloads in chain order.
    pub(crate) fn read_bitmap(&mut self, first: u64) -> Result<Vec<u8>, VssError> {
        let mut bitmap = Vec::new();
        self.walk(first, BITMAP_RECORD_TYPE, |payload| {
            bitmap.extend_from_slice(payload);
            Ok(())
        })?;
        Ok(bitmap)
    }

    /// Collect a block list's descriptors in on-disk order, skipping empty slots.
    pub(crate) fn read_block_list(&mut self, first: u64) -> Result<Vec<BlockDescriptor>, VssError> {
        let mut descriptors = Vec::new();
        self.walk(first, BLOCK_LIST_RECORD_TYPE, |payload| {
            let (records, _) = payload.as_chunks::<BLOCK_DESCRIPTOR_LEN>();
            for record in records {
                if record.iter().all(|&byte| byte == 0) {
                    continue;
                }
                let descriptor = BlockDescriptor::parse(record);
                if !descriptor.flags.is_not_used()
                    && (!descriptor.original_offset.is_multiple_of(BLOCK_BYTES)
                        || (descriptor.flags.is_forwarder()
                            && !descriptor.relative_store_offset.is_multiple_of(BLOCK_BYTES))
                        || (!descriptor.flags.is_forwarder()
                            && !descriptor.store_offset.is_multiple_of(BLOCK_BYTES)))
                {
                    return Err("unaligned block descriptor offset");
                }
                if descriptor.flags.is_forwarder() && descriptor.store_offset != 0 {
                    return Err("forwarder descriptor carries a store offset");
                }
                descriptors.push(descriptor);
            }
            Ok(())
        })?;
        Ok(descriptors)
    }

    fn read_ranges(&mut self, first: u64) -> Result<Vec<StoreBlockRange>, VssError> {
        let mut ranges = Vec::new();
        let volume_size = self.volume_size;
        self.walk(first, 5, |payload| {
            for record in payload.as_chunks::<STORE_BLOCK_RANGE_LEN>().0 {
                if record.iter().all(|byte| *byte == 0) {
                    continue;
                }
                let range = StoreBlockRange::parse(record);
                if range.range_size == 0
                    || !range.store_offset.is_multiple_of(BLOCK_BYTES)
                    || !range.relative_offset.is_multiple_of(BLOCK_BYTES)
                    || !range.range_size.is_multiple_of(BLOCK_BYTES)
                    || range
                        .store_offset
                        .checked_add(range.range_size)
                        .is_none_or(|end| end > volume_size)
                    || range
                        .relative_offset
                        .checked_add(range.range_size)
                        .is_none()
                {
                    return Err("invalid store block range");
                }
                ranges.push(range);
            }
            Ok(())
        })?;
        Ok(ranges)
    }
}

impl<R: Read + Seek> VssVolume<R> {
    /// Enumerate raw nonempty descriptors in disk order, including ignored,
    /// replaced and overlay records. This matches libvshadow's block API;
    /// `StoreMap::block_count` instead counts folded original blocks.
    pub fn store_blocks(&mut self, index: usize) -> Result<Vec<BlockDescriptor>, VssError> {
        let count = self.stores.len();
        let offset = self
            .stores
            .get(index)
            .ok_or(VssError::StoreIndexOutOfRange { index, count })?
            .store_block_list_offset
            .ok_or(VssError::StoreInfoUnavailable { index })?;
        ChainReader::new(&mut self.reader, self.volume_size, index).read_block_list(offset)
    }

    /// Enumerate and validate the store allocation ranges. They describe the
    /// diff area, not logical file extents, and are optional for reconstruction.
    pub fn store_ranges(&mut self, index: usize) -> Result<Vec<StoreBlockRange>, VssError> {
        let count = self.stores.len();
        let offset = self
            .stores
            .get(index)
            .ok_or(VssError::StoreIndexOutOfRange { index, count })?
            .store_block_range_list_offset
            .ok_or(VssError::StoreInfoUnavailable { index })?;
        ChainReader::new(&mut self.reader, self.volume_size, index).read_ranges(offset)
    }
}
