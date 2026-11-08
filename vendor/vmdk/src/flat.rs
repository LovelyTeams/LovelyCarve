//! Multi-extent flat VMDK reader: concatenates one or more raw extent files
//! into a single `Read + Seek` virtual sector stream.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::descriptor::ExtentEntry;

pub(crate) struct MultiExtentReader {
    extents: Vec<FlatExtent>,
    pos: u64,
    total_bytes: u64,
}

struct FlatExtent {
    /// First virtual byte this extent covers (inclusive).
    byte_start: u64,
    /// First virtual byte NOT covered by this extent.
    byte_end: u64,
    /// Byte offset in the extent file where this extent's data begins.
    file_offset: u64,
    /// `None` for a ZERO extent (no backing file — reads as zeros).
    file: Option<BufReader<File>>,
}

impl MultiExtentReader {
    pub(crate) fn open(base_dir: &Path, extents: &[ExtentEntry]) -> io::Result<Self> {
        let mut flat = Vec::with_capacity(extents.len());
        let mut virt = 0u64;
        for ext in extents {
            let size_bytes = ext.size_sectors.checked_mul(512).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "extent size overflow")
            })?;
            let end = virt.checked_add(size_bytes).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "extent capacity overflow")
            })?;
            ext.file_byte_offset
                .checked_add(size_bytes)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "extent file offset overflow")
                })?;
            // ZERO extents (and NOACCESS holes) have no backing file — they read as zeros.
            let file = if ext.is_zero {
                None
            } else {
                let path = crate::descriptor::resolve_extent_path(base_dir, ext.filename.as_ref())?;
                Some(BufReader::new(File::open(&path).map_err(|e| {
                    io::Error::new(e.kind(), format!("{}: {e}", path.display()))
                })?))
            };
            flat.push(FlatExtent {
                byte_start: virt,
                byte_end: end,
                file_offset: ext.file_byte_offset,
                file,
            });
            virt = end;
        }
        Ok(MultiExtentReader {
            extents: flat,
            pos: 0,
            total_bytes: virt,
        })
    }
}

impl Read for MultiExtentReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.total_bytes || buf.is_empty() {
            return Ok(0);
        }
        let ext = self
            .extents
            .iter_mut()
            .find(|e| e.byte_start <= self.pos && self.pos < e.byte_end)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "offset beyond extents"))?;
        let offset_in_extent = self.pos - ext.byte_start;
        let remaining_in_extent = (ext.byte_end - self.pos) as usize;
        let remaining_total = (self.total_bytes - self.pos) as usize;
        let to_read = buf.len().min(remaining_in_extent).min(remaining_total);
        let n = if let Some(file) = &mut ext.file {
            file.seek(SeekFrom::Start(ext.file_offset + offset_in_extent))?;
            file.read(&mut buf[..to_read])?
        } else {
            // ZERO extent: emit zeros without touching disk.
            buf[..to_read].fill(0);
            to_read
        };
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for MultiExtentReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.pos as i64 + n,
            SeekFrom::End(n) => self.total_bytes as i64 + n,
        };
        if new_pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::ExtentEntry;
    use std::io::Write as _;

    fn two_sector_reader(dir: &std::path::Path) -> MultiExtentReader {
        std::fs::File::create(dir.join("e.vmdk"))
            .unwrap()
            .write_all(&[7u8; 1024])
            .unwrap();
        let ext = ExtentEntry {
            size_sectors: 2,
            filename: Box::from("e.vmdk"),
            file_byte_offset: 0,
            is_zero: false,
        };
        MultiExtentReader::open(dir, &[ext]).unwrap()
    }

    #[test]
    fn seek_current_end_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = two_sector_reader(dir.path());
        assert_eq!(r.seek(SeekFrom::Start(512)).unwrap(), 512);
        assert_eq!(r.seek(SeekFrom::Current(-256)).unwrap(), 256);
        assert_eq!(r.seek(SeekFrom::End(-100)).unwrap(), 1024 - 100);
        let mut b = [0u8; 4];
        r.read_exact(&mut b).unwrap();
        assert_eq!(b, [7, 7, 7, 7]);
    }

    #[test]
    fn seek_before_start_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = two_sector_reader(dir.path());
        assert!(r.seek(SeekFrom::End(-99999)).is_err());
    }

    #[test]
    fn read_past_end_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = two_sector_reader(dir.path());
        r.seek(SeekFrom::Start(1024)).unwrap();
        let mut b = [0u8; 4];
        assert_eq!(r.read(&mut b).unwrap(), 0);
        // empty buffer also yields 0
        r.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(r.read(&mut []).unwrap(), 0);
    }

    #[test]
    fn missing_extent_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let ext = ExtentEntry {
            size_sectors: 2,
            filename: Box::from("nope.vmdk"),
            file_byte_offset: 0,
            is_zero: false,
        };
        assert!(MultiExtentReader::open(dir.path(), &[ext]).is_err());
    }

    #[test]
    fn zero_extent_reads_zeros() {
        let dir = tempfile::tempdir().unwrap();
        let ext = ExtentEntry {
            size_sectors: 2,
            filename: Box::from(""),
            file_byte_offset: 0,
            is_zero: true,
        };
        let mut r = MultiExtentReader::open(dir.path(), &[ext]).unwrap();
        let mut b = [0xFFu8; 8];
        r.read_exact(&mut b).unwrap();
        assert_eq!(b, [0u8; 8]);
    }
}
