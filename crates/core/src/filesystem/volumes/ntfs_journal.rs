//! Read USN metadata through the existing NTFS/VMDK reader, without loading a
//! multi-gigabyte sparse journal into memory.
use super::{missing_stream_error, reject_unreadable_attribute, NtfsVolume};
use crate::filesystem::volumes::ntfs_usn::{UsnRecord, UsnStreamDecoder};
use anyhow::{anyhow, Result};
use ntfs::NtfsAttributeFlags;
use std::io::SeekFrom;
use tokio_util::sync::CancellationToken;

impl NtfsVolume<'_> {
    /// Finds the MFT record number of `$Extend\$UsnJrnl` by enumerating the
    /// `$Extend` directory index (MFT record 11) rather than assuming a fixed
    /// record number — the change journal's record varies by volume. Returns
    /// `None` if the volume has no change journal.
    fn usn_journal_record(&self) -> Result<Option<u64>> {
        const EXTEND_RECORD: u64 = 11;
        let mut fs = self.reader();
        let extend = self
            .ntfs
            .file(&mut fs, EXTEND_RECORD)
            .map_err(|err| anyhow!("NTFS $Extend record: {err}"))?;
        if !extend.is_directory() {
            return Ok(None);
        }
        let index = extend
            .directory_index(&mut fs)
            .map_err(|err| anyhow!("NTFS $Extend index: {err}"))?;
        let mut iter = index.entries();
        while let Some(entry) = iter.next(&mut fs) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    tracing::debug!("NTFS $Extend entry skipped: {err}");
                    continue;
                }
            };
            let key = match entry.key() {
                Some(Ok(key)) => key,
                _ => continue,
            };
            if key.name().to_string_lossy() == "$UsnJrnl" {
                if let Ok(file) = entry.to_file(&self.ntfs, &mut fs) {
                    return Ok(Some(file.file_record_number()));
                }
            }
        }
        Ok(None)
    }

    /// Visit the populated journal with bounded buffers; cancellation is checked
    /// even while traversing zero-filled regions. No journal is a valid empty result.
    pub fn visit_usn_journal(
        &self,
        cancel: &CancellationToken,
        mut visit: impl FnMut(UsnRecord) -> Result<()>,
    ) -> Result<u64> {
        self.visit_usn_journal_bytes(u64::MAX, cancel, &mut visit)
    }

    /// Compatibility API for the journal panel. The byte budget starts after
    /// the leading sparse runs, rather than being consumed by sparse zeros.
    pub fn read_usn_journal(&self, max_bytes: usize) -> Result<Vec<UsnRecord>> {
        let mut records = Vec::new();
        self.visit_usn_journal_bytes(max_bytes as u64, &CancellationToken::new(), &mut |record| {
            records.push(record);
            Ok(())
        })?;
        Ok(records)
    }

    fn visit_usn_journal_bytes(
        &self,
        max_bytes: u64,
        cancel: &CancellationToken,
        visit: &mut impl FnMut(UsnRecord) -> Result<()>,
    ) -> Result<u64> {
        let Some(record) = self.usn_journal_record()? else {
            return Ok(0);
        };
        let mut reader = self.reader();
        let file = self.ntfs.file(&mut reader, record)?;
        let item = file
            .data(&mut reader, "$J")
            .ok_or_else(|| missing_stream_error(record, "$J"))??;
        let attribute = item.to_attribute()?;
        reject_unreadable_attribute(attribute.flags(), record)?;
        // Runlist offsets are logical; seeking skips holes without reading their
        // multi-GB zero representation. Connected attributes retain the library's
        // normal reader path, so fragmented journals still work.
        let mut start = 0u64;
        if !attribute.flags().contains(NtfsAttributeFlags::COMPRESSED) {
            for extent in attribute.data_extents(&mut reader)? {
                if extent.data_position.value().is_some() {
                    break;
                }
                start = start
                    .checked_add(extent.allocated_size)
                    .ok_or_else(|| anyhow!("NTFS USN sparse prefix overflow"))?;
            }
        }
        let mut value = super::logical_stream::LogicalStream::new(
            &attribute,
            &file,
            &mut reader,
            self.ntfs.cluster_size(),
            record,
        )?;
        value.seek(&mut reader, SeekFrom::Start(start.min(value.len())))?;
        let mut decoder = UsnStreamDecoder::default();
        let mut buffer = vec![0u8; 256 * 1024];
        let mut bytes = 0u64;
        let mut records = 0u64;
        let mut populated = false;
        while !cancel.is_cancelled() && bytes < max_bytes {
            let size = buffer
                .len()
                .min((max_bytes - bytes).min(usize::MAX as u64) as usize);
            let read = value.read(&mut reader, &mut buffer[..size])?;
            if read == 0 {
                break;
            }
            // An attribute-list-backed stream may still start in a sparse run.
            // Empty prefix chunks must not exhaust the journal panel's cap.
            if !populated && buffer[..read].iter().all(|byte| *byte == 0) {
                continue;
            }
            populated = true;
            bytes += read as u64;
            decoder.push(&buffer[..read], |record| {
                if !cancel.is_cancelled() {
                    visit(record)?;
                    records += 1;
                }
                Ok(())
            })?;
        }
        Ok(records)
    }
}
