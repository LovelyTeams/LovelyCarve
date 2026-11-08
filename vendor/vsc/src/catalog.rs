//! VSS volume header (offset 0x1E00) and catalog block / entry decoding.
//!
//! The volume header names the catalog; the catalog is one or more 16 KiB
//! blocks, each a 128-byte header followed by 128-byte entries. A type-0x02
//! entry is a snapshot descriptor; the paired type-0x03 entry points at the
//! store's block header so its store-information can be read.

use crate::bytes::{le_u32, le_u64, read_guid};
use crate::guid::{format_guid, VSS_IDENTIFIER};

/// Byte offset of the VSS volume header within the NTFS volume.
pub const VSS_VOLUME_HEADER_OFFSET: u64 = 0x1E00;

/// VSS block size (catalog block and store block), in bytes.
pub const BLOCK_SIZE: usize = 16_384;

/// Length of a catalog block header, in bytes.
pub const CATALOG_BLOCK_HEADER_LEN: usize = 128;

/// Length of a single catalog entry, in bytes.
pub const CATALOG_ENTRY_LEN: usize = 128;

/// Record type of the VSS volume header (offset 20).
pub const VOLUME_HEADER_RECORD_TYPE: u32 = 0x01;

/// Record type of a catalog block header (offset 20).
pub const CATALOG_BLOCK_RECORD_TYPE: u32 = 0x02;

/// Upper bound on catalog blocks walked, to bound a corrupt/looping chain.
pub const MAX_CATALOG_BLOCKS: usize = 4096;

/// The decoded VSS volume header at [`VSS_VOLUME_HEADER_OFFSET`].
#[derive(Debug, Clone)]
pub struct VolumeHeader {
    /// Whether the 16 bytes at offset 0 are the VSS identifier GUID — i.e. this
    /// volume carries VSS metadata at all.
    pub has_vss_identifier: bool,
    /// Format version.
    pub version: u32,
    /// Record type (expected [`VOLUME_HEADER_RECORD_TYPE`]).
    pub record_type: u32,
    /// Current offset field (expected [`VSS_VOLUME_HEADER_OFFSET`]).
    pub current_offset: u64,
    /// Catalog offset (volume-relative); 0 when there is no catalog.
    pub catalog_offset: u64,
    /// Maximum store size, or 0 when unbounded.
    pub maximum_size: u64,
    /// Volume identifier GUID.
    pub volume_identifier: [u8; 16],
    /// Shadow-copy storage volume identifier GUID.
    pub storage_volume_identifier: [u8; 16],
}

impl VolumeHeader {
    /// Parse a volume header from a byte slice positioned at offset 0x1E00.
    #[must_use]
    pub fn parse(buf: &[u8]) -> Self {
        VolumeHeader {
            has_vss_identifier: read_guid(buf, 0) == VSS_IDENTIFIER,
            version: le_u32(buf, 16),
            record_type: le_u32(buf, 20),
            current_offset: le_u64(buf, 24),
            catalog_offset: le_u64(buf, 48),
            maximum_size: le_u64(buf, 56),
            volume_identifier: read_guid(buf, 64),
            storage_volume_identifier: read_guid(buf, 80),
        }
    }
}

/// A shadow-copy store as described by a catalog type-0x02 entry, augmented with
/// the store-header offset from its paired type-0x03 entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDescriptor {
    /// Store identifier GUID (also the store's on-disk filename).
    pub store_id: [u8; 16],
    /// Shadow-copy volume size at snapshot time.
    pub volume_size: u64,
    /// Catalog sequence number.
    pub sequence: u64,
    /// Snapshot flags (0x40 = Vista/7, 0x440 = Win8 file backup).
    pub flags: u64,
    /// Shadow-copy creation time, a raw Windows FILETIME (100 ns since 1601, UTC).
    pub creation_time: u64,
    /// Volume-relative offset of the store block header (from the type-0x03
    /// entry), or `None` when the catalog has no type-0x03 entry (Win 2003 R2).
    pub store_header_offset: Option<u64>,
    /// Volume-relative offset of the store bitmap chain (0x0006), from the
    /// type-0x03 entry at offset 48, or `None` when there is no type-0x03 entry.
    /// Phase-2 reconstruction walks this chain to tell unallocated blocks apart.
    pub store_bitmap_offset: Option<u64>,
    /// Volume-relative offset of the block-descriptor list chain (0x0003), from
    /// the type-0x03 entry at offset 8, or `None` when there is no type-0x03
    /// entry.
    pub store_block_list_offset: Option<u64>,
    /// Volume-relative offset of the store's previous-bitmap chain (0x0006),
    /// from the type-0x03 entry at offset 72; `None` when the entry is absent
    /// or records no previous bitmap (offset 0).
    pub store_previous_bitmap_offset: Option<u64>,
    /// Volume-relative range-list pointer from type 0x03 (+40).
    pub store_block_range_list_offset: Option<u64>,
    /// Raw NTFS file reference from type 0x03 (+56).
    pub ntfs_file_reference: Option<u64>,
    /// Raw allocated size from type 0x03 (+64).
    pub allocated_size: Option<u64>,
}

impl StoreDescriptor {
    /// Whether a type-0x03 entry supplies this store's in-volume data pointers.
    /// This does not establish that the pointed-to metadata is readable.
    #[must_use]
    pub fn has_in_volume_data(&self) -> bool {
        self.store_header_offset.is_some()
    }

    /// The store identifier rendered as a canonical GUID string.
    #[must_use]
    pub fn store_id_string(&self) -> String {
        format_guid(&self.store_id)
    }
}

/// One decoded catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CatalogEntry {
    /// Type 0x00 — empty padding slot.
    Empty,
    /// Type 0x02 — a snapshot descriptor.
    Snapshot(StoreDescriptor),
    /// Type 0x03 — a pointer to a store's block header, block list and bitmap
    /// chains.
    StorePointer {
        store_id: [u8; 16],
        store_block_list_offset: u64,
        store_header_offset: u64,
        store_bitmap_offset: u64,
        store_previous_bitmap_offset: u64,
        store_block_range_list_offset: u64,
        ntfs_file_reference: u64,
        allocated_size: u64,
    },
    /// Any other entry type (0x01, unknown).
    Other,
}

/// Parse a single 128-byte catalog entry.
pub(crate) fn parse_catalog_entry(buf: &[u8]) -> CatalogEntry {
    match le_u64(buf, 0) {
        0x02 => CatalogEntry::Snapshot(StoreDescriptor {
            store_id: read_guid(buf, 16),
            volume_size: le_u64(buf, 8),
            sequence: le_u64(buf, 32),
            flags: le_u64(buf, 40),
            creation_time: le_u64(buf, 48),
            store_header_offset: None,
            store_bitmap_offset: None,
            store_block_list_offset: None,
            store_previous_bitmap_offset: None,
            store_block_range_list_offset: None,
            ntfs_file_reference: None,
            allocated_size: None,
        }),
        0x03 => CatalogEntry::StorePointer {
            store_id: read_guid(buf, 16),
            store_block_list_offset: le_u64(buf, 8),
            store_header_offset: le_u64(buf, 32),
            store_bitmap_offset: le_u64(buf, 48),
            store_previous_bitmap_offset: le_u64(buf, 72),
            store_block_range_list_offset: le_u64(buf, 40),
            ntfs_file_reference: le_u64(buf, 56),
            allocated_size: le_u64(buf, 64),
        },
        0x00 => CatalogEntry::Empty,
        _ => CatalogEntry::Other,
    }
}

/// Read the next-block offset (volume-relative) from a catalog block header.
pub(crate) fn catalog_next_block_offset(block: &[u8]) -> u64 {
    le_u64(block, 40)
}

/// Whether a catalog block header carries the VSS identifier and catalog record
/// type — used to stop walking on a corrupt/foreign block.
pub(crate) fn is_catalog_block(block: &[u8]) -> bool {
    read_guid(block, 0) == VSS_IDENTIFIER
        && le_u32(block, 16) == 1
        && le_u32(block, 20) == CATALOG_BLOCK_RECORD_TYPE
}
