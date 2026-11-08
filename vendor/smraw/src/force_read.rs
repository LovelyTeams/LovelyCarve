//! Deliberately tolerant RAW reads; strict `read_at` and validation stay available.
use crate::reader::OpenMode;
use crate::{discover_contiguous, RawImage};
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;

/// Outcome of a forced read. A short read preserves its prefix and diagnostic;
/// entirely out-of-capacity requests explicitly write zeroes instead of leaving
/// stale caller memory. The host must audit `synthetic` bytes as fabricated.
#[derive(Debug)]
pub struct ForcedRead {
    pub bytes_read: usize,
    pub synthetic: bool,
    pub capacity_warning: bool,
    pub short_read: Option<io::Error>,
}

impl RawImage {
    pub fn open_forced(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_paths(discover_contiguous(path)?.paths, OpenMode::Force)
    }

    /// Read the available prefix, retrying one transient Windows device/network
    /// failure after closing cached handles. Source stability is intentionally
    /// checked separately by evidentiary hash/export workflows.
    pub fn read_at_forced(&self, offset: u64, destination: &mut [u8]) -> io::Result<ForcedRead> {
        self.read_forced_with(offset, destination, positioned_read)
    }

    /// Per-image latch for the first nonempty request exceeding capacity.
    pub fn take_capacity_warning(&self, offset: u64, length: usize) -> bool {
        length > 0
            && length as u64 > self.media_size().saturating_sub(offset)
            && !self.capacity_warned.swap(true, Ordering::Relaxed)
    }

    pub(crate) fn read_forced_with(
        &self,
        offset: u64,
        destination: &mut [u8],
        mut read: impl FnMut(&File, u64, &mut [u8]) -> io::Result<usize>,
    ) -> io::Result<ForcedRead> {
        self.abort.check()?;
        let capacity_warning = self.take_capacity_warning(offset, destination.len());
        let mut result = ForcedRead {
            bytes_read: 0,
            synthetic: false,
            capacity_warning,
            short_read: None,
        };
        if destination.is_empty() {
            return Ok(result);
        }
        if offset >= self.media_size() {
            // Bound the representable span before the host records its provenance.
            offset
                .checked_add(destination.len() as u64)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "RAW read range overflow")
                })?;
            destination.fill(0);
            result.bytes_read = destination.len();
            result.synthetic = true;
            return Ok(result);
        }
        let available = self
            .media_size()
            .saturating_sub(offset)
            .min(destination.len() as u64) as usize;
        let mut retried = false;
        let mut index = self
            .segments
            .partition_point(|segment| segment.offset <= offset)
            .saturating_sub(1);
        while result.bytes_read < available {
            self.abort.check()?;
            let segment = self
                .segments
                .get(index)
                .ok_or_else(|| io::Error::other("RAW segment mapping incomplete"))?;
            if segment.len == 0 {
                index += 1;
                continue;
            }
            let local = offset + result.bytes_read as u64 - segment.offset;
            let length = (segment.len - local).min((available - result.bytes_read) as u64) as usize;
            let file = self.open_handle(index, OpenMode::Force)?;
            // Bound individual I/O and cancellation latency, including large segments.
            let length = length.min(1024 * 1024);
            let start = result.bytes_read;
            let outcome = read(&file, local, &mut destination[start..start + length]);
            match outcome {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if !retried && transient(&error) => {
                    drop(file);
                    self.reset_handles()?;
                    retried = true;
                    continue;
                }
                Err(error) => {
                    result.short_read = Some(error);
                    return Ok(result);
                }
                Ok(count) if count <= length => {
                    result.bytes_read += count;
                    if count < length {
                        result.short_read = Some(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            format!(
                                "short RAW read in {} at {local}: {count}/{length} bytes",
                                segment.path.display()
                            ),
                        ));
                        return Ok(result);
                    }
                }
                Ok(_) => {
                    return Err(io::Error::other(
                        "RAW reader returned an invalid byte count",
                    ))
                }
            }
            if local + length as u64 == segment.len {
                index += 1;
            }
        }
        Ok(result)
    }
}

fn transient(error: &io::Error) -> bool {
    cfg!(windows) && matches!(error.raw_os_error(), Some(21 | 1006 | 64))
}

fn positioned_read(file: &File, offset: u64, destination: &mut [u8]) -> io::Result<usize> {
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(file, destination, offset)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_at(file, destination, offset)
    }
}
