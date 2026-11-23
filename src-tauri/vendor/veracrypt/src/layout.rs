//! Where VeraCrypt and TrueCrypt keep volume headers, and which bytes the
//! volume behind each header spans.
//!
//! A file container or partition holds a normal header at 0, a hidden-volume
//! header at 64 KiB and embedded backups of both in its last 128 KiB.
//! TrueCrypt before 6.0 kept the hidden header 1536 bytes before the end and
//! data right after a 512-byte header. System encryption keeps one header in
//! sector 62 of the boot drive; its encrypted area is given in absolute drive
//! offsets.

use crate::error::{Result, VeraError};
use crate::header::{Flavor, Header, FLAG_IN_PLACE};
use crate::kdf::KdfTarget;
use crate::xts::DATA_UNIT;

const HEADER_AREA: u64 = 64 * 1024;
const HEADER_GROUP: u64 = 2 * HEADER_AREA;
const LEGACY_HIDDEN_FROM_END: u64 = 1536;
const LEGACY_DATA_START: u64 = 512;
/// Offset of a system encryption header on its drive (sector 62).
pub const SYSTEM_HEADER_OFFSET: u64 = 62 * 512;

/// A header location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeaderKind {
    /// The normal (outer) volume header at offset 0.
    Normal,
    /// A hidden volume's header at 64 KiB.
    Hidden,
    /// The normal header's embedded backup, 128 KiB before the end.
    BackupNormal,
    /// The hidden header's embedded backup, 64 KiB before the end.
    BackupHidden,
    /// A TrueCrypt 5.x hidden header, 1536 bytes before the end.
    LegacyHidden,
    /// A system encryption header in sector 62 of the drive.
    System,
}

impl HeaderKind {
    /// Stable camelCase identifier.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            HeaderKind::Normal => "normal",
            HeaderKind::Hidden => "hidden",
            HeaderKind::BackupNormal => "backupNormal",
            HeaderKind::BackupHidden => "backupHidden",
            HeaderKind::LegacyHidden => "legacyHidden",
            HeaderKind::System => "system",
        }
    }

    /// Whether the header belongs to a hidden volume.
    #[must_use]
    pub fn is_hidden(self) -> bool {
        matches!(
            self,
            HeaderKind::Hidden | HeaderKind::BackupHidden | HeaderKind::LegacyHidden
        )
    }

    /// The header's offset in a source of `size` bytes.
    pub(crate) fn offset(self, size: u64) -> Option<u64> {
        match self {
            HeaderKind::Normal => Some(0),
            HeaderKind::Hidden => Some(HEADER_AREA),
            HeaderKind::BackupNormal => size.checked_sub(HEADER_GROUP),
            HeaderKind::BackupHidden => size.checked_sub(HEADER_AREA),
            HeaderKind::LegacyHidden => size.checked_sub(LEGACY_HIDDEN_FROM_END),
            HeaderKind::System => Some(SYSTEM_HEADER_OFFSET),
        }
    }

    pub(crate) fn kdf_target(self) -> KdfTarget {
        match self {
            HeaderKind::System => KdfTarget::System,
            HeaderKind::Normal
            | HeaderKind::Hidden
            | HeaderKind::BackupNormal
            | HeaderKind::BackupHidden
            | HeaderKind::LegacyHidden => KdfTarget::Volume,
        }
    }

    /// Only TrueCrypt before 6.0 wrote this location.
    pub(crate) fn truecrypt_only(self) -> bool {
        self == HeaderKind::LegacyHidden
    }
}

/// The span a system encryption unlock exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemRange {
    /// One partition of the drive, at its absolute byte range; it must start
    /// inside the encrypted area, as VeraCrypt requires.
    Partition {
        /// Absolute offset of the partition on the drive.
        offset: u64,
        /// Partition length in bytes.
        length: u64,
    },
    /// The whole drive: bytes outside the encrypted area read as stored.
    Drive,
}

/// Which headers an unlock reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderSelection {
    /// The normal and the hidden header (and TrueCrypt 5's hidden header),
    /// as VeraCrypt tries them when mounting.
    Auto,
    /// Only the normal header.
    Normal,
    /// Only hidden-volume headers.
    Hidden,
    /// The embedded backup headers of the normal and the hidden volume.
    Backup,
    /// The system encryption header of the drive the source is.
    System(SystemRange),
}

impl HeaderSelection {
    pub(crate) fn kinds(self) -> &'static [HeaderKind] {
        match self {
            HeaderSelection::Auto => &[
                HeaderKind::Normal,
                HeaderKind::Hidden,
                HeaderKind::LegacyHidden,
            ],
            HeaderSelection::Normal => &[HeaderKind::Normal],
            HeaderSelection::Hidden => &[HeaderKind::Hidden, HeaderKind::LegacyHidden],
            HeaderSelection::Backup => &[HeaderKind::BackupNormal, HeaderKind::BackupHidden],
            HeaderSelection::System(_) => &[HeaderKind::System],
        }
    }
}

/// The bytes a volume exposes, in absolute source offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub(crate) view_start: u64,
    pub(crate) view_len: u64,
    /// Data units in `[encrypted_start, encrypted_end)` are decrypted; any
    /// other unit of the view reads as stored (system drives, unfinished
    /// system encryption).
    pub(crate) encrypted_start: u64,
    pub(crate) encrypted_end: u64,
    /// The sector VeraCrypt passes through when it holds a plaintext NTFS
    /// boot sector (Windows repair rewrites it on system partitions).
    pub(crate) boot_sector_candidate: Option<u64>,
    pub(crate) complete: bool,
}

fn aligned(value: u64) -> bool {
    value % DATA_UNIT as u64 == 0
}

fn fits(start: u64, length: u64, size: u64, what: &str) -> Result<u64> {
    let end = start
        .checked_add(length)
        .ok_or_else(|| VeraError::Corrupt(format!("{what} overflows")))?;
    if end > size {
        return Err(VeraError::Corrupt(format!(
            "{what} ends at byte {end}, past the {size}-byte source"
        )));
    }
    Ok(end)
}

/// The oldest program version whose volumes use the 6.0+ layout.
fn legacy_layout(header: &Header) -> bool {
    match header.flavor {
        Flavor::VeraCrypt => header.required_program_version < 0x010b,
        Flavor::TrueCrypt => header.required_program_version < 0x0600,
    }
}

/// The volume behind `header`, found at `kind` in a source of `size` bytes.
pub(crate) fn geometry(
    kind: HeaderKind,
    header: &Header,
    size: u64,
    system: Option<SystemRange>,
) -> Result<Geometry> {
    match kind {
        HeaderKind::System => system_geometry(header, size, system.unwrap_or(SystemRange::Drive)),
        HeaderKind::LegacyHidden => {
            let length = header.hidden_volume_size;
            let start = size
                .checked_sub(LEGACY_HIDDEN_FROM_END)
                .and_then(|end| end.checked_sub(length))
                .ok_or_else(|| VeraError::Corrupt("hidden volume larger than its host".into()))?;
            volume_view(start, length, size)
        }
        HeaderKind::Normal if legacy_layout(header) => volume_view(
            LEGACY_DATA_START,
            size.saturating_sub(LEGACY_DATA_START),
            size,
        ),
        HeaderKind::BackupNormal | HeaderKind::BackupHidden if legacy_layout(header) => Err(
            VeraError::Unsupported("a pre-6.0 TrueCrypt layout keeps no backup header".into()),
        ),
        HeaderKind::Normal
        | HeaderKind::Hidden
        | HeaderKind::BackupNormal
        | HeaderKind::BackupHidden => {
            if header.encrypted_area_length != header.volume_size {
                return Err(if header.flags & FLAG_IN_PLACE != 0 {
                    VeraError::Unsupported(
                        "in-place encryption of the volume is not complete".into(),
                    )
                } else {
                    VeraError::Corrupt(format!(
                        "encrypted area of {} bytes for a {}-byte volume",
                        header.encrypted_area_length, header.volume_size
                    ))
                });
            }
            if header.encrypted_area_start < LEGACY_DATA_START {
                return Err(VeraError::Corrupt(
                    "the data area overlaps the header".into(),
                ));
            }
            volume_view(header.encrypted_area_start, header.volume_size, size)
        }
    }
}

fn volume_view(start: u64, length: u64, size: u64) -> Result<Geometry> {
    if length == 0 || !aligned(start) || !aligned(length) {
        return Err(VeraError::Corrupt(format!(
            "data area of {length} bytes at {start} is not whole 512-byte units"
        )));
    }
    let end = fits(start, length, size, "the volume")?;
    Ok(Geometry {
        view_start: start,
        view_len: length,
        encrypted_start: start,
        encrypted_end: end,
        boot_sector_candidate: None,
        complete: true,
    })
}

fn system_geometry(header: &Header, size: u64, range: SystemRange) -> Result<Geometry> {
    let start = header.encrypted_area_start;
    let length = header.encrypted_area_length;
    if length == 0 || !aligned(start) || !aligned(length) {
        return Err(VeraError::Corrupt(format!(
            "encrypted area of {length} bytes at {start} is not whole 512-byte units"
        )));
    }
    // The area may extend past an image cut short after the partition the
    // caller reads: like VeraCrypt, only the view itself must be present.
    let end = start
        .checked_add(length)
        .ok_or_else(|| VeraError::Corrupt("the encrypted area overflows".into()))?;
    let complete = length == header.volume_size;
    let (view_start, view_len, candidate) = match range {
        SystemRange::Drive => (0, size, start),
        SystemRange::Partition {
            offset,
            length: part_len,
        } => {
            if offset < start || offset >= end {
                return Err(VeraError::Corrupt(format!(
                    "the partition at byte {offset} is outside the encrypted area {start}..{end}"
                )));
            }
            if part_len == 0 || !aligned(offset) || !aligned(part_len) {
                return Err(VeraError::InvalidRequest(
                    "the partition is not whole 512-byte units".into(),
                ));
            }
            let part_end = fits(offset, part_len, size, "the partition")?;
            if complete && part_end > end {
                return Err(VeraError::Corrupt(format!(
                    "the partition ends at byte {part_end}, past the encrypted area end {end}"
                )));
            }
            (offset, part_len, offset)
        }
    };
    Ok(Geometry {
        view_start,
        view_len,
        encrypted_start: start,
        encrypted_end: end,
        boot_sector_candidate: Some(candidate),
        complete,
    })
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
