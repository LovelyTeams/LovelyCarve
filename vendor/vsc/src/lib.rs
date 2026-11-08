//! # vsc-core — Windows Volume Shadow Copy (VSS) reader
//!
//! A panic-free decoder for the on-disk structures of Windows Volume Shadow Copy
//! (VSS), the `[P^H]` disk-history substrate of the forensic fleet. Given a
//! positioned `Read + Seek` over an NTFS **volume** (offset 0 = the NTFS boot
//! sector), [`VssVolume::open`] reads the VSS volume header at byte offset
//! `0x1E00`, walks the catalog of shadow-copy stores, and exposes each store's
//! [`StoreInfo`] on demand.
//!
//! The reader stays pure: it decodes bytes into typed records and makes no
//! forensic judgments (those live in the sibling `vsc-forensic` analyzer). It
//! never loads the whole volume into memory — the real volumes are hundreds of
//! gigabytes — and every multi-byte read is bounds-checked, so malformed input
//! yields safe defaults in the raw field decoders or a typed [`VssError`] from
//! the validated volume/chain reader, never a panic.
//!
//! It enumerates stores and decodes store information plus the typed diff-area
//! records ([`BlockDescriptor`], [`StoreBlockRange`]), and reconstructs a
//! snapshot's copy-on-write view of the volume via [`VssVolume::snapshot`] →
//! [`Snapshot::read_block`] / [`Snapshot::read_at`], or through the owned,
//! reader-independent [`SnapshotLayout`] built from per-store [`StoreMap`]s.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::{Read, Seek, SeekFrom};

pub mod block;
mod bytes;
pub mod catalog;
mod catalog_walk;
pub mod error;
mod geometry;
pub mod guid;
mod metadata;
pub mod reconstruct;
pub mod store;

#[cfg(test)]
mod tests;

pub use block::{BlockDescriptor, BlockDescriptorFlags, StoreBlockRange};
pub use catalog::{StoreDescriptor, VolumeHeader};
pub use error::VssError;
pub use guid::{format_guid, VSS_IDENTIFIER};
pub use reconstruct::{Snapshot, SnapshotLayout, StoreMap};
pub use store::{AttributeFlags, StoreBlockHeader, StoreInfo};

use catalog::{BLOCK_SIZE, CATALOG_BLOCK_HEADER_LEN, VSS_VOLUME_HEADER_OFFSET};
use catalog_walk::walk_catalog;
use store::{STORE_BLOCK_HEADER_LEN, STORE_HEADER_RECORD_TYPE};

/// Check the VSS identifier at 0x1e00 without parsing the catalog. The original
/// reader position is restored; read/seek faults remain errors.
pub fn check_volume_signature<R: Read + Seek>(reader: &mut R) -> Result<bool, VssError> {
    let position = reader.stream_position()?;
    let result = (|| -> Result<bool, VssError> {
        let size = reader.seek(SeekFrom::End(0))?;
        if size < VSS_VOLUME_HEADER_OFFSET + 16 {
            return Ok(false);
        }
        reader.seek(SeekFrom::Start(VSS_VOLUME_HEADER_OFFSET))?;
        let mut identifier = [0; 16];
        reader.read_exact(&mut identifier)?;
        Ok(identifier == VSS_IDENTIFIER)
    })();
    reader.seek(SeekFrom::Start(position))?;
    result
}

/// A read-only view over the Volume Shadow Copy metadata of an NTFS volume.
///
/// Construct with [`VssVolume::open`] over a `Read + Seek` positioned at the
/// start of the NTFS volume. The catalog of stores is read eagerly (it is
/// small) and ordered by creation time, oldest first — the order the
/// copy-on-write chain is applied in; each store's [`StoreInfo`] is read lazily
/// via [`VssVolume::store_info`].
#[derive(Debug)]
pub struct VssVolume<R> {
    reader: R,
    volume_size: u64,
    has_vss_header: bool,
    catalog_offset: u64,
    header: Option<VolumeHeader>,
    stores: Vec<StoreDescriptor>,
}

impl<R: Read + Seek> VssVolume<R> {
    /// Open a VSS view over a positioned NTFS volume reader.
    ///
    /// Reads the volume header at `0x1E00`; if it carries the VSS identifier and
    /// names a catalog, walks the catalog and enumerates the stores. A volume
    /// with no VSS header (the header region is zeroed) opens successfully with
    /// [`VssVolume::has_vss_header`] `== false` and zero stores.
    ///
    /// # Errors
    /// Returns [`VssError::Io`] on an underlying read/seek failure, or
    /// [`VssError::CorruptVolumeMetadata`] for an invalid header or catalog.
    pub fn open(mut reader: R) -> Result<Self, VssError> {
        let volume_size = reader.seek(SeekFrom::End(0))?;

        let mut has_vss_header = false;
        let mut catalog_offset = 0u64;
        let mut header = None;
        if volume_size >= VSS_VOLUME_HEADER_OFFSET + CATALOG_BLOCK_HEADER_LEN as u64 {
            reader.seek(SeekFrom::Start(VSS_VOLUME_HEADER_OFFSET))?;
            let mut hdr = [0u8; CATALOG_BLOCK_HEADER_LEN];
            reader.read_exact(&mut hdr)?;
            let vh = VolumeHeader::parse(&hdr);
            has_vss_header = vh.has_vss_identifier;
            if has_vss_header {
                if !matches!(vh.version, 1 | 2) || vh.record_type != 1 {
                    return Err(VssError::CorruptVolumeMetadata {
                        offset: VSS_VOLUME_HEADER_OFFSET,
                        reason: "unsupported VSS volume header version or record type",
                    });
                }
                catalog_offset = vh.catalog_offset;
                header = Some(vh);
            }
        } else if check_volume_signature(&mut reader)? {
            return Err(VssError::CorruptVolumeMetadata {
                offset: VSS_VOLUME_HEADER_OFFSET,
                reason: "truncated VSS volume header",
            });
        }

        let stores = if has_vss_header && catalog_offset != 0 {
            walk_catalog(&mut reader, catalog_offset, volume_size)?
        } else {
            Vec::new()
        };

        Ok(VssVolume {
            reader,
            volume_size,
            has_vss_header,
            catalog_offset,
            header,
            stores,
        })
    }

    /// Whether the volume carries a VSS volume header at `0x1E00`.
    #[must_use]
    pub fn has_vss_header(&self) -> bool {
        self.has_vss_header
    }

    /// Complete volume header, including volume/storage GUIDs and maximum size.
    #[must_use]
    pub fn header(&self) -> Option<&VolumeHeader> {
        self.header.as_ref()
    }

    /// Readable snapshot size, distinct from raw catalog sizes and source length.
    /// Consistent, bounded catalog sizes are used directly. Otherwise validated
    /// primary and backup NTFS boot sectors must establish the geometry.
    pub fn snapshot_size(&mut self) -> Result<u64, VssError> {
        let size = self.stores.first().map_or(0, |store| store.volume_size);
        if size > 0
            && size <= self.volume_size
            && self.stores.iter().all(|store| store.volume_size == size)
        {
            return Ok(size);
        }
        geometry::ntfs_volume_size(&mut self.reader, self.volume_size)
    }

    /// The enumerated shadow-copy store descriptors.
    #[must_use]
    pub fn stores(&self) -> &[StoreDescriptor] {
        &self.stores
    }

    /// The number of enumerated shadow-copy stores.
    #[must_use]
    pub fn store_count(&self) -> usize {
        self.stores.len()
    }

    /// The catalog offset from the volume header (0 when there is no catalog).
    #[must_use]
    pub fn catalog_offset(&self) -> u64 {
        self.catalog_offset
    }

    /// The total size of the underlying volume, in bytes.
    #[must_use]
    pub fn volume_size(&self) -> u64 {
        self.volume_size
    }

    /// Read and decode the store information for store `index`.
    ///
    /// # Errors
    /// - [`VssError::StoreIndexOutOfRange`] if `index` is past the last store.
    /// - [`VssError::StoreInfoUnavailable`] if the store has no type-0x03
    ///   catalog pointer.
    /// - [`VssError::StoreOffsetOutOfBounds`] if the store-header offset runs
    ///   past the end of the volume.
    /// - [`VssError::Io`] on an underlying read/seek failure.
    pub fn store_info(&mut self, index: usize) -> Result<StoreInfo, VssError> {
        let count = self.stores.len();
        let header_off = self
            .stores
            .get(index)
            .ok_or(VssError::StoreIndexOutOfRange { index, count })?
            .store_header_offset
            .ok_or(VssError::StoreInfoUnavailable { index })?;

        if header_off
            .checked_add(BLOCK_SIZE as u64)
            .is_none_or(|end| end > self.volume_size)
        {
            return Err(VssError::StoreOffsetOutOfBounds {
                index,
                offset: header_off,
                volume_size: self.volume_size,
            });
        }
        self.reader.seek(SeekFrom::Start(header_off))?;
        let mut block = vec![0; BLOCK_SIZE];
        self.reader.read_exact(&mut block)?;
        let header = StoreBlockHeader::parse(&block);
        if !header.has_vss_identifier
            || header.version != 1
            || header.record_type != STORE_HEADER_RECORD_TYPE
        {
            return Err(VssError::CorruptStoreMetadata {
                index,
                offset: header_off,
                reason: "not a version-1 VSS store header",
            });
        }
        // libvshadow reads the entire 16 KiB block; real headers can record
        // zero in the information-size field, so it is not a read bound.
        StoreInfo::parse_checked(&block[STORE_BLOCK_HEADER_LEN..]).map_err(|reason| {
            VssError::CorruptStoreMetadata {
                index,
                offset: header_off,
                reason,
            }
        })
    }
}
