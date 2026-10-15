//! LDPlayer (雷电模拟器) `.ldbk` backup loader.
//!
//! A `.ldbk` file is a non-solid 7z archive holding an emulator instance's
//! virtual disks (`*.vmdk`, each a `monolithicSparse` VMDK that carries its OWN
//! MBR + Linux/ext4 partitions) plus a small `leidian.config` JSON. LDPlayer
//! writes two flavours — LZMA2-compressed and stored (`Method = Copy`,
//! "uncompressed").
//!
//! Both are read without extracting anything to disk:
//!   * **stored** members are mmapped in place — the VMDK bytes already live
//!     contiguously inside the `.ldbk`, so we compute their byte range from the
//!     7z stream map and point a [`SparseVmdk`] reader straight at the mmap
//!     window. A 256 GiB sparse disk only pages in the grains actually touched,
//!     so loading is near-instant;
//!   * **compressed** members are decoded once into RAM (no tempfile, no disk
//!     write) and read from there.
//!
//! Either way each VMDK becomes a [`SparseVmdk`] `SectorSource`; the disks are
//! laid out back-to-back into one synthetic block stream. Because each VMDK has
//! its OWN MBR at its media offset 0, a single whole-stream scan would only see
//! the first disk's table, so partition detection runs PER-VMDK against a
//! [`PartitionSlice`] view and the LBAs are re-based into the concat. The result
//! flows through the normal `part:N` navigation pipeline like a real
//! multi-partition disk. Embedded VMDKs are decoded without temporary files.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context};
use memmap2::Mmap;
use sevenz_rust2::{Archive, ArchiveReader, Password, SIGNATURE_HEADER_SIZE};

use crate::core::evidence::image::{OpenObserver, OpenProgress};
use crate::core::evidence::partitions::{
    detect_partitions_with_meta, refine_partition_kinds, Partition, PartitionKind, PartitionSlice,
    SectorSource,
};
use crate::core::evidence::readers::vmdk_sparse::{Backing, SparseVmdk};

/// Sector size LDPlayer's MBR-partitioned VMDKs use — `start_lba` 2048 is a
/// 1 MiB-aligned classic MBR. 4Kn never appears in these emulator images.
const LD_SECTOR: usize = 512;

/// Decode buffer for streaming a compressed member into RAM.
const DECODE_CHUNK: usize = 1 << 20;

/// Everything `core::evidence::image` needs to build a `DiskImage` over an `.ldbk`.
pub struct PreparedLdplayer {
    /// Concatenated block stream over every VMDK. Owns the mmap windows / decoded
    /// buffers, so keeping this `Arc` alive keeps the backing bytes alive.
    pub source: Arc<dyn SectorSource + Send + Sync>,
    /// Partitions across all VMDKs, re-based into the concat stream.
    pub partitions: Vec<Partition>,
    pub sector_size: usize,
    /// Total virtual size of the concatenated disks.
    pub size_bytes: u64,
}

/// One VMDK occupying `[base, base+len)` of the concat stream.
struct Segment {
    reader: Arc<dyn SectorSource + Send + Sync>,
    base: u64,
    len: u64,
}

/// The concatenated multi-VMDK block device.
struct LdplayerSource {
    segments: Vec<Segment>,
    total: u64,
}

impl SectorSource for LdplayerSource {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        let end = offset.saturating_add(len as u64).min(self.total);
        let mut pos = offset.min(self.total);
        if pos >= end {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity((end - pos) as usize);
        while pos < end {
            let Some(segment) = self
                .segments
                .iter()
                .find(|s| pos >= s.base && pos < s.base + s.len)
            else {
                // Segments are contiguous by construction, so this is
                // unreachable; bail loudly rather than silently truncate.
                bail!("ldplayer concat gap at offset {pos}");
            };
            let within = pos - segment.base;
            let want = ((segment.base + segment.len).min(end) - pos) as usize;
            let chunk = segment.reader.read(within, want)?;
            let got = chunk.len();
            out.extend_from_slice(&chunk);
            // A short read inside the media (shouldn't happen) is padded with
            // zeros so downstream offsets stay aligned to the segment layout.
            if got < want {
                out.resize(out.len() + (want - got), 0);
            }
            pos += want as u64;
        }
        Ok(out)
    }

    fn size(&self) -> u64 {
        self.total
    }
}

/// How a `*.vmdk` member's bytes are obtained.
enum MemberKind {
    /// Stored (`Method = Copy`): the bytes are contiguous in the `.ldbk` at
    /// `[offset, offset+len)` and read straight from the mmap.
    Stored { offset: u64, len: u64 },
    /// Compressed: decoded into RAM via the 7z reader.
    Compressed,
}

struct Member {
    name: String,
    /// Uncompressed (media-image) size of the member, for decode progress.
    size: u64,
    kind: MemberKind,
}

/// Open an LDPlayer `.ldbk` backup: locate / decode its VMDKs, assemble the
/// concat stream, and detect every partition across all disks. `progress`
/// receives decode + detection phases so the loader is never a blank spinner.
pub fn open(path: &Path, progress: OpenObserver<'_>) -> anyhow::Result<PreparedLdplayer> {
    open_cancellable(path, progress, &tokio_util::sync::CancellationToken::new())
}

pub(crate) fn open_cancellable(
    path: &Path,
    progress: OpenObserver<'_>,
    cancel: &tokio_util::sync::CancellationToken,
) -> anyhow::Result<PreparedLdplayer> {
    anyhow::ensure!(!cancel.is_cancelled(), "image load cancelled");

    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    // SAFETY: the mmap is only read, and the `.ldbk` is evidence we treat as
    // read-only; concurrent truncation by another process is out of scope, as
    // it is for every other mmap-backed source in `core::evidence::image`.
    let mmap =
        Arc::new(unsafe { Mmap::map(&file) }.with_context(|| format!("mmap {}", path.display()))?);

    let mut reader = ArchiveReader::open(path, Password::empty())
        .with_context(|| format!("failed to open 7z archive: {}", path.display()))?;
    let members = collect_members(reader.archive());
    if members.is_empty() {
        bail!("no *.vmdk members in {}", path.display());
    }

    // Decode every compressed member up front, in one streaming pass that
    // reports cumulative byte progress. Stored members are skipped (mmapped in
    // place below), so a fully-stored backup does no decode work at all.
    let decoded = decode_compressed_members(&mut reader, &members, progress, cancel)?;

    // Build one VMDK reader per member, contiguous in the concat stream.
    let mut segments: Vec<Segment> = Vec::new();
    let mut layout: Vec<(String, u64, u64)> = Vec::new();
    let mut base = 0u64;
    for member in &members {
        let backing = match &member.kind {
            MemberKind::Stored { offset, len } => Backing::Mapped {
                mmap: Arc::clone(&mmap),
                base: *offset as usize,
                len: *len as usize,
            },
            MemberKind::Compressed => {
                let data = decoded
                    .get(&member.name)
                    .ok_or_else(|| anyhow!("decoded member missing: {}", member.name))?;
                Backing::Owned(Arc::clone(data))
            }
        };
        let vmdk = SparseVmdk::open(backing)
            .with_context(|| format!("parse VMDK member {}", member.name))?;
        let len = vmdk.size();
        if len == 0 {
            bail!("VMDK member {} reports zero media size", member.name);
        }
        segments.push(Segment {
            reader: Arc::new(vmdk),
            base,
            len,
        });
        layout.push((member.name.clone(), base, len));
        base += len;
    }
    let total = base;

    let concat: Arc<dyn SectorSource + Send + Sync> = Arc::new(LdplayerSource { segments, total });

    // Per-VMDK detection, re-based into the concat. Each VMDK carries its own
    // MBR at its media offset 0, so we feed the dispatcher a slice view bounded
    // to that disk and shift the returned LBAs by the segment base.
    progress(OpenProgress::DetectingPartitions);
    let mut partitions: Vec<Partition> = Vec::new();
    let mut next_id: u16 = 0;
    for (name, seg_base, seg_len) in &layout {
        let stem = name.strip_suffix(".vmdk").unwrap_or(name);
        let view = PartitionSlice::new(concat.as_ref(), *seg_base, *seg_len);
        let detected = detect_partitions_with_meta(&view, LD_SECTOR)
            .with_context(|| format!("partition detection failed for {name}"))?;
        let base_lba = seg_base / LD_SECTOR as u64;

        if detected.partitions.is_empty() {
            // Keep an unrecognised disk navigable in hex by exposing it whole.
            partitions.push(Partition {
                id: take_id(&mut next_id)?,
                kind: PartitionKind::Unknown,
                start_lba: base_lba,
                length_lba: seg_len / LD_SECTOR as u64,
                label: Some(stem.to_string()),
                unique_guid: None,
            });
            continue;
        }

        for mut part in detected.partitions {
            let slot = part.id;
            let detected_label = part.label.take();
            part.id = take_id(&mut next_id)?;
            part.start_lba += base_lba;
            part.label = Some(match detected_label {
                Some(label) if !label.is_empty() => format!("{stem} · {label}"),
                _ => format!("{stem} · p{}", slot + 1),
            });
            partitions.push(part);
        }
    }

    // Promote kinds (Linux 0x83 → Ext4 etc.) by probing each partition's boot
    // region against the whole concat at its now-absolute offset.
    refine_partition_kinds(concat.as_ref(), &mut partitions, LD_SECTOR)
        .context("refining LDPlayer partition kinds")?;

    Ok(PreparedLdplayer {
        source: concat,
        partitions,
        sector_size: LD_SECTOR,
        size_bytes: total,
    })
}

/// Index the `*.vmdk` members of the archive, deciding for each whether it is
/// stored (mmappable in place) or compressed (must be decoded). Sorted by name
/// so `data.vmdk` precedes `sdcard.vmdk` for a stable disk order.
fn collect_members(archive: &Archive) -> Vec<Member> {
    let mut members: Vec<Member> = archive
        .files
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.has_stream && !entry.is_directory && is_vmdk(entry.name()))
        .map(|(index, entry)| Member {
            name: entry.name().to_string(),
            size: entry.size(),
            kind: match stored_range(archive, index, entry.size()) {
                Some((offset, len)) => MemberKind::Stored { offset, len },
                None => MemberKind::Compressed,
            },
        })
        .collect();
    members.sort_by(|a, b| a.name.cmp(&b.name));
    members
}

/// Decode every compressed member into RAM in a single streaming pass, reporting
/// cumulative byte progress so a long LZMA2 decode is never a blank spinner.
/// Stored members are skipped here (they are mmapped in place), so a fully
/// stored backup returns an empty map without touching the decoder.
fn decode_compressed_members(
    reader: &mut ArchiveReader<File>,
    members: &[Member],
    progress: OpenObserver<'_>,
    cancel: &tokio_util::sync::CancellationToken,
) -> anyhow::Result<HashMap<String, Arc<Vec<u8>>>> {
    let wanted: std::collections::HashSet<&str> = members
        .iter()
        .filter(|m| matches!(m.kind, MemberKind::Compressed))
        .map(|m| m.name.as_str())
        .collect();
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
    let grand_total: u64 = members
        .iter()
        .filter(|m| matches!(m.kind, MemberKind::Compressed))
        .map(|m| m.size)
        .sum();

    let mut decoded: HashMap<String, Arc<Vec<u8>>> = HashMap::new();
    let mut done = 0u64;
    reader
        .for_each_entries(|entry, rd| {
            if !wanted.contains(entry.name()) {
                return Ok(true);
            }
            let mut buf: Vec<u8> = Vec::with_capacity(entry.size() as usize);
            let mut chunk = vec![0u8; DECODE_CHUNK];
            loop {
                if cancel.is_cancelled() {
                    return Err(std::io::Error::other("image load cancelled").into());
                }
                let read = rd.read(&mut chunk)?;
                if read == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..read]);
                done += read as u64;
                progress(OpenProgress::Decompressing {
                    label: entry.name().to_string(),
                    done,
                    total: grand_total,
                });
            }
            decoded.insert(entry.name().to_string(), Arc::new(buf));
            Ok(true)
        })
        .map_err(|err| anyhow!("decompress 7z members: {err}"))?;
    Ok(decoded)
}

/// Absolute `(offset, len)` of a member's bytes within the `.ldbk`, but only
/// when it is stored uncompressed in its own pack stream — i.e. a single `Copy`
/// coder whose pack-stream size equals the file size (so no other member shares
/// the stream). Returns `None` for compressed / solid members, which must go
/// through the decoder. Mirrors `ArchiveReader::read_file`'s own offset maths.
fn stored_range(archive: &Archive, file_index: usize, file_size: u64) -> Option<(u64, u64)> {
    let block_index = (*archive.stream_map.file_block_index.get(file_index)?)?;
    let block = archive.blocks.get(block_index)?;
    // 7z "Copy" (store) is the single-byte method id 0x00.
    let id = block.coders.first()?.encoder_method_id();
    if block.coders.len() != 1 || id.len() != 1 || id[0] != 0 {
        return None;
    }
    let pack_index = *archive
        .stream_map
        .block_first_pack_stream_index()
        .get(block_index)?;
    // The member must own the whole pack stream (rules out solid sharing).
    if *archive.pack_sizes().get(pack_index)? != file_size {
        return None;
    }
    let pack_offset = *archive.stream_map.pack_stream_offsets().get(pack_index)?;
    Some((
        SIGNATURE_HEADER_SIZE + archive.pack_pos() + pack_offset,
        file_size,
    ))
}

/// Allocate the next collision-free partition id from a `u16` counter, erroring
/// only once the whole `u8` id space (`Partition::id`) is exhausted — so the
/// last valid id 255 is still handed out (a handful of partitions in practice).
fn take_id(next: &mut u16) -> anyhow::Result<u8> {
    let id = u8::try_from(*next).map_err(|_| anyhow!("too many LDPlayer partitions"))?;
    *next += 1;
    Ok(id)
}

/// True for archive members that are VMDK virtual disks (case-insensitive).
fn is_vmdk(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".vmdk")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opens the real LDPlayer backup at `LOVELYCARVE_LDBK_TEST` (compressed or
    /// stored) and asserts it decomposes into navigable ext4 partitions. Skipped
    /// when the env var is unset (the fixture is large).
    #[test]
    fn opens_real_ldbk_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_LDBK_TEST") else {
            return;
        };
        let phases = std::sync::Mutex::new(Vec::<&'static str>::new());
        let observer = |p: OpenProgress| {
            let name = match p {
                OpenProgress::Opening => "opening",
                OpenProgress::Decompressing { .. } => "decompressing",
                OpenProgress::DetectingPartitions => "partitions",
                OpenProgress::DetectingFilesystems { .. } => "filesystems",
            };
            phases.lock().expect("phase lock").push(name);
        };
        let prepared = open(Path::new(&path), &observer).expect("LDPlayer backup should open");
        // The loader must report progress, not run silently — at minimum the
        // partition-detection phase fires for every backup.
        assert!(
            phases.lock().expect("phase lock").contains(&"partitions"),
            "expected a DetectingPartitions progress event"
        );
        assert!(prepared.size_bytes > 0, "concat size must be non-zero");
        assert!(
            !prepared.partitions.is_empty(),
            "at least one partition expected"
        );
        // The userdata + sdcard VMDKs are ext4-formatted, so detection must
        // surface at least one ext4 volume.
        assert!(
            prepared
                .partitions
                .iter()
                .any(|p| p.kind == PartitionKind::Ext4),
            "expected an ext4 partition, got {:?}",
            prepared
                .partitions
                .iter()
                .map(|p| p.kind)
                .collect::<Vec<_>>()
        );
        // First sector of the first partition must be readable through the
        // concat / sparse-VMDK stack.
        let first = &prepared.partitions[0];
        let bytes = prepared
            .source
            .read(first.start_lba * LD_SECTOR as u64, 512)
            .expect("first partition sector should read");
        assert_eq!(bytes.len(), 512);
    }

    #[test]
    fn is_vmdk_is_case_insensitive() {
        assert!(is_vmdk("data.vmdk"));
        assert!(is_vmdk("SDCARD.VMDK"));
        assert!(!is_vmdk("leidian.config"));
        assert!(!is_vmdk("notes.vmdk.bak"));
    }

    #[test]
    fn take_id_increments_and_guards_overflow() {
        let mut next = 254u16;
        assert_eq!(take_id(&mut next).unwrap(), 254);
        assert_eq!(take_id(&mut next).unwrap(), 255);
        // 256 no longer fits in `u8`, so the id space is exhausted.
        assert!(take_id(&mut next).is_err());
    }
}
