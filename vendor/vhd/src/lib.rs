//! Pure-Rust read-only legacy VHD disk image reader.
//!
//! Implements the MS-VHD specification (Virtual PC / Virtual Server / Hyper-V
//! Generation-1 format). Supports Fixed and Dynamic disk types. Differencing
//! disks are rejected (parent locator resolution is out of scope).
//!
//! # Format overview
//! Every VHD ends with a 512-byte footer (`cookie = "conectix"`).
//! - **Fixed**: the footer immediately follows the raw sector data.
//! - **Dynamic**: footer → dynamic header → Block Allocation Table (BAT)
//!   → data blocks, with a copy of the footer at byte 0.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

mod dynamic;
mod error;
mod read;

#[cfg(feature = "test-helpers")]
pub mod footer;
#[cfg(not(feature = "test-helpers"))]
mod footer;

#[cfg(feature = "vfs")]
pub mod vfs;

pub use error::VhdError;
pub use footer::{DiskType, VhdFooter};

/// A seekable, thread-safe byte source the reader can sit on: a `File`, an
/// in-RAM `Cursor`, or a positioned sub-range of a `.zip`. Lets a caller open a
/// VHD straight out of an archive (no temp-file extraction) via
/// [`VhdReader::open_reader`], while [`VhdReader::open`] keeps the file-path
/// convenience.
pub trait ReadSeekSend: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> ReadSeekSend for T {}

/// Read-only VHD container reader.
///
/// Implements `Read + Seek` over the virtual sector stream.
pub struct VhdReader {
    inner: VhdInner,
    pos: u64,
    virtual_disk_size: u64,
    original_size: u64,
}

enum VhdInner {
    Fixed {
        file: Box<dyn ReadSeekSend>,
    },
    Dynamic {
        file: Box<dyn ReadSeekSend>,
        bat: dynamic::BlockAllocationTable,
        block_size: u32,
    },
}

impl VhdReader {
    /// Open a VHD disk image.
    ///
    /// Returns [`VhdError`] if the file is not a valid VHD, or if it is a
    /// Differencing disk (parent resolution is not supported).
    pub fn open(path: &Path) -> Result<Self, VhdError> {
        Self::open_reader(Box::new(std::fs::File::open(path)?))
    }

    /// Open a VHD image from any seekable byte source (a `Cursor` over inflated
    /// bytes, a positioned sub-range of a `.zip`, …) rather than a file path —
    /// so an image stored inside an archive can be read without extracting it to
    /// a temp file first.
    pub fn open_reader(mut backing: Box<dyn ReadSeekSend>) -> Result<Self, VhdError> {
        let file_len = backing.seek(SeekFrom::End(0))?;
        let footer_offset = file_len.checked_sub(512).ok_or(VhdError::FileTooSmall)?;
        backing.seek(SeekFrom::Start(footer_offset))?;
        let mut footer_bytes = [0; 512];
        backing.read_exact(&mut footer_bytes)?;
        let footer = footer::VhdFooter::parse(&footer_bytes)?;

        let (inner, virtual_disk_size) = match footer.disk_type {
            footer::DiskType::Fixed => {
                if footer.current_size > footer_offset {
                    return Err(VhdError::BlockOutOfBounds);
                }
                (VhdInner::Fixed { file: backing }, footer.current_size)
            }
            footer::DiskType::Dynamic => {
                if footer
                    .data_offset
                    .checked_add(1024)
                    .is_none_or(|end| end > footer_offset)
                {
                    return Err(VhdError::BatOutOfBounds);
                }
                backing.seek(SeekFrom::Start(footer.data_offset))?;
                let mut header_bytes = [0; 1024];
                backing.read_exact(&mut header_bytes)?;
                let mut header = dynamic::DynamicHeader::parse(&header_bytes, 0)?;
                let table_bytes = u64::from(header.max_bat_entries) * 4;
                // Bound allocation independently of the untrusted container length.
                if table_bytes > 64 * 1024 * 1024
                    || header
                        .bat_offset
                        .checked_add(table_bytes)
                        .is_none_or(|end| end > footer_offset)
                    || u64::from(header.max_bat_entries) * u64::from(header.block_size)
                        < footer.current_size
                {
                    return Err(VhdError::BatOutOfBounds);
                }
                backing.seek(SeekFrom::Start(header.bat_offset))?;
                let mut table = vec![0; table_bytes as usize];
                backing.read_exact(&mut table)?;
                header.bat_offset = 0;
                let bat = dynamic::BlockAllocationTable::parse(&table, &header)?;
                bat.validate_payload_bounds(footer_offset)?;
                (
                    VhdInner::Dynamic {
                        file: backing,
                        bat,
                        block_size: header.block_size,
                    },
                    footer.current_size,
                )
            }
        };

        Ok(VhdReader {
            inner,
            pos: 0,
            virtual_disk_size,
            original_size: footer.original_size,
        })
    }

    /// Virtual disk size in bytes — the current, readable capacity
    /// (footer CurrentSize, offset 48). This is what qemu-img reports.
    pub fn virtual_disk_size(&self) -> u64 {
        self.virtual_disk_size
    }

    /// Original (creation-time) size in bytes (footer OriginalSize, offset 40).
    /// Equals `virtual_disk_size` for an un-resized disk and differs after a
    /// resize (a forensic signal). This is the value libvhdi reports as media size.
    pub fn original_size(&self) -> u64 {
        self.original_size
    }

    /// Disk type (Fixed or Dynamic).
    pub fn disk_type(&self) -> DiskType {
        match &self.inner {
            VhdInner::Fixed { .. } => DiskType::Fixed,
            VhdInner::Dynamic { .. } => DiskType::Dynamic,
        }
    }
}

impl Read for VhdReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.virtual_disk_size || buf.is_empty() {
            return Ok(0);
        }
        let remaining = usize::try_from(self.virtual_disk_size - self.pos).unwrap_or(usize::MAX);
        let to_read = buf.len().min(remaining);

        match &mut self.inner {
            VhdInner::Fixed { file } => {
                file.seek(SeekFrom::Start(self.pos))?;
                file.read_exact(&mut buf[..to_read])?;
                let n = to_read;
                self.pos += n as u64;
                Ok(n)
            }
            VhdInner::Dynamic {
                file,
                bat,
                block_size,
            } => {
                let block_size_u64 = u64::from(*block_size);
                let block_end = ((self.pos / block_size_u64) + 1) * block_size_u64;
                let chunk = to_read.min((block_end - self.pos) as usize);

                if let Some(file_off) = bat
                    .file_offset_for_byte(self.pos)
                    .map_err(|e| std::io::Error::other(e.to_string()))?
                {
                    file.seek(SeekFrom::Start(file_off))?;
                    file.read_exact(&mut buf[..chunk])?;
                    let n = chunk;
                    self.pos += n as u64;
                    Ok(n)
                } else {
                    // Sparse block — return zeroes.
                    buf[..chunk].fill(0);
                    self.pos += chunk as u64;
                    Ok(chunk)
                }
            }
        }
    }
}

impl Seek for VhdReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.pos) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.virtual_disk_size) + i128::from(n),
        };
        self.pos = u64::try_from(new_pos).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek outside u64 range")
        })?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal valid Fixed VHD: 512 bytes of sector data + 512-byte footer.
    fn fixed_vhd_bytes(sector_data: &[u8]) -> Vec<u8> {
        let mut buf = sector_data.to_vec();
        buf.extend_from_slice(&footer::test_fixed_footer(sector_data.len() as u64));
        buf
    }

    fn write_tmp(data: &[u8]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(data).unwrap();
        f
    }

    #[test]
    fn seek_current_and_end_and_before_start() {
        let image = fixed_vhd_bytes(&[0u8; 1024]);
        let tmp = write_tmp(&image);
        let mut r = VhdReader::open(tmp.path()).unwrap();
        r.seek(SeekFrom::Start(200)).unwrap();
        assert_eq!(r.seek(SeekFrom::Current(100)).unwrap(), 300); // relative seek
        assert_eq!(
            r.seek(SeekFrom::End(-10)).unwrap(),
            r.virtual_disk_size() - 10
        );
        // Seeking before the start is an error, not a panic.
        assert!(r.seek(SeekFrom::Current(-100_000)).is_err());
    }

    #[test]
    fn open_reader_over_cursor_matches_open_path() {
        use std::io::Cursor;
        let sector: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
        let image = fixed_vhd_bytes(&sector);

        // Oracle: open(path) and read the whole virtual disk.
        let tmp = write_tmp(&image);
        let mut via_path = VhdReader::open(tmp.path()).expect("open path");
        let mut want = Vec::new();
        via_path.read_to_end(&mut want).expect("read path");

        // Under test: open_reader over an in-RAM Cursor of the SAME bytes — the
        // zip-direct backing path.
        let mut via_reader =
            VhdReader::open_reader(Box::new(Cursor::new(image.clone()))).expect("open_reader");
        let mut got = Vec::new();
        via_reader.read_to_end(&mut got).expect("read reader");

        assert_eq!(
            got, want,
            "open_reader must read byte-identically to open(path)"
        );
        assert_eq!(via_reader.virtual_disk_size(), via_path.virtual_disk_size());
    }

    #[test]
    fn open_nonexistent_returns_err() {
        assert!(VhdReader::open(Path::new("/tmp/no_such.vhd")).is_err());
    }

    #[test]
    fn open_empty_file_returns_err() {
        let f = write_tmp(&[]);
        assert!(VhdReader::open(f.path()).is_err());
    }

    #[test]
    fn open_non_vhd_file_returns_err() {
        let f = write_tmp(b"this is not a vhd file at all, no footer here");
        assert!(VhdReader::open(f.path()).is_err());
    }

    #[test]
    fn fixed_vhd_size_matches_footer() {
        let sector = vec![0u8; 512];
        let vhd = fixed_vhd_bytes(&sector);
        let f = write_tmp(&vhd);
        let reader = VhdReader::open(f.path()).expect("open fixed vhd");
        assert_eq!(reader.virtual_disk_size(), 512);
    }

    #[test]
    fn fixed_vhd_disk_type_is_fixed() {
        let sector = vec![0u8; 512];
        let vhd = fixed_vhd_bytes(&sector);
        let f = write_tmp(&vhd);
        let reader = VhdReader::open(f.path()).expect("open fixed vhd");
        assert_eq!(reader.disk_type(), DiskType::Fixed);
    }

    #[test]
    fn fixed_vhd_read_returns_sector_data() {
        let mut sector = vec![0u8; 512];
        sector[42] = 0xDE;
        sector[43] = 0xAD;
        let vhd = fixed_vhd_bytes(&sector);
        let f = write_tmp(&vhd);
        let mut reader = VhdReader::open(f.path()).expect("open");
        let mut buf = vec![0u8; 512];
        reader.read_exact(&mut buf).expect("read");
        assert_eq!(buf[42], 0xDE);
        assert_eq!(buf[43], 0xAD);
    }

    #[test]
    fn seek_and_read_at_offset() {
        let mut sector = vec![0u8; 512];
        sector[100] = 0xBE;
        sector[101] = 0xEF;
        let vhd = fixed_vhd_bytes(&sector);
        let f = write_tmp(&vhd);
        let mut reader = VhdReader::open(f.path()).expect("open");
        reader.seek(SeekFrom::Start(100)).unwrap();
        let mut buf = [0u8; 2];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [0xBE, 0xEF]);
    }

    #[test]
    fn differencing_disk_returns_err() {
        // A footer with disk_type=4 (Differencing) must be rejected.
        let mut footer_bytes = footer::test_fixed_footer(512);
        // Disk type field is at offset 60 in the footer; set to 4.
        footer_bytes[60] = 0;
        footer_bytes[61] = 0;
        footer_bytes[62] = 0;
        footer_bytes[63] = 4;
        let mut vhd = vec![0u8; 512];
        vhd.extend_from_slice(&footer_bytes);
        let f = write_tmp(&vhd);
        assert!(VhdReader::open(f.path()).is_err());
    }

    #[test]
    fn vhd_reader_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<VhdReader>();
    }

    // ── block_size=0 in dynamic header must be rejected, not panic ────────────
    //
    // A crafted dynamic VHD with block_size=0 causes div-by-zero in:
    //   file_offset_for_byte: virtual_byte / block_size
    //   Read::read: self.pos / block_size_u64
    // open() must return Err before reaching those sites.
    #[test]
    fn dynamic_vhd_block_size_zero_rejected() {
        use std::io::Write;

        const BLOCK_SIZE: u64 = 0; // deliberately invalid
        let mut file = vec![0u8; 3072 + BLOCK_SIZE as usize + 512];

        let footer = {
            let mut f = vec![0u8; 512];
            f[0..8].copy_from_slice(b"conectix");
            f[8..12].copy_from_slice(&0x0000_0002u32.to_be_bytes());
            f[12..16].copy_from_slice(&0x0001_0000u32.to_be_bytes());
            f[16..24].copy_from_slice(&512u64.to_be_bytes());
            f[40..48].copy_from_slice(&(512u64).to_be_bytes()); // original_size (offset 40)
            f[48..56].copy_from_slice(&(512u64).to_be_bytes()); // current_size (offset 48)
            f[60..64].copy_from_slice(&3u32.to_be_bytes()); // Dynamic
            let mut s: u32 = 0;
            for (i, &b) in f.iter().enumerate() {
                if !(64..68).contains(&i) {
                    s = s.wrapping_add(u32::from(b));
                }
            }
            f[64..68].copy_from_slice(&(!s).to_be_bytes());
            f
        };

        file[0..512].copy_from_slice(&footer);
        let footer_start = file.len() - 512;
        file[footer_start..].copy_from_slice(&footer);
        file[512..520].copy_from_slice(b"cxsparse");
        file[512 + 16..512 + 24].copy_from_slice(&1536u64.to_be_bytes());
        file[512 + 28..512 + 32].copy_from_slice(&1u32.to_be_bytes());
        file[512 + 32..512 + 36].copy_from_slice(&(BLOCK_SIZE as u32).to_be_bytes()); // 0!

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&file).unwrap();
        assert!(
            VhdReader::open(tmp.path()).is_err(),
            "block_size=0 must be rejected at open() to prevent div-by-zero"
        );
    }

    // ── BITMAP_SECTORS must be computed from block_size, not hardcoded to 1 ────
    //
    // MS-VHD spec §2.3: each dynamic block is preceded by a sector bitmap whose
    // size (in sectors) = ((block_size / (8 * 512) + 511) & ~511) / 512.
    // For the standard 2 MiB block_size this is 1 sector; for 4 MiB it is 2.
    // Hardcoding BITMAP_SECTORS = 1 causes 4 MiB blocks to be mis-read by exactly
    // 512 bytes — returning the second bitmap sector instead of the first data sector.
    #[test]
    fn bitmap_sectors_computed_for_4mib_block_size() {
        use std::io::Write;

        // Build a minimal dynamic VHD with block_size = 4 MiB.
        //
        // File layout (all offsets in bytes):
        //   [0..512)   : footer copy (dynamic, data_offset=512, virtual_size=4MiB)
        //   [512..1536): dynamic header (bat_offset=1536, block_size=4MiB, max_bat_entries=1)
        //   [1536..2048): BAT (entry 0 = sector 4, padded)
        //   [2048..2560): block 0 bitmap sector 1 (0xFF — all sectors present)
        //   [2560..3072): block 0 bitmap sector 2 (0xFF)
        //   [3072..3584): block 0 data sector 0 (0xAB pattern — the "right" answer)
        //   [3584..3072+4MiB): remainder of allocated block
        //   [3072+4MiB..3584+4MiB): real footer (same as copy)

        const BLOCK_SIZE: u64 = 4 * 1024 * 1024;
        let mut file = vec![0u8; 3072 + BLOCK_SIZE as usize + 512];

        // Footer builder (dynamic disk type).
        let footer = {
            let mut f = vec![0u8; 512];
            f[0..8].copy_from_slice(b"conectix");
            f[8..12].copy_from_slice(&0x0000_0002u32.to_be_bytes()); // features
            f[12..16].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // file format version
            f[16..24].copy_from_slice(&512u64.to_be_bytes()); // data_offset → dynamic header
            f[40..48].copy_from_slice(&BLOCK_SIZE.to_be_bytes()); // original_size (offset 40)
            f[48..56].copy_from_slice(&BLOCK_SIZE.to_be_bytes()); // current_size (offset 48)
            f[60..64].copy_from_slice(&3u32.to_be_bytes()); // disk_type = Dynamic
                                                            // One's-complement checksum (bytes 64-67 zeroed during computation).
            let mut s: u32 = 0;
            for (i, &b) in f.iter().enumerate() {
                if !(64..68).contains(&i) {
                    s = s.wrapping_add(u32::from(b));
                }
            }
            f[64..68].copy_from_slice(&(!s).to_be_bytes());
            f
        };

        file[0..512].copy_from_slice(&footer); // footer copy
        let footer_start = file.len() - 512;
        file[footer_start..].copy_from_slice(&footer); // real footer at end

        // Dynamic header (bat_offset=1536, block_size=4MiB, max_bat_entries=1).
        file[512..520].copy_from_slice(b"cxsparse");
        file[512 + 16..512 + 24].copy_from_slice(&1536u64.to_be_bytes()); // bat_offset
        file[512 + 28..512 + 32].copy_from_slice(&1u32.to_be_bytes()); // max_bat_entries
        file[512 + 32..512 + 36].copy_from_slice(&(BLOCK_SIZE as u32).to_be_bytes()); // block_size

        // BAT: entry 0 = sector 4 (byte 2048 = start of block 0).
        file[1536..1540].copy_from_slice(&4u32.to_be_bytes());

        // Block 0 bitmap: 2 sectors × 0xFF (all 512-byte sectors present).
        file[2048..2560].fill(0xFF); // bitmap sector 1
        file[2560..3072].fill(0xFF); // bitmap sector 2

        // Block 0 data: known sentinel. The test asserts this is what we read.
        // With BITMAP_SECTORS=1 (bug), the reader skips only 1 sector and reads
        // bytes [2560..3072) which are 0xFF (bitmap) — mismatch.
        file[3072..3584].fill(0xAB);

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&file).unwrap();

        let mut reader = VhdReader::open(tmp.path()).expect("open synthetic 4MiB-block vhd");
        let mut buf = [0u8; 512];
        reader.seek(SeekFrom::Start(0)).unwrap();
        reader
            .read_exact(&mut buf)
            .expect("read block 0 data sector 0");
        assert_eq!(
            buf, [0xABu8; 512],
            "with 4 MiB block_size, bitmap is 2 sectors (1024 bytes); \
             BITMAP_SECTORS must not be hardcoded to 1"
        );
    }

    // ── Differential test: bytes must match qemu-img convert -O raw output ────
    //
}
