//! Typed diff-area records: the block descriptor and store block range.
//!
//! These raw records retain the mapping before reconstruction folds forwarders
//! and overlays. [`crate::VssVolume::store_blocks`] / `store_ranges` enumerate
//! them; [`crate::SnapshotLayout`] reconstructs logical snapshot bytes.

use crate::bytes::{le_u32, le_u64};

/// Length of a block descriptor record, in bytes.
pub const BLOCK_DESCRIPTOR_LEN: usize = 32;

/// Length of a store block range record, in bytes.
pub const STORE_BLOCK_RANGE_LEN: usize = 24;

/// Flags on a block descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockDescriptorFlags(pub u32);

impl BlockDescriptorFlags {
    /// Forwarder — maps to the next block's original offset.
    pub const FORWARDER: u32 = 0x01;
    /// Overlay — the allocation bitmap defines how the block is filled.
    pub const OVERLAY: u32 = 0x02;
    /// Not used — the block is ignored.
    pub const NOT_USED: u32 = 0x04;

    /// The raw flag bits.
    #[must_use]
    pub fn bits(self) -> u32 {
        self.0
    }

    /// Whether any of the given flag bits are set.
    #[must_use]
    pub fn contains(self, flag: u32) -> bool {
        self.0 & flag != 0
    }

    /// Whether the forwarder flag is set.
    #[must_use]
    pub fn is_forwarder(self) -> bool {
        self.contains(Self::FORWARDER)
    }

    /// Whether the overlay flag is set.
    #[must_use]
    pub fn is_overlay(self) -> bool {
        self.contains(Self::OVERLAY)
    }

    /// Whether the not-used flag is set.
    #[must_use]
    pub fn is_not_used(self) -> bool {
        self.contains(Self::NOT_USED)
    }
}

/// A 32-byte diff-area block descriptor: it maps an original volume block to the
/// copy-on-write block saved in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockDescriptor {
    /// Original data block offset (volume-relative).
    pub original_offset: u64,
    /// Relative store data block offset (store-relative).
    pub relative_store_offset: u64,
    /// Store data block offset (volume-relative).
    pub store_offset: u64,
    /// Flags.
    pub flags: BlockDescriptorFlags,
    /// Allocation bitmap (used when the overlay flag is set).
    pub allocation_bitmap: u32,
}

impl BlockDescriptor {
    /// Parse a 32-byte block descriptor.
    #[must_use]
    pub fn parse(buf: &[u8]) -> Self {
        BlockDescriptor {
            original_offset: le_u64(buf, 0),
            relative_store_offset: le_u64(buf, 8),
            store_offset: le_u64(buf, 16),
            flags: BlockDescriptorFlags(le_u32(buf, 24)),
            allocation_bitmap: le_u32(buf, 28),
        }
    }
}

/// A 24-byte store block range record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreBlockRange {
    /// Store block range start offset (volume-relative).
    pub store_offset: u64,
    /// Relative block range start offset (store-relative).
    pub relative_offset: u64,
    /// Block range size, in bytes.
    pub range_size: u64,
}

impl StoreBlockRange {
    /// Parse a 24-byte store block range record.
    #[must_use]
    pub fn parse(buf: &[u8]) -> Self {
        StoreBlockRange {
            store_offset: le_u64(buf, 0),
            relative_offset: le_u64(buf, 8),
            range_size: le_u64(buf, 16),
        }
    }
}
