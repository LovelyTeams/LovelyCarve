//! Filesystem session read operations.
use super::*;

impl<'a> FsSession<'a> {
    /// Read a bounded logical file using this session's cached volume when the
    /// node belongs to a cacheable filesystem. This mirrors
    /// [`read_file_limited`] for synthetic evidence, virtual unallocated
    /// ranges, named streams and deleted-entry byte ranges, while avoiding a second
    /// `fs_dispatch::dispatch` open when the same request already used this
    /// session for `children`, `stat` or another stream.
    pub fn read_file_limited(
        &mut self,
        node_id: &str,
        max_bytes: usize,
    ) -> anyhow::Result<Vec<u8>> {
        let image = self.image.filesystem_session(node_id);
        image.require_source()?;
        if parse_lvm_id(node_id).is_some() {
            let volume = crate::filesystem::volumes::lvm::logical_volume_for_id(image, node_id)?;
            return volume.readable_layout()?.read(image.as_ref(), 0, max_bytes);
        }
        if let Some(id) = host_docker::parse_host_docker_id(node_id) {
            return host_docker::read_file(image, &id, max_bytes);
        }

        if let Some(path) = docker_volume::parse_docker_id(node_id) {
            return docker_volume::read_file(image, &path, max_bytes);
        }

        if let Some(rel) = folder_volume::parse_folder_id(node_id) {
            return folder_volume::read_file(image, &rel, max_bytes);
        }

        if let Some(rel) = tar_volume::parse_tar_id(node_id) {
            return tar_volume::read_file(image, &rel, max_bytes);
        }

        if let Some(rel) = zip_volume::parse_zip_id(node_id) {
            return zip_volume::read_file(image, &rel, max_bytes);
        }

        if let Some(rel) = sevenz_volume::parse_sevenz_id(node_id) {
            return sevenz_volume::read_file(image, &rel, max_bytes);
        }

        if let Some(id) = ad1_volume::parse_ad1_id(node_id) {
            return ad1_volume::read_file(image, id, max_bytes);
        }

        if let Some((offset, len)) = parse_virtual_unalloc_id(node_id) {
            return image.read_range(offset, (len as usize).min(max_bytes));
        }

        if let Some((source, path, stream)) = parse_refs_snap_id(node_id) {
            return open_refs_source(image, source)?.read_named_stream(&path, &stream, max_bytes);
        }

        if let Some((source, record, stream)) = parse_ntfs_ads_id(node_id) {
            return open_ntfs_for_source(image, source)?
                .read_data_stream(record, &stream, max_bytes);
        }

        if let Some(result) = crate::filesystem::deleted::read(image, node_id, max_bytes) {
            return result;
        }

        if let Some(volume) = self.volume(node_id)? {
            return volume.read_file(node_id, max_bytes);
        }

        match crate::filesystem::fs_dispatch::dispatch(image, node_id)? {
            Some(volume) => volume.read_file(node_id, max_bytes),
            None => anyhow::bail!("unsupported filesystem node id: {node_id}"),
        }
    }

    /// Return metadata for `node_id`, reusing this session's cached volume for
    /// regular filesystem nodes. Behaviour matches [`file_stat`] for synthetic
    /// evidence roots, virtual unallocated files, named streams and deleted
    /// directory entries.
    pub fn file_stat(&mut self, node_id: &str) -> anyhow::Result<FsStat> {
        let image = self.image.filesystem_session(node_id);
        image.require_source()?;
        if parse_lvm_id(node_id).is_some() {
            return crate::filesystem::volumes::lvm::logical_volume_stat(image, node_id);
        }
        if let Some(id) = host_docker::parse_host_docker_id(node_id) {
            return host_docker::file_stat(image, &id, node_id);
        }

        if let Some(path) = docker_volume::parse_docker_id(node_id) {
            return docker_volume::file_stat(image, &path, node_id);
        }

        if let Some(rel) = folder_volume::parse_folder_id(node_id) {
            return folder_volume::file_stat(image, &rel, node_id);
        }

        if let Some(rel) = tar_volume::parse_tar_id(node_id) {
            return tar_volume::file_stat(image, &rel, node_id);
        }

        if let Some(rel) = zip_volume::parse_zip_id(node_id) {
            return zip_volume::file_stat(image, &rel, node_id);
        }

        if let Some(rel) = sevenz_volume::parse_sevenz_id(node_id) {
            return sevenz_volume::file_stat(image, &rel, node_id);
        }

        if let Some(id) = ad1_volume::parse_ad1_id(node_id) {
            return ad1_volume::file_stat(image, id, node_id);
        }

        if let Some((offset, len)) = parse_virtual_unalloc_id(node_id) {
            return Ok(crate::filesystem::unalloc::virtual_unalloc_stat(
                image, node_id, offset, len,
            ));
        }

        if let Some((source, path, stream)) = parse_refs_snap_id(node_id) {
            return open_refs_source(image, source)?.named_stream_stat(&path, &stream, node_id);
        }

        if let Some((source, record, stream)) = parse_ntfs_ads_id(node_id) {
            return open_ntfs_for_source(image, source)?
                .named_stream_stat(record, &stream, node_id);
        }

        if let Some(stat) = crate::filesystem::deleted::stat(image, node_id) {
            return Ok(stat);
        }

        if let Some(volume) = self.volume(node_id)? {
            return volume.file_stat(node_id, node_id);
        }

        match crate::filesystem::fs_dispatch::dispatch(image, node_id)? {
            Some(volume) => volume.file_stat(node_id, node_id),
            None => anyhow::bail!("unsupported filesystem node id: {node_id}"),
        }
    }

    /// Stream `node_id`'s logical bytes in `chunk_size` slabs, identical to the
    /// free [`stream_file_chunks`] but served from this walker's cached volume
    /// handles. For the cached filesystem kinds the volume is opened/parsed once
    /// for the whole walk instead of per file — turning an O(files) pile of
    /// volume reopens into O(volumes). Kinds without a cache (FAT, ZFS,
    /// HFS+, Docker, folder) fall back to the per-call open path.
    pub fn stream_file<F>(
        &mut self,
        node_id: &str,
        chunk_size: usize,
        mut visitor: F,
    ) -> anyhow::Result<u64>
    where
        F: FnMut(&[u8]) -> anyhow::Result<()>,
    {
        let image = self.image.filesystem_session(node_id);
        image.require_source()?;
        // A host-Docker file is an overlay view of a file on the host volume:
        // stream the backing node through this session's cached volume instead
        // of the standalone path, which opens a fresh session (and re-loads the
        // host filesystem) for every file.
        if let Some(id) = host_docker::parse_host_docker_id(node_id) {
            let source_id = host_docker::source_file_id(image, &id)?;
            return self.stream_file(&source_id, chunk_size, visitor);
        }
        if let Some(volume) = self.volume(node_id)? {
            return volume.stream_file(node_id, chunk_size, &mut visitor);
        }
        // FAT / ZFS / HFS+ / Docker / folder: no per-walk cache, open
        // per call exactly like the standalone path.
        stream_file_chunks(image, node_id, chunk_size, visitor)
    }

    /// Session-cached variant of [`crate::filesystem::tree::fs_tree_read::read_file_range`]:
    /// the volume behind `node_id` is opened once per volume-root instead of
    /// once per call. Seekable volumes (`Volume::read_range`) read the range
    /// directly; the others stream the prefix with bounded memory and abort
    /// once the range is complete.
    pub fn read_file_range(
        &mut self,
        node_id: &str,
        offset: u64,
        length: usize,
    ) -> anyhow::Result<Vec<u8>> {
        let image = self.image.filesystem_session(node_id);
        image.require_source()?;
        #[derive(Debug)]
        struct RangeComplete;

        impl fmt::Display for RangeComplete {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("requested file range collected")
            }
        }

        impl Error for RangeComplete {}

        let length_u64 =
            u64::try_from(length).map_err(|_| anyhow::anyhow!("range length overflow"))?;
        let end = offset.checked_add(length_u64).ok_or_else(|| {
            anyhow::anyhow!("file range overflow: offset={offset}, length={length}")
        })?;
        if length == 0 {
            return Ok(Vec::new());
        }
        if let Some(id) = host_docker::parse_host_docker_id(node_id) {
            let source_id = host_docker::source_file_id(image, &id)?;
            return self.read_file_range(&source_id, offset, length);
        }
        // Seekable filesystems and contiguous virtual ranges answer directly;
        // the rest fall through to the sequential stream below.
        if let Some(bytes) = self.read_range_direct(node_id, offset, length)? {
            return Ok(bytes);
        }

        let mut output = Vec::with_capacity(length);
        let mut stream_offset = 0u64;
        let result = self.stream_file(node_id, 1024 * 1024, |chunk| {
            let chunk_length = u64::try_from(chunk.len())
                .map_err(|_| anyhow::anyhow!("stream chunk length overflow"))?;
            let chunk_end = stream_offset
                .checked_add(chunk_length)
                .ok_or_else(|| anyhow::anyhow!("stream offset overflow"))?;

            if chunk_end > offset && stream_offset < end {
                let start_in_chunk = offset.saturating_sub(stream_offset);
                let end_in_chunk = end.min(chunk_end) - stream_offset;
                let start_in_chunk = usize::try_from(start_in_chunk)
                    .map_err(|_| anyhow::anyhow!("range start conversion overflow"))?;
                let end_in_chunk = usize::try_from(end_in_chunk)
                    .map_err(|_| anyhow::anyhow!("range end conversion overflow"))?;
                output.extend_from_slice(&chunk[start_in_chunk..end_in_chunk]);
            }

            stream_offset = chunk_end;
            if stream_offset >= end {
                return Err(RangeComplete.into());
            }
            Ok(())
        });

        match result {
            Ok(_) => Ok(output),
            Err(err) if err.downcast_ref::<RangeComplete>().is_some() => Ok(output),
            Err(err) => Err(err),
        }
    }
}
