//! Read-only NTFS volume adapter built on top of the `ntfs` crate.
//!
//! The volume is opened against either a partition slice of a `DiskImage`
//! or the plaintext output of an unlocked BitLocker volume. Each filesystem
//! call instantiates a fresh `Read + Seek` reader over the chosen backing
//! so the underlying image source (raw mmap, EWF, VMDK, BitLocker, ...) only
//! needs to expose a byte-addressable `read` API.

mod backing;
mod deleted;
mod models;
pub use models::{NtfsDeletedRaw, NtfsDeletedSnapshot, NtfsSource};
mod data_stream;
mod directory;
mod forensic_metadata;
mod logical_stream;
mod lznt1;
mod mft_metadata;
mod recovery;
mod reparse;
pub(crate) use recovery::discover_mft;
pub use recovery::RecoveredNtfsGeometry;
mod security;
mod wof;
pub use forensic_metadata::{
    NtfsAttributeMetadata, NtfsFilenameMetadata, NtfsRecordMetadata, NtfsStreamExtent,
    NtfsVolumeMetadata,
};
pub use mft_metadata::{NtfsMftMetadataFile, NtfsMftVolumeMetadata};
mod metadata;
mod read;
mod scan;
use backing::*;

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{TimeZone, Utc};
use ntfs::structured_values::{NtfsFileName, NtfsFileNamespace, NtfsStandardInformation};
use ntfs::{Ntfs, NtfsAttributeFlags, NtfsAttributeType, NtfsFileFlags, NtfsReadSeek, NtfsTime};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::evidence::crypto::bitlocker::UnlockedVolume;
use crate::core::evidence::image::{DiskImage, Partition};
use crate::core::filesystem::fs_dispatch::MissingDataStream;
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};
use crate::core::filesystem::volumes::lvm::LvmVolume;

#[path = "ntfs_journal.rs"]
mod journal;

const ROOT_RECORD: u64 = 5;
const SAMPLE_BYTES: u64 = 4096;
const STREAM_CHUNK_FLOOR: usize = 64 * 1024;

/// One `$DATA` attribute on an MFT record. The unnamed default stream
/// surfaces with `name == ""`; everything else is an NTFS Alternate Data
/// Stream (ADS). `size` is the canonical full stream size (libntfs reports
/// the full value_length even on each split piece of a non-resident
/// attribute).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NtfsDataStream {
    pub name: String,
    pub size: u64,
    pub resident: bool,
}

/// One `$FILE_NAME` edge discovered during a full MFT scan: the child record
/// linked to the parent directory it is named under, plus a ready-to-serve
/// [`FsNode`]. A hard-linked record produces one edge per distinct parent/name,
/// mirroring what a per-directory index walk would surface. The bulk output
/// of [`NtfsVolume::scan_tree_edges`].
#[derive(Debug)]
pub struct NtfsTreeEdge {
    pub record_number: u64,
    pub parent_record_number: u64,
    pub node: FsNode,
}

/// One named Alternate Data Stream (ADS) discovered during a full MFT scan,
/// keyed back to the MFT record that owns it. Surfaced by
/// [`NtfsVolume::scan_tree_edges`] so the snapshot builder can bake each ADS
/// in as an independent file row directly beneath its owning file (the NTFS
/// analog of a ReFS `refsutil` snapshot stream). The unnamed default `$DATA`
/// stream and the ubiquitous `Zone.Identifier` mark-of-the-web stream are
/// excluded — see [`is_surfaced_ads_name`].
#[derive(Debug, Clone)]
pub struct NtfsAdsStream {
    pub record_number: u64,
    pub name: String,
    pub size: u64,
}

/// Whether a `$DATA` attribute name should surface as an independent ADS row.
/// The unnamed default stream (`""`) is the file body itself, and
/// `Zone.Identifier` is the mark-of-the-web stream Windows attaches to nearly
/// every downloaded file — surfacing it would flood the listing with thousands
/// of low-signal rows. Both are still visible in the per-file streams panel
/// (`list_data_streams`); only the inline pseudo-rows skip them.
pub(crate) fn is_surfaced_ads_name(name: &str) -> bool {
    !name.is_empty() && name != "Zone.Identifier"
}

/// Poll cancellation and report progress every N records during a full MFT
/// scan. A power of two so the modulo folds to a mask.
const SCAN_CHECK_EVERY: u64 = 4_096;

fn missing_stream_error(record_number: u64, stream_name: &str) -> anyhow::Error {
    let message = if stream_name.is_empty() {
        format!("NTFS file {record_number} has no $DATA attribute")
    } else {
        format!("NTFS file {record_number} has no $DATA stream named '{stream_name}'")
    };
    MissingDataStream { message }.into()
}

/// MFT records 0–15 are reserved for NTFS metadata files (`$MFT`, `$LogFile`,
/// `$Volume`, ...). Surfacing them as user-visible "deleted" entries is noise.
const SYSTEM_RESERVED_RECORDS: u64 = 16;

/// Hard cap on how many deleted records we keep per partition. Large NTFS
/// volumes can have millions of MFT slots; we'd burn memory and flood the
/// renderer if a single directory expansion attached every one of them.
const DELETED_SCAN_MAX_RESULTS: usize = 50_000;

pub struct NtfsVolume<'a> {
    backing: PartitionBacking<'a>,
    source: NtfsSource,
    ntfs: Ntfs,
}

impl<'a> NtfsVolume<'a> {
    pub fn open(image: &'a DiskImage, partition: &Partition) -> Result<Self> {
        let backing = PartitionBacking::Image {
            image,
            base_offset: partition.start_lba * image.sector_size as u64,
            length_bytes: partition.length_lba * image.sector_size as u64,
            layout: None,
        };
        Self::from_backing(backing, NtfsSource::Partition(partition.id))
    }

    /// Opens an NTFS volume that lives inside an LVM2 logical volume on
    /// `partition`. The `lv_index` is the position in
    /// [`LvmVolume::logical_volumes`].
    pub fn open_lvm(image: &'a DiskImage, partition: &Partition, lv_index: usize) -> Result<Self> {
        let lvm = LvmVolume::open(image, partition)?;
        let lv = lvm
            .logical_volumes
            .get(lv_index)
            .ok_or_else(|| anyhow!("logical volume not found: lvm:{}:{lv_index}", partition.id))?;
        let (base_offset, layout) = lv.reader_geometry()?;
        let backing = PartitionBacking::Image {
            image,
            base_offset,
            length_bytes: lv.size_bytes,
            layout,
        };
        Self::from_backing(
            backing,
            NtfsSource::Lvm {
                part_id: partition.id,
                lv_index,
            },
        )
    }

    /// Opens an NTFS volume backed by an already-unlocked BitLocker volume.
    pub fn open_bitlocker(unlocked: Arc<UnlockedVolume>, partition_id: u8) -> Result<Self> {
        let length_bytes = unlocked.size();
        let backing = PartitionBacking::Bitlocker {
            unlocked,
            length_bytes,
        };
        Self::from_backing(backing, NtfsSource::Partition(partition_id))
    }

    fn from_backing(backing: PartitionBacking<'a>, source: NtfsSource) -> Result<Self> {
        let mut reader = backing.reader();
        let mut ntfs = Ntfs::new(&mut reader)
            .map_err(|err| anyhow!("failed to read NTFS structures: {err}"))?;
        // Loading the upcase table enables case-insensitive lookups but is not
        // mandatory for plain enumeration; ignore failures so volumes with a
        // damaged $UpCase still list.
        if let Err(err) = ntfs.read_upcase_table(&mut reader) {
            tracing::debug!("NTFS $UpCase table not loaded: {err}");
        }
        Ok(Self {
            backing,
            source,
            ntfs,
        })
    }

    /// Where this NTFS volume lives — used as the cache key for the deleted
    /// snapshot and as the namespace for the IDs this volume emits.
    pub fn source(&self) -> NtfsSource {
        self.source
    }

    fn reader(&self) -> PartitionReader<'_> {
        self.backing.reader()
    }

    /// Total MFT record count derived from `$MFT`'s own `$DATA` length.
    ///
    /// The count is taken from `$MFT` (record 0) only — never from `$MFTMirr`
    /// (record 1), whose `$DATA` (and whose `$DATA` runlist) describes just the
    /// ~4-record mirror region, so any count derived from it would silently
    /// truncate the deleted scan to nothing. We first try record 0's
    /// `$DATA` valid length, then fall back to its `$DATA` *allocated run span*
    /// (covering a record 0 whose valid-length header reads 0 but whose runlist
    /// is intact), and bail when neither is usable.
    pub fn mft_total_records(&self) -> Result<u64> {
        let mut fs = self.reader();
        let record_size = self.ntfs.file_record_size().max(1) as u64;
        // Primary: $MFT (record 0) own $DATA valid length.
        if let Some(total) = self.records_from_data_length(0, &mut fs, record_size) {
            if total > 0 {
                return Ok(total);
            }
        }
        // Fallback: the allocated byte span of $MFT's own $DATA runlist — this
        // genuinely covers the full $MFT, unlike $MFTMirr's runlist (~4 records).
        if let Some(total) = self.records_from_data_run_span(0, &mut fs, record_size) {
            if total > 0 {
                tracing::debug!("NTFS $MFT record-count via $DATA run span: {total}");
                return Ok(total);
            }
        }
        anyhow::bail!("NTFS could not determine MFT record count from $MFT")
    }

    /// `$DATA` valid length of MFT record `rec`, divided into records. Only
    /// meaningful for record 0 ($MFT itself); never call on $MFTMirr, whose own
    /// `$DATA` is the ~4-record mirror, not the real MFT size.
    fn records_from_data_length(
        &self,
        rec: u64,
        fs: &mut PartitionReader<'_>,
        record_size: u64,
    ) -> Option<u64> {
        let file = self.ntfs.file(fs, rec).ok()?;
        let item = file.data(fs, "")?.ok()?;
        let attr = item.to_attribute().ok()?;
        Some(attr.value_length() / record_size)
    }

    /// Total records derived from the *allocated run span* of MFT record `rec`'s
    /// `$DATA` runlist. Only sound for record 0 ($MFT), whose runlist describes
    /// the whole on-disk `$MFT` extent and so yields the true MFT byte span even
    /// when the valid-length header reads 0.
    fn records_from_data_run_span(
        &self,
        rec: u64,
        fs: &mut PartitionReader<'_>,
        record_size: u64,
    ) -> Option<u64> {
        let file = self.ntfs.file(fs, rec).ok()?;
        let item = file.data(fs, "")?.ok()?;
        let attribute = item.to_attribute().ok()?;
        let value = match attribute.value(fs).ok()? {
            ntfs::attribute_value::NtfsAttributeValue::NonResident(value) => value,
            _ => return None,
        };
        let mut span: u64 = 0;
        for run in value.data_runs() {
            let Ok(run) = run else { break };
            span = span.saturating_add(run.allocated_size());
        }
        (span > 0).then(|| span / record_size)
    }

    fn node_id(&self, record: u64) -> String {
        match self.source {
            NtfsSource::Partition(part) => format!("ntfs:part:{part}:{record}"),
            NtfsSource::Lvm { part_id, lv_index } => {
                format!("ntfs:lvm:{part_id}:{lv_index}:{record}")
            }
        }
    }

    fn parent_id(&self, record: u64) -> String {
        if record == ROOT_RECORD {
            match self.source {
                NtfsSource::Partition(part) => format!("part:{part}"),
                NtfsSource::Lvm { part_id, lv_index } => format!("lvm:{part_id}:{lv_index}"),
            }
        } else {
            self.node_id(record)
        }
    }
}

pub fn parse_ntfs_id(id: &str) -> Option<(NtfsSource, u64)> {
    let mut parts = id.split(':');
    match parts.next()? {
        "ntfs" => {}
        _ => return None,
    }
    match parts.next()? {
        "part" => {
            let part_id = parts.next()?.parse().ok()?;
            let record = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            Some((NtfsSource::Partition(part_id), record))
        }
        "lvm" => {
            let part_id = parts.next()?.parse().ok()?;
            let lv_index = parts.next()?.parse().ok()?;
            let record = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            Some((NtfsSource::Lvm { part_id, lv_index }, record))
        }
        _ => None,
    }
}

/// Parse a `ntfs-ads:{source}:{record}:{hex(stream_name)}` ID — the pseudo-file
/// id minted for a named NTFS Alternate Data Stream surfaced as an independent
/// file row (the NTFS analog of `refs-snap:`). `{source}:{record}` is exactly
/// the `ntfs:`-id tail (`part:N:record` or `lvm:N:M:record`); the stream name
/// is hex-encoded so arbitrary bytes / `:` in it are safe. Returns the source,
/// the owning MFT record, and the decoded stream name.
pub fn parse_ntfs_ads_id(id: &str) -> Option<(NtfsSource, u64, String)> {
    let rest = id.strip_prefix("ntfs-ads:")?;
    let (head, hex_stream) = rest.rsplit_once(':')?;
    let stream = String::from_utf8(hex::decode(hex_stream).ok()?).ok()?;
    if stream.is_empty() {
        return None;
    }
    let (source, record) = parse_ntfs_id(&format!("ntfs:{head}"))?;
    Some((source, record, stream))
}

fn data_value_length<T: Read + Seek>(file: &ntfs::NtfsFile, fs: &mut T) -> Option<u64> {
    let item = match file.data(fs, "")? {
        Ok(item) => item,
        Err(err) => {
            tracing::trace!("NTFS deleted-record $DATA error: {err}");
            return None;
        }
    };
    let attr = match item.to_attribute() {
        Ok(attr) => attr,
        Err(err) => {
            tracing::trace!("NTFS deleted-record $DATA attribute error: {err}");
            return None;
        }
    };
    if !matches!(attr.ty().ok()?, NtfsAttributeType::Data) {
        return None;
    }
    Some(attr.value_length())
}

/// EFS requires the source user's keys and must not expose ciphertext as data.
fn reject_unreadable_attribute(flags: NtfsAttributeFlags, record_number: u64) -> Result<()> {
    if flags.contains(NtfsAttributeFlags::ENCRYPTED) {
        anyhow::bail!("NTFS 记录 {record_number} 的 $DATA 为 EFS 加密流，无法读取");
    }
    Ok(())
}

fn file_name_timestamps(name: &NtfsFileName) -> Mactb {
    Mactb {
        modified: ntfs_time_to_rfc3339(name.modification_time()),
        accessed: ntfs_time_to_rfc3339(name.access_time()),
        created: ntfs_time_to_rfc3339(name.creation_time()),
        mft_changed: ntfs_time_to_rfc3339(name.mft_record_modification_time()),
        birth: ntfs_time_to_rfc3339(name.creation_time()),
    }
}

fn ntfs_time_to_rfc3339(time: NtfsTime) -> Option<String> {
    // NtfsTime stores 100ns intervals since 1601-01-01 UTC; convert to a
    // chrono::DateTime via the well-known FILETIME -> Unix epoch offset.
    const FILETIME_UNIX_OFFSET_SECS: i64 = 11_644_473_600;
    let raw = time.nt_timestamp();
    if raw == 0 {
        return None;
    }
    let secs_since_filetime = (raw / 10_000_000) as i64;
    let nanos = ((raw % 10_000_000) * 100) as u32;
    let unix_secs = secs_since_filetime.checked_sub(FILETIME_UNIX_OFFSET_SECS)?;
    Utc.timestamp_opt(unix_secs, nanos)
        .single()
        .map(|dt| dt.to_rfc3339())
}

#[cfg(test)]
mod tests;
