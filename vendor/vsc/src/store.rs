//! Store block header, store information, and store attribute flags.
//!
//! A store's first block header (record type 0x04) records the size of the
//! store information that follows it directly. The store information carries the
//! shadow-copy identity GUIDs, the snapshot context, attribute flags, and the
//! operating/service machine strings.

use crate::bytes::{le_u16, le_u32, le_u64, read_guid, utf16le_string};
use crate::guid::{format_guid, VSS_IDENTIFIER};

/// Length of a store block header, in bytes.
pub const STORE_BLOCK_HEADER_LEN: usize = 128;

/// Record type of a store header block (offset 20).
pub const STORE_HEADER_RECORD_TYPE: u32 = 0x0004;

/// Legacy caller-side information bound. `VssVolume::store_info` now reads the
/// format's fixed 16 KiB block instead of trusting its declared information size.
pub const MAX_STORE_INFO_LEN: usize = 1 << 20;

/// A decoded store block header (the 128-byte record at a store's block offset).
#[derive(Debug, Clone)]
pub struct StoreBlockHeader {
    /// Whether offset 0 carries the VSS identifier GUID.
    pub has_vss_identifier: bool,
    /// Format version.
    pub version: u32,
    /// Record type (see [`STORE_HEADER_RECORD_TYPE`] and the sibling record
    /// types 0x0003 block-descriptor-list / 0x0005 ranges / 0x0006 bitmap).
    pub record_type: u32,
    /// Store-relative offset of this block.
    pub relative_offset: u64,
    /// Volume-relative offset of this block.
    pub current_offset: u64,
    /// Volume-relative offset of the next block, or 0 if last.
    pub next_offset: u64,
    /// Size of the store information (only meaningful in the first block).
    pub store_information_size: u64,
}

impl StoreBlockHeader {
    /// Parse a 128-byte store block header.
    #[must_use]
    pub fn parse(buf: &[u8]) -> Self {
        StoreBlockHeader {
            has_vss_identifier: read_guid(buf, 0) == VSS_IDENTIFIER,
            version: le_u32(buf, 16),
            record_type: le_u32(buf, 20),
            relative_offset: le_u64(buf, 24),
            current_offset: le_u64(buf, 32),
            next_offset: le_u64(buf, 40),
            store_information_size: le_u64(buf, 48),
        }
    }
}

/// VSS store attribute flags (`VSS_VOLUME_SNAPSHOT_ATTRIBUTES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeFlags(pub u32);

impl AttributeFlags {
    /// Persistent — survives reboot.
    pub const PERSISTENT: u32 = 0x0000_0001;
    /// No auto-recovery.
    pub const NO_AUTO_RECOVERY: u32 = 0x0000_0002;
    /// Client-accessible (a user-facing "Previous Versions" copy).
    pub const CLIENT_ACCESSIBLE: u32 = 0x0000_0004;
    /// No auto-release.
    pub const NO_AUTO_RELEASE: u32 = 0x0000_0008;
    /// No writers.
    pub const NO_WRITERS: u32 = 0x0000_0010;
    /// Transportable.
    pub const TRANSPORTABLE: u32 = 0x0000_0020;
    /// Not surfaced.
    pub const NOT_SURFACED: u32 = 0x0000_0040;
    /// Not transacted.
    pub const NOT_TRANSACTED: u32 = 0x0000_0080;
    /// Differential — the copy-on-write mechanism.
    pub const DIFFERENTIAL: u32 = 0x0002_0000;
    /// Plex — a complete copy.
    pub const PLEX: u32 = 0x0004_0000;

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

    /// Whether the persistent flag is set (survives reboot).
    #[must_use]
    pub fn is_persistent(self) -> bool {
        self.contains(Self::PERSISTENT)
    }

    /// Whether the client-accessible flag is set.
    #[must_use]
    pub fn is_client_accessible(self) -> bool {
        self.contains(Self::CLIENT_ACCESSIBLE)
    }

    /// Whether the differential (copy-on-write) flag is set.
    #[must_use]
    pub fn is_differential(self) -> bool {
        self.contains(Self::DIFFERENTIAL)
    }
}

/// The decoded store information record (identity + attributes of a snapshot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreInfo {
    /// Shadow-copy identifier GUID.
    pub shadow_copy_id: [u8; 16],
    /// Shadow-copy set identifier GUID.
    pub shadow_copy_set_id: [u8; 16],
    /// Snapshot context (backup type).
    pub snapshot_context: u32,
    /// Attribute flags.
    pub attributes: AttributeFlags,
    /// Operating machine string (UTF-16LE on disk).
    pub operating_machine: String,
    /// Service machine string (UTF-16LE on disk).
    pub service_machine: String,
}

impl StoreInfo {
    /// Decode a complete store-information payload, rejecting truncated or odd
    /// UTF-16 string lengths rather than publishing default identity fields.
    pub fn parse_checked(buf: &[u8]) -> Result<Self, &'static str> {
        if buf.len() < 68 {
            return Err("truncated store information");
        }
        let operating_size = usize::from(le_u16(buf, 64));
        let service_size_offset = 66 + operating_size;
        if operating_size % 2 != 0 || service_size_offset + 2 > buf.len() {
            return Err("invalid operating machine string size");
        }
        let service_size = usize::from(le_u16(buf, service_size_offset));
        if service_size % 2 != 0 || service_size_offset + 2 + service_size > buf.len() {
            return Err("invalid service machine string size");
        }
        Ok(Self::parse(buf))
    }

    /// The shadow-copy identifier rendered as a canonical GUID string.
    #[must_use]
    pub fn shadow_copy_id_string(&self) -> String {
        format_guid(&self.shadow_copy_id)
    }

    /// The shadow-copy set identifier rendered as a canonical GUID string.
    #[must_use]
    pub fn shadow_copy_set_id_string(&self) -> String {
        format_guid(&self.shadow_copy_set_id)
    }

    /// Parse store information from the bytes directly following the store
    /// block header.
    ///
    /// The two machine strings are length-prefixed UTF-16LE runs; every offset
    /// and length is bounds-checked, so a truncated or lying length yields an
    /// empty string rather than a panic or out-of-bounds read.
    #[must_use]
    pub fn parse(buf: &[u8]) -> Self {
        let op_size = le_u16(buf, 64) as usize;
        let operating_machine = read_string(buf, 66, op_size);
        let svc_size_off = 66 + op_size;
        let svc_size = le_u16(buf, svc_size_off) as usize;
        let service_machine = read_string(buf, svc_size_off + 2, svc_size);
        StoreInfo {
            shadow_copy_id: read_guid(buf, 16),
            shadow_copy_set_id: read_guid(buf, 32),
            snapshot_context: le_u32(buf, 48),
            attributes: AttributeFlags(le_u32(buf, 56)),
            operating_machine,
            service_machine,
        }
    }
}

/// Read a UTF-16LE string of `len` bytes at `off`, yielding an empty string when
/// the range is out of bounds.
fn read_string(buf: &[u8], off: usize, len: usize) -> String {
    buf.get(off..off + len)
        .map(utf16le_string)
        .unwrap_or_default()
}
