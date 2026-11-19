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
use std::path::{Path, PathBuf};

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
        is_differencing: bool,
        parent: Option<Box<VhdReader>>,
    },
}

const MAX_VHD_CHAIN_DEPTH: usize = 32;

impl VhdReader {
    /// Open a VHD disk image (Fixed, Dynamic, or Differencing chain).
    pub fn open(path: &Path) -> Result<Self, VhdError> {
        Self::open_chain(path, 0)
    }

    fn open_chain(path: &Path, depth: usize) -> Result<Self, VhdError> {
        if depth >= MAX_VHD_CHAIN_DEPTH {
            return Err(VhdError::DifferencingNotSupported);
        }
        Self::open_reader_internal(
            Box::new(std::fs::File::open(path)?),
            path.parent(),
            depth,
            None,
        )
    }

    /// Open a VHD image from any seekable byte source without resolving external parent paths.
    pub fn open_reader(backing: Box<dyn ReadSeekSend>) -> Result<Self, VhdError> {
        Self::open_reader_internal(backing, None, 0, None)
    }

    /// Open a Differencing (or Dynamic) VHD image over an arbitrary byte source with an explicit parent reader.
    pub fn open_reader_with_parent(
        backing: Box<dyn ReadSeekSend>,
        parent: VhdReader,
    ) -> Result<Self, VhdError> {
        Self::open_reader_internal(backing, None, 0, Some(Box::new(parent)))
    }

    fn open_reader_internal(
        mut backing: Box<dyn ReadSeekSend>,
        dir: Option<&Path>,
        depth: usize,
        explicit_parent: Option<Box<VhdReader>>,
    ) -> Result<Self, VhdError> {
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
            footer::DiskType::Dynamic | footer::DiskType::Differencing => {
                let is_differencing = footer.disk_type == footer::DiskType::Differencing;
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
                let parent = if is_differencing {
                    if let Some(p) = explicit_parent {
                        Some(p)
                    } else if let Some(base_dir) = dir {
                        let candidates = dynamic::parse_parent_locator_candidates(
                            &header_bytes,
                            &mut backing,
                            footer_offset,
                        );
                        let parent_path = resolve_vhd_parent_path(base_dir, &candidates)
                            .ok_or(VhdError::DifferencingNotSupported)?;
                        Some(Box::new(Self::open_chain(&parent_path, depth + 1)?))
                    } else {
                        return Err(VhdError::DifferencingNotSupported);
                    }
                } else {
                    None
                };
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
                        is_differencing,
                        parent,
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

    /// Disk type (Fixed, Dynamic, or Differencing).
    pub fn disk_type(&self) -> DiskType {
        match &self.inner {
            VhdInner::Fixed { .. } => DiskType::Fixed,
            VhdInner::Dynamic {
                is_differencing: true,
                ..
            } => DiskType::Differencing,
            VhdInner::Dynamic {
                is_differencing: false,
                ..
            } => DiskType::Dynamic,
        }
    }
}

fn resolve_vhd_parent_path(dir: &Path, candidates: &[String]) -> Option<PathBuf> {
    for raw in candidates {
        let normalized = raw.replace('\\', "/");
        let rel = Path::new(&normalized);
        if !rel.is_absolute() {
            let joined = dir.join(rel);
            if joined.is_file() {
                return Some(joined);
            }
        }
        if let Some(fname) = rel.file_name() {
            let same_dir = dir.join(fname);
            if same_dir.is_file() {
                return Some(same_dir);
            }
        }
    }
    None
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
                parent,
                ..
            } => {
                let block_size_u64 = u64::from(*block_size);
                let block_end = ((self.pos / block_size_u64) + 1) * block_size_u64;
                let mut chunk = to_read.min((block_end - self.pos) as usize);

                if let Some(parent_reader) = parent.as_mut() {
                    if let Some((payload_off, bmp_off, bit_mask, sector_in_block, sectors_per_block)) =
                        bat.differencing_sector_location(self.pos)
                            .map_err(|e| std::io::Error::other(e.to_string()))?
                    {
                        let mut b = [0u8; 1];
                        file.seek(SeekFrom::Start(bmp_off))?;
                        file.read_exact(&mut b)?;
                        let owned_by_child = (b[0] & bit_mask) != 0;

                        // Coalesce contiguous same-owner 512-byte sectors inside the block.
                        let mut run_sectors = 1u64;
                        let first_bit = (sector_in_block % 8) + 1;
                        let mut cur_byte = b[0];
                        let mut byte_off = bmp_off;
                        for s in (sector_in_block + 1)..sectors_per_block {
                            let bit_pos = s % 8;
                            if bit_pos == 0 && s != sector_in_block + first_bit {
                                byte_off += 1;
                                file.seek(SeekFrom::Start(byte_off))?;
                                file.read_exact(&mut b)?;
                                cur_byte = b[0];
                            } else if bit_pos == 0 {
                                byte_off += 1;
                                file.seek(SeekFrom::Start(byte_off))?;
                                file.read_exact(&mut b)?;
                                cur_byte = b[0];
                            }
                            if ((cur_byte & (0x80 >> bit_pos)) != 0) != owned_by_child {
                                break;
                            }
                            run_sectors += 1;
                        }
                        let sector_run_end =
                            ((self.pos / 512) + run_sectors) * 512;
                        chunk = chunk.min((sector_run_end - self.pos) as usize);

                        if owned_by_child {
                            file.seek(SeekFrom::Start(payload_off))?;
                            file.read_exact(&mut buf[..chunk])?;
                        } else {
                            parent_reader.seek(SeekFrom::Start(self.pos))?;
                            parent_reader.read_exact(&mut buf[..chunk])?;
                        }
                        self.pos += chunk as u64;
                        Ok(chunk)
                    } else {
                        parent_reader.seek(SeekFrom::Start(self.pos))?;
                        let n = parent_reader.read(&mut buf[..chunk])?;
                        if n == 0 && chunk > 0 {
                            buf[..chunk].fill(0);
                            self.pos += chunk as u64;
                            Ok(chunk)
                        } else {
                            self.pos += n as u64;
                            Ok(n)
                        }
                    }
                } else if let Some(file_off) = bat
                    .file_offset_for_byte(self.pos)
                    .map_err(|e| std::io::Error::other(e.to_string()))?
                {
                    file.seek(SeekFrom::Start(file_off))?;
                    file.read_exact(&mut buf[..chunk])?;
                    self.pos += chunk as u64;
                    Ok(chunk)
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

    #[test]
    fn differencing_vhd_resolves_parent_via_locator_and_sector_bitmap() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();

        // Fixed parent (2 blocks * 512B = 1024B): sector 0 = 0x11, sector 1 = 0x22.
        let mut base = vec![0x11u8; 512];
        base.extend(vec![0x22u8; 512]);
        let base_vhd = fixed_vhd_bytes(&base);
        std::fs::write(dir.path().join("base.vhd"), base_vhd).unwrap();

        // Child differencing VHD: block_size=512, 2 BAT entries:
        // block 0 allocated with bitmap bit 0 = 1 (child owns sector 0 -> 0xCC)
        // block 1 allocated with bitmap bit 0 = 0 (inherits parent sector 1 -> 0x22)
        // Locator entry "W2ru" pointing to "base.vhd".
        let rel_u16: Vec<u8> = "base.vhd".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut child = vec![0u8; 512 * 10];
        // Footer helpers (type=4 Differencing, data_offset=512, current_size=1024).
        let mut f = footer::test_fixed_footer(1024);
        f[16..24].copy_from_slice(&512u64.to_be_bytes());
        f[60..64].copy_from_slice(&4u32.to_be_bytes());
        let mut sum: u32 = 0;
        for (i, &b) in f.iter().enumerate() {
            if !(64..68).contains(&i) {
                sum = sum.wrapping_add(u32::from(b));
            }
        }
        f[64..68].copy_from_slice(&(!sum).to_be_bytes());

        child[0..512].copy_from_slice(&f);
        let tail = child.len() - 512;
        child[tail..].copy_from_slice(&f);

        // Dynamic header at offset 512 (bat_offset=1536, max_bat_entries=2, block_size=512).
        child[512..520].copy_from_slice(b"cxsparse");
        child[512 + 16..512 + 24].copy_from_slice(&1536u64.to_be_bytes());
        child[512 + 28..512 + 32].copy_from_slice(&2u32.to_be_bytes());
        child[512 + 32..512 + 36].copy_from_slice(&512u32.to_be_bytes());
        // Parent locator 0 ("W2ru") pointing to offset 2048.
        let loc_off = 512 + 576;
        child[loc_off..loc_off + 4].copy_from_slice(b"W2ru");
        child[loc_off + 8..loc_off + 12].copy_from_slice(&(rel_u16.len() as u32).to_be_bytes());
        child[loc_off + 16..loc_off + 24].copy_from_slice(&2048u64.to_be_bytes());

        // BAT at 1536: block 0 = sector 5 (2560), block 1 = sector 7 (3584).
        child[1536..1540].copy_from_slice(&5u32.to_be_bytes());
        child[1540..1544].copy_from_slice(&7u32.to_be_bytes());
        // Parent locator bytes at 2048.
        child[2048..2048 + rel_u16.len()].copy_from_slice(&rel_u16);
        // Block 0 (sector 5 = 2560): bitmap bit 0 set (0x80), payload = 0xCC.
        child[2560] = 0x80;
        child[3072..3584].fill(0xCC);
        // Block 1 (sector 7 = 3584): bitmap bit 0 clear (0x00), payload = 0xDD (ignored!).
        child[3584] = 0x00;
        child[4096..4608].fill(0xDD);

        let child_path = dir.path().join("child.vhd");
        std::fs::File::create(&child_path)
            .unwrap()
            .write_all(&child)
            .unwrap();

        let mut reader = VhdReader::open(&child_path).expect("differencing child opens");
        assert_eq!(reader.disk_type(), DiskType::Differencing);
        let mut out = vec![0u8; 1024];
        reader.read_exact(&mut out).expect("read full differencing disk");
        assert_eq!(&out[0..512], &[0xCCu8; 512][..]);
        assert_eq!(&out[512..1024], &[0x22u8; 512][..]);
    }
}
