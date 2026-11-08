//! Copy-on-write snapshot reconstruction across the store chain.
//!
//! A shadow copy is not self-contained. Windows saves a block into the diff area
//! of the *newest* store the first time the block is overwritten, so the
//! pre-change bytes of an older snapshot may sit in its own store, in any newer
//! store, or still on the live volume. Reconstructing store `i` therefore
//! consults store `i`, then every newer store in creation order, then the
//! volume. The rules below follow the libvshadow "Reading snapshot data"
//! procedure and are checked block-for-block against pyvshadow by the
//! env-gated `tests/oracle_dfvfs.rs`.
//!
//! Each store's diff-area block list is folded, front to back, into a forward
//! map keyed by the original 16 KiB block:
//! - "not used" descriptors are ignored;
//! - a non-overlay descriptor whose original block is the target of an earlier
//!   forwarder takes over that forwarder's original offset (the reverse list),
//!   and the reverse entry is consumed;
//! - a forwarder that points at its own original offset is ignored;
//! - an overlay merges its 512-byte allocation bitmap into the block's overlay;
//!   a non-overlay descriptor replaces the block's base and keeps its overlay;
//! - every forwarder that becomes (or replaces) a base is recorded in the
//!   reverse list by its target block.
//!
//! Reading the requested (active) store walks the chain block by block:
//! - present in the store: the overlay's allocated sub-blocks (active store
//!   only) or else the base. A forwarder reads the *next* store at its target
//!   offset (the volume when there is no next store); any other descriptor reads
//!   the volume at its store offset. An overlay-only entry is invisible to an
//!   older store's read-through;
//! - absent: the next store at the same offset. Past the newest store the
//!   volume, except when the newest store is itself the active one: a block set
//!   in its current bitmap (and in its previous bitmap, when it has one) that is
//!   not a forwarder target was unallocated at snapshot time and reads as zeros.
//!
//! Metadata is validated while it is loaded: a chain block that is out of range,
//! is not a version-1 VSS block of the expected record type, or is reached
//! twice, and a forwarder that carries a store offset, are
//! [`VssError::CorruptStoreMetadata`]. Data reads that would fall outside the
//! volume are I/O errors and are never zero-filled.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use crate::block::BlockDescriptor;
use crate::catalog::BLOCK_SIZE;
use crate::error::VssError;
use crate::metadata::ChainReader;
use crate::VssVolume;

/// VSS block size in volume-relative bytes.
const BLOCK_BYTES: u64 = BLOCK_SIZE as u64;
/// Each overlay bitmap addresses 32 sectors of 512 bytes.
const SUB_BLOCK_SIZE: usize = 512;
const SUB_BLOCK_COUNT: u32 = (BLOCK_SIZE / SUB_BLOCK_SIZE) as u32;

fn block_number(offset: u64) -> u64 {
    offset / BLOCK_BYTES
}

fn bit_is_set(bitmap: &[u8], block: u64) -> bool {
    usize::try_from(block / 8)
        .ok()
        .and_then(|index| bitmap.get(index))
        .is_some_and(|byte| byte & (1u8 << (block % 8)) != 0)
}

/// The descriptors a store holds for one original block: the base saved block
/// (or forwarder) and the overlay patching 512-byte sub-blocks over it. The base
/// is itself the overlay when the store saved only sub-blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ForwardEntry {
    base: BlockDescriptor,
    overlay: Option<BlockDescriptor>,
}

impl ForwardEntry {
    fn overlay(&self) -> Option<&BlockDescriptor> {
        if self.base.flags.is_overlay() {
            Some(&self.base)
        } else {
            self.overlay.as_ref()
        }
    }
}

/// Where the bytes of a piece of a block come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PieceSource {
    /// A descriptor's block: the volume at `offset`, or the next store when the
    /// descriptor is a forwarder and a next store exists.
    Descriptor { offset: u64, forwarder: bool },
    /// Not defined by this store.
    Absent,
}

/// The copy-on-write metadata of one store: its folded diff-area descriptors,
/// the forwarder targets, and its current and previous allocation bitmaps.
///
/// Built by [`VssVolume::store_map`]. Immutable once built, so one map can be
/// shared by every snapshot layout whose chain includes the store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreMap {
    /// Forward entries sorted by original block number.
    forward: Vec<(u64, ForwardEntry)>,
    /// Forwarder target block numbers, sorted.
    reverse: Vec<u64>,
    /// Current bitmap; bit `n` set ⇒ block `n` is not in use by the store.
    current_bitmap: Vec<u8>,
    /// Previous bitmap, when the store records one.
    previous_bitmap: Option<Vec<u8>>,
}

impl StoreMap {
    /// Fold a store's block list, in on-disk order, into a map.
    pub(crate) fn build(
        descriptors: &[BlockDescriptor],
        current_bitmap: Vec<u8>,
        previous_bitmap: Option<Vec<u8>>,
    ) -> Self {
        let mut forward: HashMap<u64, ForwardEntry> = HashMap::new();
        // Forwarder target block -> the forwarder's original offset.
        let mut reverse: HashMap<u64, u64> = HashMap::new();

        for raw in descriptors {
            if raw.flags.is_not_used() {
                continue;
            }
            let mut descriptor = *raw;
            if !descriptor.flags.is_overlay() {
                if let Some(original) = reverse.remove(&block_number(descriptor.original_offset)) {
                    descriptor.original_offset = original;
                }
            }
            if descriptor.flags.is_forwarder()
                && descriptor.original_offset == descriptor.relative_store_offset
            {
                continue;
            }
            match forward.entry(block_number(descriptor.original_offset)) {
                Entry::Vacant(slot) => {
                    slot.insert(ForwardEntry {
                        base: descriptor,
                        overlay: None,
                    });
                }
                Entry::Occupied(mut slot) => {
                    let entry = slot.get_mut();
                    if descriptor.flags.is_overlay() {
                        if entry.base.flags.is_overlay() {
                            entry.base.allocation_bitmap |= descriptor.allocation_bitmap;
                        } else if let Some(overlay) = entry.overlay.as_mut() {
                            overlay.allocation_bitmap |= descriptor.allocation_bitmap;
                        } else {
                            entry.overlay = Some(descriptor);
                        }
                        continue;
                    }
                    let replaced = std::mem::replace(&mut entry.base, descriptor);
                    if replaced.flags.is_overlay() {
                        entry.overlay = Some(replaced);
                    }
                }
            }
            if descriptor.flags.is_forwarder() {
                reverse.insert(
                    block_number(descriptor.relative_store_offset),
                    descriptor.original_offset,
                );
            }
        }

        let mut forward: Vec<(u64, ForwardEntry)> = forward.into_iter().collect();
        forward.sort_unstable_by_key(|(block, _)| *block);
        let mut reverse: Vec<u64> = reverse.into_keys().collect();
        reverse.sort_unstable();
        StoreMap {
            forward,
            reverse,
            current_bitmap,
            previous_bitmap,
        }
    }

    /// The number of original blocks the store defines.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.forward.len()
    }

    fn entry(&self, offset: u64) -> Option<&ForwardEntry> {
        let block = block_number(offset);
        self.forward
            .binary_search_by_key(&block, |(key, _)| *key)
            .ok()
            .and_then(|index| self.forward.get(index))
            .map(|(_, entry)| entry)
    }

    fn is_forwarder_target(&self, offset: u64) -> bool {
        self.reverse.binary_search(&block_number(offset)).is_ok()
    }

    /// Whether the block was unallocated when the snapshot was taken; only
    /// meaningful for the newest store when it is the one being read.
    fn reads_as_zeros(&self, offset: u64) -> bool {
        let block = block_number(offset);
        !self.is_forwarder_target(offset)
            && bit_is_set(&self.current_bitmap, block)
            && self
                .previous_bitmap
                .as_ref()
                .is_none_or(|bitmap| bit_is_set(bitmap, block))
    }
}

/// The owned, reader-independent reconstruction state of one shadow copy: the
/// requested store's [`StoreMap`] followed by every newer store's, in creation
/// order.
///
/// Cheap to clone and `Send + Sync`. Reads take the live-volume reader as a
/// parameter, so one layout can serve any number of positioned readers.
#[derive(Debug, Clone)]
pub struct SnapshotLayout {
    volume_size: u64,
    source_size: u64,
    stores: Vec<Arc<StoreMap>>,
}

impl SnapshotLayout {
    /// Compose a layout from store maps: the requested store first, then each
    /// newer store in creation order (the order of [`VssVolume::stores`]).
    /// `volume_size` is the validated logical snapshot length. Raw catalog
    /// sizes remain available separately on `StoreDescriptor`.
    #[must_use]
    pub fn new(volume_size: u64, stores: Vec<Arc<StoreMap>>) -> Self {
        SnapshotLayout {
            volume_size,
            source_size: volume_size,
            stores,
        }
    }

    /// The size of the reconstructed volume, in bytes.
    #[must_use]
    pub fn volume_size(&self) -> u64 {
        self.volume_size
    }

    /// Fill `buf` with the snapshot's bytes at `offset`.
    ///
    /// `read_volume(offset, buf)` must fill `buf` exactly from the live volume
    /// (offset 0 = the NTFS boot sector) or fail.
    ///
    /// # Errors
    /// [`io::ErrorKind::UnexpectedEof`] when the logical or source range is out
    /// of bounds, [`io::ErrorKind::InvalidData`] on descriptor offset overflow,
    /// and any error of `read_volume`.
    pub fn read_at<F>(&self, offset: u64, buf: &mut [u8], read_volume: &mut F) -> io::Result<()>
    where
        F: FnMut(u64, &mut [u8]) -> io::Result<()>,
    {
        let in_range = u64::try_from(buf.len())
            .ok()
            .and_then(|len| offset.checked_add(len))
            .is_some_and(|end| end <= self.volume_size);
        if !in_range {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "shadow copy read of {} bytes at {offset:#x} runs past the {}-byte volume",
                    buf.len(),
                    self.volume_size
                ),
            ));
        }
        let mut read_volume = |offset: u64, out: &mut [u8]| {
            if offset
                .checked_add(out.len() as u64)
                .is_none_or(|end| end > self.source_size)
            {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "snapshot data lies outside the source volume",
                ));
            }
            read_volume(offset, out)
        };
        let mut done = 0;
        while done < buf.len() {
            let mut target = offset + done as u64;
            let mut length = buf.len() - done;
            let mut depth = 0;
            loop {
                let Some(store) = self.stores.get(depth) else {
                    read_volume(target, &mut buf[done..done + length])?;
                    break;
                };
                let within = (target % BLOCK_BYTES) as usize;
                let (source, piece) = piece_at(store, target, within, depth == 0);
                length = length.min(piece);
                let has_next = depth + 1 < self.stores.len();
                match source {
                    PieceSource::Descriptor {
                        offset: block,
                        forwarder,
                    } => {
                        target = block.checked_add(within as u64).ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "shadow copy descriptor offset overflows",
                            )
                        })?;
                        if forwarder && has_next {
                            depth += 1;
                            continue;
                        }
                        read_volume(target, &mut buf[done..done + length])?;
                    }
                    PieceSource::Absent if has_next => {
                        depth += 1;
                        continue;
                    }
                    PieceSource::Absent if depth == 0 && store.reads_as_zeros(target) => {
                        buf[done..done + length].fill(0);
                    }
                    PieceSource::Absent => read_volume(target, &mut buf[done..done + length])?,
                }
                break;
            }
            done += length;
        }
        Ok(())
    }
}

/// Resolve the source of the bytes at `offset` (`within` bytes into its block)
/// and how many bytes from there share it.
fn piece_at(store: &StoreMap, offset: u64, within: usize, is_active: bool) -> (PieceSource, usize) {
    let to_block_end = BLOCK_SIZE - within;
    let Some(entry) = store.entry(offset) else {
        return (PieceSource::Absent, to_block_end);
    };
    let base = &entry.base;
    let base_source = PieceSource::Descriptor {
        offset: if base.flags.is_forwarder() {
            base.relative_store_offset
        } else {
            base.store_offset
        },
        forwarder: base.flags.is_forwarder(),
    };
    let overlay_only = base.flags.is_overlay();
    let Some(overlay) = entry.overlay() else {
        return (base_source, to_block_end);
    };
    if !is_active {
        // Overlays apply to the requested store only.
        let source = if overlay_only {
            PieceSource::Absent
        } else {
            base_source
        };
        return (source, to_block_end);
    }

    let sub_block = (within / SUB_BLOCK_SIZE) as u32;
    let shifted = overlay.allocation_bitmap >> sub_block;
    let covered = shifted & 1 != 0;
    let run = if covered {
        shifted.trailing_ones()
    } else {
        (!shifted).trailing_ones()
    }
    .min(SUB_BLOCK_COUNT - sub_block) as usize;
    let piece = run * SUB_BLOCK_SIZE - within % SUB_BLOCK_SIZE;
    let source = if covered {
        PieceSource::Descriptor {
            offset: overlay.store_offset,
            forwarder: overlay.flags.is_forwarder(),
        }
    } else if overlay_only {
        PieceSource::Absent
    } else {
        base_source
    };
    (source, piece)
}

/// A reconstructed, read-only view of one shadow copy over the volume's reader.
///
/// Built by [`VssVolume::snapshot`]; borrows the volume's reader mutably so
/// blocks are materialized on demand (the volumes are far too large to hold in
/// memory). For an owned handle use [`Snapshot::layout`] /
/// [`VssVolume::snapshot_layout`] with a reader of your own.
pub struct Snapshot<'v, R> {
    reader: &'v mut R,
    layout: SnapshotLayout,
    position: u64,
}

impl<R: Read + Seek> VssVolume<R> {
    /// Load and fold the copy-on-write metadata of store `index`.
    ///
    /// # Errors
    /// - [`VssError::StoreIndexOutOfRange`] if `index` is past the last store.
    /// - [`VssError::StoreInfoUnavailable`] if the store has no type-0x03
    ///   catalog pointer, so its diff area cannot be located.
    /// - [`VssError::CorruptStoreMetadata`] for an invalid diff-area or bitmap
    ///   chain.
    /// - [`VssError::Io`] on an underlying read/seek failure.
    pub fn store_map(&mut self, index: usize) -> Result<StoreMap, VssError> {
        self.store_info(index)?;
        let count = self.stores.len();
        let store = self
            .stores
            .get(index)
            .ok_or(VssError::StoreIndexOutOfRange { index, count })?;
        let (Some(block_list_offset), Some(bitmap_offset)) =
            (store.store_block_list_offset, store.store_bitmap_offset)
        else {
            return Err(VssError::StoreInfoUnavailable { index });
        };
        let previous_bitmap_offset = store.store_previous_bitmap_offset;

        let mut chains = ChainReader::new(&mut self.reader, self.volume_size, index);
        let current_bitmap = chains.read_bitmap(bitmap_offset)?;
        let previous_bitmap = previous_bitmap_offset
            .map(|offset| chains.read_bitmap(offset))
            .transpose()?;
        let descriptors = chains.read_block_list(block_list_offset)?;
        Ok(StoreMap::build(
            &descriptors,
            current_bitmap,
            previous_bitmap,
        ))
    }

    /// Build the reconstruction layout of store `index`: its own metadata and
    /// that of every newer store.
    ///
    /// # Errors
    /// As [`VssVolume::store_map`], for store `index` or any newer store.
    pub fn snapshot_layout(&mut self, index: usize) -> Result<SnapshotLayout, VssError> {
        self.store_info(index)?;
        let count = self.stores.len();
        self.stores
            .get(index)
            .ok_or(VssError::StoreIndexOutOfRange { index, count })?;
        let volume_size = self.snapshot_size()?;
        let mut maps = Vec::with_capacity(count - index);
        for chain_index in index..count {
            maps.push(Arc::new(self.store_map(chain_index)?));
        }
        let mut layout = SnapshotLayout::new(volume_size, maps);
        layout.source_size = self.volume_size;
        Ok(layout)
    }

    /// Reconstruct store `index` over this volume's reader.
    ///
    /// # Errors
    /// As [`VssVolume::snapshot_layout`].
    pub fn snapshot(&mut self, index: usize) -> Result<Snapshot<'_, R>, VssError> {
        let layout = self.snapshot_layout(index)?;
        Ok(Snapshot {
            reader: &mut self.reader,
            layout,
            position: 0,
        })
    }
}

impl<R: Read + Seek> Snapshot<'_, R> {
    /// The owned reconstruction state behind this view.
    #[must_use]
    pub fn layout(&self) -> &SnapshotLayout {
        &self.layout
    }

    /// Reconstruct the aligned 16384-byte block containing `offset`.
    ///
    /// # Errors
    /// As [`Snapshot::read_at`].
    pub fn read_block(&mut self, offset: u64) -> Result<[u8; BLOCK_SIZE], VssError> {
        let mut block = [0u8; BLOCK_SIZE];
        self.read_at(offset - offset % BLOCK_BYTES, &mut block)?;
        Ok(block)
    }

    /// Fill `buf` with the snapshot's bytes at `offset`.
    ///
    /// # Errors
    /// [`VssError::Io`] when the range runs past the volume, a descriptor
    /// points outside it, or the underlying read/seek fails.
    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), VssError> {
        let reader = &mut *self.reader;
        self.layout
            .read_at(offset, buf, &mut |volume_offset, out: &mut [u8]| {
                reader.seek(SeekFrom::Start(volume_offset))?;
                reader.read_exact(out)
            })?;
        Ok(())
    }
}

impl<R: Read + Seek> Read for Snapshot<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = buffer.len().min(
            usize::try_from(self.layout.volume_size.saturating_sub(self.position))
                .unwrap_or(usize::MAX),
        );
        if count == 0 {
            return Ok(0);
        }
        self.read_at(self.position, &mut buffer[..count])
            .map_err(|error| match error {
                VssError::Io(error) => error,
                other => io::Error::new(io::ErrorKind::InvalidData, other),
            })?;
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for Snapshot<'_, R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.layout.volume_size) + i128::from(delta),
        };
        self.position = u64::try_from(position)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid shadow copy seek"))?;
        Ok(self.position)
    }
}
