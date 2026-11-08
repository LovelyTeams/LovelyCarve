//! Public read/stat facade for filesystem tree nodes.
//!
//! `FsSession` owns the detailed per-filesystem dispatch and request-scoped
//! volume cache. This module keeps the externally used `fs_tree::read_*` and
//! `fs_tree::file_stat` wrappers small while preserving their old paths through
//! re-exports from `fs_tree`, and holds the session's ranged readers
//! ([`FsSession::read_range_direct`], [`FsSession::stream_file_range`]).

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use crate::filesystem::docker::host_docker;
use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{open_ntfs_for_source, FsSession, FsStat};
use crate::filesystem::unalloc::parse_virtual_unalloc_id;
use crate::filesystem::volumes::ad1 as ad1_volume;
use crate::filesystem::volumes::docker as docker_volume;
use crate::filesystem::volumes::folder as folder_volume;
use crate::filesystem::volumes::ntfs::parse_ntfs_ads_id;
use crate::filesystem::volumes::refs::{open_refs_source, parse_refs_snap_id};
use crate::filesystem::volumes::sevenz as sevenz_volume;
use crate::filesystem::volumes::tar as tar_volume;
use crate::filesystem::volumes::zip as zip_volume;

pub fn read_file(image: &Arc<EvidenceSession>, node_id: &str) -> anyhow::Result<Vec<u8>> {
    read_file_limited(image, node_id, 128 * 1024 * 1024)
}

pub fn read_file_limited(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    max_bytes: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut session = FsSession::new(image);
    session.read_file_limited(node_id, max_bytes)
}

/// Read a bounded logical range from a file without materialising the whole
/// file. Filesystems implementing `Volume::read_range` (NTFS today) seek
/// directly; the others still stream and skip the preceding bytes, with memory
/// bounded and the visitor aborting as soon as the requested range is complete.
/// Callers scanning many files should hold one `FsSession` and use
/// `FsSession::read_file_range` instead — that caches the volume per root.
pub fn read_file_range(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    offset: u64,
    length: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut session = FsSession::new(image);
    session.read_file_range(node_id, offset, length)
}

pub fn file_stat(image: &Arc<EvidenceSession>, node_id: &str) -> anyhow::Result<FsStat> {
    let mut session = FsSession::new(image);
    session.file_stat(node_id)
}

pub fn stream_file_chunks<F>(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    chunk_size: usize,
    mut visitor: F,
) -> anyhow::Result<u64>
where
    F: FnMut(&[u8]) -> anyhow::Result<()>,
{
    let image = image.filesystem_session(node_id);
    image.require_source()?;
    if crate::filesystem::volumes::lvm::parse_lvm_id(node_id).is_some() {
        let volume = crate::filesystem::volumes::lvm::logical_volume_for_id(image, node_id)?;
        let layout = volume.readable_layout()?;
        let mut offset = 0u64;
        while offset < layout.size {
            let bytes = layout.read(image.as_ref(), offset, chunk_size.max(1))?;
            if bytes.is_empty() {
                anyhow::bail!("LVM LV ended before EOF");
            }
            visitor(&bytes)?;
            offset += bytes.len() as u64;
        }
        return Ok(offset);
    }
    if let Some(id) = host_docker::parse_host_docker_id(node_id) {
        return host_docker::stream_file(image, &id, chunk_size, visitor);
    }

    if let Some(path) = docker_volume::parse_docker_id(node_id) {
        return docker_volume::stream_file(image, &path, chunk_size, visitor);
    }

    if let Some(rel) = folder_volume::parse_folder_id(node_id) {
        return folder_volume::stream_file(image, &rel, chunk_size, visitor);
    }

    if let Some(rel) = tar_volume::parse_tar_id(node_id) {
        return tar_volume::stream_file(image, &rel, chunk_size, visitor);
    }

    if let Some(rel) = zip_volume::parse_zip_id(node_id) {
        return zip_volume::stream_file(image, &rel, chunk_size, visitor);
    }

    if let Some(rel) = sevenz_volume::parse_sevenz_id(node_id) {
        return sevenz_volume::stream_file(image, &rel, chunk_size, visitor);
    }

    if let Some(id) = ad1_volume::parse_ad1_id(node_id) {
        return ad1_volume::stream_file(image, id, chunk_size, visitor);
    }

    if let Some((offset, len)) = parse_virtual_unalloc_id(node_id) {
        let mut pos = offset;
        let mut remaining = len;
        while remaining > 0 {
            let want = remaining.min(chunk_size as u64) as usize;
            let buf = image.read_range(pos, want)?;
            if buf.is_empty() {
                break;
            }
            let read = buf.len() as u64;
            visitor(&buf)?;
            pos += read;
            remaining -= read;
        }
        return Ok(len - remaining);
    }

    if let Some((source, path, stream)) = parse_refs_snap_id(node_id) {
        return open_refs_source(image, source)?.stream_named_stream(
            &path,
            &stream,
            chunk_size,
            |chunk| visitor(chunk),
        );
    }

    if let Some((source, record, stream)) = parse_ntfs_ads_id(node_id) {
        return open_ntfs_for_source(image, source)?.stream_data_stream(
            record,
            &stream,
            chunk_size,
            |chunk| visitor(chunk),
        );
    }

    if let Some((offset, len)) = crate::filesystem::deleted::parse_run_id(node_id) {
        let mut pos = offset;
        let mut remaining = len;
        while remaining > 0 {
            let want = remaining.min(chunk_size as u64) as usize;
            let buf = image.read_range(pos, want)?;
            if buf.is_empty() {
                break;
            }
            let read = buf.len() as u64;
            visitor(&buf)?;
            pos += read;
            remaining -= read;
        }
        return Ok(len - remaining);
    }

    let mut visitor = visitor;
    match crate::filesystem::fs_dispatch::dispatch(image, node_id)? {
        Some(volume) => volume.stream_file(node_id, chunk_size, &mut visitor),
        None => anyhow::bail!("unsupported filesystem node id: {node_id}"),
    }
}

/// Visitor marker that stops the fallback stream once the range is delivered.
#[derive(Debug)]
struct RangeDelivered;

impl fmt::Display for RangeDelivered {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("requested file range delivered")
    }
}

impl Error for RangeDelivered {}

/// The image span of a node that is one contiguous byte range of the image:
/// virtual unallocated / disk-gap / system-area files and deleted-entry runs.
fn contiguous_image_span(node_id: &str) -> Option<(u64, u64)> {
    parse_virtual_unalloc_id(node_id).or_else(|| crate::filesystem::deleted::parse_run_id(node_id))
}

impl FsSession<'_> {
    /// Random-access read of `[offset, offset + length)` (end-of-file
    /// truncated) for nodes that support it: contiguous image spans, AD1
    /// files (independent zlib chunks) and volumes implementing
    /// `Volume::read_range`. `Ok(None)` means the node can only be streamed
    /// from its start.
    pub(crate) fn read_range_direct(
        &mut self,
        node_id: &str,
        offset: u64,
        length: usize,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if crate::filesystem::volumes::lvm::parse_lvm_id(node_id).is_some() {
            let image = self.image().filesystem_session(node_id);
            let volume = crate::filesystem::volumes::lvm::logical_volume_for_id(image, node_id)?;
            return volume
                .readable_layout()?
                .read(image.as_ref(), offset, length)
                .map(Some);
        }
        if let Some((source, record, stream)) = parse_ntfs_ads_id(node_id) {
            return open_ntfs_for_source(self.image(), source)?
                .read_data_stream_range(record, &stream, offset, length)
                .map(Some);
        }
        if let Some(rel) = folder_volume::parse_folder_id(node_id) {
            let folder = self
                .image()
                .folder
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("not a folder image"))?;
            return folder.read_ios_range(&rel, offset, length);
        }
        if let Some(id) = ad1_volume::parse_ad1_id(node_id) {
            return ad1_volume::read_range(self.image(), id, offset, length).map(Some);
        }
        if let Some((base, span_len)) = contiguous_image_span(node_id) {
            if offset >= span_len {
                return Ok(Some(Vec::new()));
            }
            let take = usize::try_from((span_len - offset).min(u64::try_from(length)?))?;
            let start = base
                .checked_add(offset)
                .ok_or_else(|| anyhow::anyhow!("image span offset overflow: {node_id}"))?;
            return self
                .image()
                .filesystem_session(node_id)
                .read_range(start, take)
                .map(Some);
        }
        match self.volume(node_id)? {
            Some(volume) => volume.read_range(node_id, offset, length),
            None => Ok(None),
        }
    }

    /// Stream the logical bytes `[offset, offset + length)` of `node_id` in
    /// `chunk_size` slabs (end-of-file truncated) and return how many bytes
    /// were delivered. Seekable nodes (see [`Self::read_range_direct`]) read
    /// only the range; the rest stream from the file start and skip the prefix,
    /// which costs O(offset). A visitor error stops the stream and is returned
    /// unchanged.
    pub fn stream_file_range<F>(
        &mut self,
        node_id: &str,
        offset: u64,
        length: u64,
        chunk_size: usize,
        mut visitor: F,
    ) -> anyhow::Result<u64>
    where
        F: FnMut(&[u8]) -> anyhow::Result<()>,
    {
        let end = offset.saturating_add(length);
        if offset >= end {
            return Ok(0);
        }
        if let Some(id) = host_docker::parse_host_docker_id(node_id) {
            let source_id = host_docker::source_file_id(self.image(), &id)?;
            return self.stream_file_range(&source_id, offset, length, chunk_size, visitor);
        }
        let slab = u64::try_from(chunk_size.max(1))?;
        let mut position = offset;
        while position < end {
            let want = usize::try_from(slab.min(end - position))?;
            let bytes = match self.read_range_direct(node_id, position, want)? {
                Some(bytes) => bytes,
                None if position == offset => {
                    return self.stream_range_skipping(node_id, offset, end, chunk_size, visitor);
                }
                None => anyhow::bail!("node stopped supporting ranged reads: {node_id}"),
            };
            if bytes.is_empty() {
                break;
            }
            visitor(&bytes)?;
            position += bytes.len() as u64;
            if bytes.len() < want {
                break;
            }
        }
        Ok(position - offset)
    }

    /// [`Self::stream_file_range`] for nodes without random access: stream from
    /// the start, hand over only the bytes inside `[offset, end)` and stop once
    /// the range is complete.
    fn stream_range_skipping<F>(
        &mut self,
        node_id: &str,
        offset: u64,
        end: u64,
        chunk_size: usize,
        mut visitor: F,
    ) -> anyhow::Result<u64>
    where
        F: FnMut(&[u8]) -> anyhow::Result<()>,
    {
        let mut stream_offset = 0u64;
        let mut delivered = 0u64;
        let result = self.stream_file(node_id, chunk_size, |chunk| {
            let chunk_end = stream_offset + chunk.len() as u64;
            if chunk_end > offset && stream_offset < end {
                let from = usize::try_from(offset.saturating_sub(stream_offset))?;
                let to = usize::try_from(end.min(chunk_end) - stream_offset)?;
                visitor(&chunk[from..to])?;
                delivered += (to - from) as u64;
            }
            stream_offset = chunk_end;
            if stream_offset >= end {
                return Err(RangeDelivered.into());
            }
            Ok(())
        });
        match result {
            Ok(_) => Ok(delivered),
            Err(err) if err.downcast_ref::<RangeDelivered>().is_some() => Ok(delivered),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::filesystem::evidence_session::EvidenceSession;
    use crate::filesystem::tree::fs_tree::FsSession;

    use super::read_file_range;

    fn collect_range(
        session: &mut FsSession<'_>,
        node_id: &str,
        offset: u64,
        length: u64,
    ) -> (u64, Vec<u8>) {
        let mut collected = Vec::new();
        let delivered = session
            .stream_file_range(node_id, offset, length, 64 * 1024, |chunk| {
                collected.extend_from_slice(chunk);
                Ok(())
            })
            .expect("stream file range");
        (delivered, collected)
    }

    #[test]
    fn ranged_stream_skips_the_prefix_of_unseekable_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let data = (0..3_000_000)
            .map(|index| (index % 253) as u8)
            .collect::<Vec<_>>();
        std::fs::write(temp.path().join("range.bin"), &data).expect("write range fixture");
        let image = Arc::new(EvidenceSession::open(temp.path()).expect("open folder evidence"));
        let node_id = format!("folder:{}", hex::encode("range.bin"));
        let mut session = FsSession::new(&image);

        let (delivered, bytes) = collect_range(&mut session, &node_id, 1_048_000, 1_500_000);
        assert_eq!(delivered, 1_500_000);
        assert_eq!(bytes, data[1_048_000..2_548_000]);

        let (delivered, bytes) = collect_range(&mut session, &node_id, 2_900_000, 1_000_000);
        assert_eq!(delivered, 100_000);
        assert_eq!(bytes, data[2_900_000..]);
    }

    #[test]
    fn ranged_stream_reads_contiguous_image_spans_directly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("disk.raw");
        let data = (0..(2 * 1024 * 1024 + 123))
            .map(|index| (index % 241) as u8)
            .collect::<Vec<_>>();
        std::fs::write(&path, &data).expect("write raw fixture");
        let image = Arc::new(EvidenceSession::open(&path).expect("open raw image"));
        let span_start = 4096usize;
        let node_id = format!("diskgap:{span_start:#x}:{:#x}", 1_000_000);
        let mut session = FsSession::new(&image);

        let (delivered, bytes) = collect_range(&mut session, &node_id, 10_000, 500_000);
        assert_eq!(delivered, 500_000);
        assert_eq!(bytes, data[span_start + 10_000..span_start + 510_000]);

        // The span end truncates like end of file.
        let (delivered, bytes) = collect_range(&mut session, &node_id, 900_000, 500_000);
        assert_eq!(delivered, 100_000);
        assert_eq!(bytes, data[span_start + 900_000..span_start + 1_000_000]);
        assert_eq!(
            read_file_range(&image, &node_id, 900_000, 500_000).expect("ranged read"),
            bytes
        );
    }

    #[test]
    fn ranged_read_is_bounded_and_checks_overflow() {
        let temp = tempfile::tempdir().expect("tempdir");
        let data = (0..(2 * 1024 * 1024 + 257))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        std::fs::write(temp.path().join("range.bin"), &data).expect("write range fixture");
        let image = Arc::new(EvidenceSession::open(temp.path()).expect("open folder evidence"));
        let node_id = format!("folder:{}", hex::encode("range.bin"));

        let offset = 1024 * 1024 - 11;
        let actual = read_file_range(&image, &node_id, offset as u64, 100)
            .expect("read range across stream chunks");
        assert_eq!(actual, data[offset..offset + 100]);
        assert!(read_file_range(&image, &node_id, u64::MAX, 2).is_err());
    }
}
