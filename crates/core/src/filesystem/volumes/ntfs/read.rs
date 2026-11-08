//! Filesystem file reads, ranges, streaming and metadata.
use super::*;

impl<'a> NtfsVolume<'a> {
    pub fn read_file(&self, record_number: u64, max_bytes: usize) -> Result<Vec<u8>> {
        self.read_data_stream(record_number, "", max_bytes)
    }

    /// Reads up to `max_bytes` from one named `$DATA` stream of the given
    /// MFT record. Pass `""` to read the unnamed default stream — same path
    /// as [`Self::read_file`]. Named streams (NTFS ADS) work on both file
    /// and directory records, so this method does not enforce the
    /// "is_directory" guard when `stream_name` is non-empty.
    pub fn read_data_stream(
        &self,
        record_number: u64,
        stream_name: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        let mut fs = self.reader();
        let file = self
            .ntfs
            .file(&mut fs, record_number)
            .map_err(|err| anyhow!("NTFS file record {record_number}: {err}"))?;
        if stream_name.is_empty() && file.is_directory() {
            anyhow::bail!("NTFS node {record_number} is not a regular file");
        }
        let data_item = file
            .data(&mut fs, stream_name)
            .ok_or_else(|| missing_stream_error(record_number, stream_name))?
            .map_err(|err| anyhow!("NTFS $DATA attribute: {err}"))?;
        let data_attribute = data_item
            .to_attribute()
            .map_err(|err| anyhow!("NTFS $DATA to_attribute: {err}"))?;
        reject_unreadable_attribute(data_attribute.flags(), record_number)?;
        let total = data_attribute.value_length();
        let target_len = total.min(max_bytes as u64) as usize;
        let mut value = super::logical_stream::LogicalStream::new(
            &data_attribute,
            &file,
            &mut fs,
            self.ntfs.cluster_size(),
            record_number,
        )?;

        let mut out = Vec::with_capacity(target_len);
        let mut buf = vec![0u8; STREAM_CHUNK_FLOOR];
        while out.len() < target_len {
            let want = (target_len - out.len()).min(buf.len());
            let read = value
                .read(&mut fs, &mut buf[..want])
                .map_err(|err| anyhow!("NTFS read: {err}"))?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read]);
        }
        Ok(out)
    }

    pub fn stream_file<F>(&self, record_number: u64, chunk_size: usize, visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        self.stream_data_stream(record_number, "", chunk_size, visitor)
    }

    /// Streams one named `$DATA` stream of the given MFT record in
    /// `chunk_size` slabs, invoking `visitor` for each slab. Pass `""` to
    /// stream the unnamed default stream (same path as [`Self::stream_file`]).
    pub fn stream_data_stream<F>(
        &self,
        record_number: u64,
        stream_name: &str,
        chunk_size: usize,
        mut visitor: F,
    ) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let mut fs = self.reader();
        let file = self
            .ntfs
            .file(&mut fs, record_number)
            .map_err(|err| anyhow!("NTFS file record {record_number}: {err}"))?;
        if stream_name.is_empty() && file.is_directory() {
            anyhow::bail!("NTFS node {record_number} is not a regular file");
        }
        let data_item = file
            .data(&mut fs, stream_name)
            .ok_or_else(|| missing_stream_error(record_number, stream_name))?
            .map_err(|err| anyhow!("NTFS $DATA attribute: {err}"))?;
        let data_attribute = data_item
            .to_attribute()
            .map_err(|err| anyhow!("NTFS $DATA to_attribute: {err}"))?;
        reject_unreadable_attribute(data_attribute.flags(), record_number)?;
        let mut value = super::logical_stream::LogicalStream::new(
            &data_attribute,
            &file,
            &mut fs,
            self.ntfs.cluster_size(),
            record_number,
        )?;

        let mut buf = vec![0u8; chunk_size.max(STREAM_CHUNK_FLOOR)];
        let mut total = 0u64;
        loop {
            let read = value
                .read(&mut fs, &mut buf)
                .map_err(|err| anyhow!("NTFS stream read: {err}"))?;
            if read == 0 {
                break;
            }
            visitor(&buf[..read])?;
            total += read as u64;
        }
        Ok(total)
    }

    /// Reads `length` bytes of the unnamed `$DATA` stream starting at logical
    /// `offset`, seeking through the data runs instead of streaming the
    /// preceding bytes. Returns fewer bytes (possibly none) at end of stream.
    pub fn read_range(&self, record_number: u64, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.read_data_stream_range(record_number, "", offset, length)
    }

    /// Seek directly within a named ADS, including connected and LZNT1 data.
    pub fn read_data_stream_range(
        &self,
        record_number: u64,
        stream_name: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        let mut fs = self.reader();
        let file = self
            .ntfs
            .file(&mut fs, record_number)
            .map_err(|err| anyhow!("NTFS file record {record_number}: {err}"))?;
        if stream_name.is_empty() && file.is_directory() {
            anyhow::bail!("NTFS node {record_number} is not a regular file");
        }
        let data_item = file
            .data(&mut fs, stream_name)
            .ok_or_else(|| missing_stream_error(record_number, stream_name))?
            .map_err(|err| anyhow!("NTFS $DATA attribute: {err}"))?;
        let data_attribute = data_item
            .to_attribute()
            .map_err(|err| anyhow!("NTFS $DATA to_attribute: {err}"))?;
        reject_unreadable_attribute(data_attribute.flags(), record_number)?;
        let mut value = super::logical_stream::LogicalStream::new(
            &data_attribute,
            &file,
            &mut fs,
            self.ntfs.cluster_size(),
            record_number,
        )?;
        let available = value.len().saturating_sub(offset);
        let wanted = usize::try_from(available).map_or(length, |available| available.min(length));
        if wanted == 0 {
            return Ok(Vec::new());
        }
        value
            .seek(&mut fs, SeekFrom::Start(offset))
            .map_err(|err| anyhow!("NTFS stream seek to {offset}: {err}"))?;
        let mut out = vec![0u8; wanted];
        let mut filled = 0usize;
        while filled < wanted {
            let read = value
                .read(&mut fs, &mut out[filled..])
                .map_err(|err| anyhow!("NTFS stream read: {err}"))?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        out.truncate(filled);
        Ok(out)
    }
}
