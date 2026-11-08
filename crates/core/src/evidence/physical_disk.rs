//! Platform-specific helpers for opening raw physical disks (`\\.\PhysicalDriveN`
//! on Windows, `/dev/sdX` on Linux/macOS) and enumerating the disks the host
//! has attached.
//!
//! The disk source itself is mounted into `core::evidence::image::ImageSource::Block`
//! — this module only provides the OS-level primitives (capacity + sector
//! size + enumeration). Reads go through `read_aligned` so callers can issue
//! arbitrary byte ranges even on Windows where physical-drive handles
//! enforce sector-aligned IO.

use std::fs::{File, OpenOptions};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

mod descriptor;
#[cfg(test)]
pub(crate) use descriptor::parse as parse_descriptor;
pub use descriptor::DriveSerial;

/// Read the serial reported by this already-open read-only device handle.
/// Windows uses StorageDeviceProperty; other platforms currently return None.
/// Unsupported queries and malformed or absent serials also return None.
pub fn drive_serial(file: &File) -> Option<DriveSerial> {
    #[cfg(target_os = "windows")]
    {
        windows_device_descriptor(file)?.serial
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _file = file;
        None
    }
}

/// Public description of one attached physical disk, surfaced to the
/// frontend by `commands::evidence::image::physical_disks_list`.
#[derive(Debug, Clone, Serialize)]
pub struct PhysicalDisk {
    /// OS-native path that can be passed to `image_load`.
    pub path: String,
    /// Short label suitable for display (`PhysicalDrive0`, `sda`, `disk2`).
    pub display: String,
    pub size_bytes: u64,
    pub sector_size: u32,
    pub model: Option<String>,
    pub is_removable: bool,
}

/// Returns true when `path` looks like a raw block device path that should
/// be opened via `open_block_file` instead of mmap'd. Used by
/// `ImageSource::open` to short-circuit before the regular file path probe.
pub fn is_block_device_path(path: &Path) -> bool {
    let display = path.to_string_lossy();
    // Windows native physical-drive paths.
    if display.starts_with("\\\\.\\PhysicalDrive") || display.starts_with("\\\\?\\PhysicalDrive") {
        return true;
    }
    // Unix block devices commonly live under /dev. We deliberately only
    // accept the well-known prefixes (sd*, nvme*, vd*, hd*, mmcblk*, disk*)
    // so a stray /dev/null doesn't get misdetected.
    let bytes = display.as_bytes();
    if bytes.starts_with(b"/dev/") {
        let tail = &display[5..];
        return tail.starts_with("sd")
            || tail.starts_with("nvme")
            || tail.starts_with("vd")
            || tail.starts_with("hd")
            || tail.starts_with("mmcblk")
            || tail.starts_with("disk");
    }
    false
}

/// Opens `path` as a read-only handle suitable for positioned reads against
/// a raw block device. On Windows this requires Administrator; on Unix the
/// user must be in the `disk` group (or root). The returned tuple is
/// `(file, size_bytes, sector_size)`.
pub fn open_block_device(path: &Path) -> Result<(File, u64, u32)> {
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("failed to open block device: {}", path.display()))?;
    let (size_bytes, sector_size) = query_geometry(&file, path)?;
    if size_bytes == 0 {
        return Err(anyhow!(
            "block device reports zero capacity: {}",
            path.display()
        ));
    }
    Ok((file, size_bytes, sector_size))
}

/// Issues a positioned read against an open block-device handle, expanding
/// the request to sector boundaries on platforms (Windows) where raw
/// device IO must be aligned. The returned `Vec<u8>` is sliced back to the
/// caller's exact requested range, so callers don't see padding bytes.
pub fn read_aligned(
    file: &File,
    sector_size: u32,
    size_bytes: u64,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>> {
    if size_bytes == 0 || len == 0 {
        return Ok(Vec::new());
    }
    let start = offset.min(size_bytes);
    let available = size_bytes.saturating_sub(start).min(len as u64);
    if available == 0 {
        return Ok(Vec::new());
    }
    let sector = sector_size.max(1) as u64;
    let aligned_start = start - (start % sector);
    let head_skip = (start - aligned_start) as usize;
    let logical_end = start + available;
    let aligned_end = logical_end
        .div_ceil(sector)
        .checked_mul(sector)
        .ok_or_else(|| anyhow!("aligned end overflow"))?
        .min(size_bytes.div_ceil(sector) * sector);
    let aligned_len = (aligned_end - aligned_start) as usize;
    let mut buf = vec![0u8; aligned_len];
    read_exact_at(file, &mut buf, aligned_start)?;
    let want = available as usize;
    let slice_end = (head_skip + want).min(buf.len());
    buf.truncate(slice_end);
    if head_skip > 0 {
        buf.drain(..head_skip);
    }
    Ok(buf)
}

/// Enumerates the physical disks visible to the host. Errors on individual
/// disks are swallowed — the caller gets a best-effort list rather than a
/// hard failure when one drive misbehaves.
pub fn enumerate() -> Vec<PhysicalDisk> {
    let mut out = enumerate_platform();
    out.sort_by(|a, b| a.display.cmp(&b.display));
    out
}

// ---------- platform-specific implementations ----------

#[cfg(target_os = "windows")]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> Result<()> {
    use std::os::windows::fs::FileExt;
    let mut filled = 0;
    while filled < buf.len() {
        let read = file
            .seek_read(&mut buf[filled..], offset + filled as u64)
            .with_context(|| format!("seek_read at {offset}+{filled}"))?;
        if read == 0 {
            return Err(anyhow!(
                "unexpected EOF reading {len} bytes at {offset}+{filled}",
                len = buf.len()
            ));
        }
        filled += read;
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
        .with_context(|| format!("read_exact_at {offset}"))
}

#[cfg(target_os = "windows")]
fn query_geometry(file: &File, path: &Path) -> Result<(u64, u32)> {
    use std::mem::{size_of, MaybeUninit};
    use std::os::windows::io::AsRawHandle;
    use winapi::shared::minwindef::DWORD;
    use winapi::um::ioapiset::DeviceIoControl;
    use winapi::um::winioctl::{
        DISK_GEOMETRY_EX, GET_LENGTH_INFORMATION, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
        IOCTL_DISK_GET_LENGTH_INFO,
    };

    let handle = file.as_raw_handle();

    let mut length: GET_LENGTH_INFORMATION = unsafe { std::mem::zeroed() };
    let mut returned: DWORD = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle as _,
            IOCTL_DISK_GET_LENGTH_INFO,
            std::ptr::null_mut(),
            0,
            (&mut length as *mut GET_LENGTH_INFORMATION) as *mut _,
            size_of::<GET_LENGTH_INFORMATION>() as DWORD,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(anyhow!(
            "IOCTL_DISK_GET_LENGTH_INFO failed for {}",
            path.display()
        ));
    }
    let size_bytes = unsafe { *length.Length.QuadPart() } as u64;

    let mut geom = MaybeUninit::<DISK_GEOMETRY_EX>::zeroed();
    let mut returned2: DWORD = 0;
    let ok2 = unsafe {
        DeviceIoControl(
            handle as _,
            IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
            std::ptr::null_mut(),
            0,
            geom.as_mut_ptr() as *mut _,
            size_of::<DISK_GEOMETRY_EX>() as DWORD,
            &mut returned2,
            std::ptr::null_mut(),
        )
    };
    let sector_size = if ok2 != 0 {
        let geom = unsafe { geom.assume_init() };
        geom.Geometry.BytesPerSector
    } else {
        512
    };
    Ok((size_bytes, sector_size))
}

#[cfg(not(target_os = "windows"))]
fn query_geometry(file: &File, path: &Path) -> Result<(u64, u32)> {
    use std::io::{Seek, SeekFrom};
    // lseek(SEEK_END) works for both regular files and block devices on
    // Linux/macOS, so it's the most portable way to size a /dev/sdX handle
    // without pulling in libc ioctls.
    let mut file = file
        .try_clone()
        .with_context(|| format!("clone handle for {}", path.display()))?;
    let size = file
        .seek(SeekFrom::End(0))
        .with_context(|| format!("seek_end for {}", path.display()))?;
    Ok((size, 512))
}

#[cfg(target_os = "windows")]
fn enumerate_platform() -> Vec<PhysicalDisk> {
    let mut out = Vec::new();
    for n in 0..32u32 {
        let path = format!("\\\\.\\PhysicalDrive{n}");
        let p = Path::new(&path);
        let Ok((file, size_bytes, sector_size)) = open_block_device(p) else {
            continue;
        };
        let descriptor = windows_device_descriptor(&file);
        let is_removable = descriptor.as_ref().is_some_and(|info| info.is_removable);
        let model = descriptor.and_then(|info| info.model);
        out.push(PhysicalDisk {
            path: path.clone(),
            display: format!("PhysicalDrive{n}"),
            size_bytes,
            sector_size,
            model,
            is_removable,
        });
    }
    out
}

#[cfg(target_os = "windows")]
fn windows_device_descriptor(file: &File) -> Option<descriptor::DeviceDescriptor> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use winapi::shared::minwindef::DWORD;
    use winapi::um::ioapiset::DeviceIoControl;
    use winapi::um::winioctl::IOCTL_STORAGE_QUERY_PROPERTY;

    #[repr(C)]
    struct StoragePropertyQuery {
        property_id: DWORD,
        query_type: DWORD,
        additional_parameters: [u8; 1],
    }

    let query = StoragePropertyQuery {
        property_id: 0, // StorageDeviceProperty
        query_type: 0,  // PropertyStandardQuery
        additional_parameters: [0],
    };
    let mut buf = vec![0u8; 1024];
    let mut returned: DWORD = 0;
    // Retry once when the returned header advertises a larger descriptor.
    // Cap allocation even when a driver reports an unreasonable size.
    for _attempt in 0..2 {
        let ok = unsafe {
            DeviceIoControl(
                file.as_raw_handle() as _,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &query as *const _ as *mut _,
                size_of::<StoragePropertyQuery>() as DWORD,
                buf.as_mut_ptr() as *mut _,
                buf.len() as DWORD,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || returned < 8 || returned as usize > buf.len() {
            return None;
        }
        let size = u32::from_le_bytes(buf[4..8].try_into().ok()?) as usize;
        if size > buf.len() {
            if size > 64 * 1024 {
                return None;
            }
            buf.resize(size, 0);
            continue;
        }
        return descriptor::parse(&buf[..returned as usize]);
    }
    None
}

#[cfg(target_os = "linux")]
fn enumerate_platform() -> Vec<PhysicalDisk> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/block") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("loop")
            || name.starts_with("ram")
            || name.starts_with("dm-")
            || name.starts_with("zram")
            || name.starts_with("md")
            || name.starts_with("sr")
        {
            continue;
        }
        let sys = entry.path();
        let sectors: u64 = std::fs::read_to_string(sys.join("size"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        if sectors == 0 {
            continue;
        }
        let size_bytes = sectors * 512;
        let model = std::fs::read_to_string(sys.join("device/model"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let is_removable = std::fs::read_to_string(sys.join("removable"))
            .ok()
            .map(|s| s.trim() == "1")
            .unwrap_or(false);
        let dev_path = format!("/dev/{name}");
        out.push(PhysicalDisk {
            path: dev_path,
            display: name,
            size_bytes,
            sector_size: 512,
            model,
            is_removable,
        });
    }
    out
}

#[cfg(target_os = "macos")]
fn enumerate_platform() -> Vec<PhysicalDisk> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Whole disks only: "disk0", "disk1" — skip slices like "disk0s1".
        if !name.starts_with("disk") {
            continue;
        }
        let tail = &name[4..];
        if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let path = format!("/dev/{name}");
        let Ok((_file, size_bytes, sector_size)) = open_block_device(Path::new(&path)) else {
            continue;
        };
        out.push(PhysicalDisk {
            path,
            display: name,
            size_bytes,
            sector_size,
            model: None,
            is_removable: false,
        });
    }
    out
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn enumerate_platform() -> Vec<PhysicalDisk> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn block_device_path_detection() {
        assert!(is_block_device_path(Path::new("\\\\.\\PhysicalDrive0")));
        assert!(is_block_device_path(Path::new("\\\\.\\PhysicalDrive15")));
        assert!(is_block_device_path(Path::new("/dev/sda")));
        assert!(is_block_device_path(Path::new("/dev/sda1")));
        assert!(is_block_device_path(Path::new("/dev/nvme0n1")));
        assert!(is_block_device_path(Path::new("/dev/disk2")));

        assert!(!is_block_device_path(Path::new("/dev/null")));
        assert!(!is_block_device_path(Path::new("C:/images/case.dd")));
        assert!(!is_block_device_path(Path::new("/tmp/image.raw")));
    }

    /// Build a tempfile filled with a deterministic byte pattern and verify
    /// `read_aligned` returns exactly the requested logical range for a
    /// representative mix of unaligned offsets / lengths / sector sizes.
    ///
    /// This is the exact path physical-disk reads go through — getting
    /// alignment math wrong here would manifest as wrong bytes returned to
    /// NTFS / Ext4 readers, which can loop / hang waiting for an "expected"
    /// directory entry. Validating it on a regular file removes that as a
    /// suspect when debugging "fs_children never returns" reports.
    fn pattern_file(size: usize) -> (tempfile::NamedTempFile, Vec<u8>) {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let bytes: Vec<u8> = (0..size).map(|i| (i & 0xff) as u8).collect();
        tmp.as_file_mut().write_all(&bytes).expect("write");
        tmp.as_file_mut().sync_all().ok();
        (tmp, bytes)
    }

    fn run(sector: u32, total: u64, offset: u64, want: usize) {
        let (tmp, bytes) = pattern_file(total as usize);
        let file = std::fs::File::open(tmp.path()).expect("reopen");
        let got = read_aligned(&file, sector, total, offset, want).expect("read_aligned");

        let start = (offset as usize).min(bytes.len());
        let end = ((offset as usize).saturating_add(want)).min(bytes.len());
        let expected = &bytes[start..end];
        assert_eq!(
            got.len(),
            expected.len(),
            "len mismatch: sector={sector} total={total} offset={offset} want={want}"
        );
        assert_eq!(
            got, expected,
            "bytes mismatch: sector={sector} total={total} offset={offset} want={want}"
        );
    }

    #[test]
    fn read_aligned_at_sector_boundary_512() {
        run(512, 8192, 0, 512);
        run(512, 8192, 512, 512);
        run(512, 8192, 1024, 2048);
    }

    #[test]
    fn read_aligned_unaligned_offset_and_length() {
        // 4Kn drive, head-skip + tail-trim: the request straddles three sectors.
        run(4096, 64 * 1024, 100, 200);
        run(4096, 64 * 1024, 4090, 20);
        run(4096, 64 * 1024, 4096 + 1, 4096 + 2);
        // 512n drive, common NTFS-volume probe size.
        run(512, 64 * 1024, 511, 1);
        run(512, 64 * 1024, 511, 2);
        run(512, 64 * 1024, 7, 4096);
    }

    #[test]
    fn read_aligned_clamps_past_end() {
        // Requests beyond EOF must return a short read, not loop.
        run(512, 1024, 0, 4096);
        run(512, 1024, 1023, 16);
        run(4096, 8192, 8000, 4096);
    }

    #[test]
    fn read_aligned_handles_zero_and_oversized_reads() {
        let (tmp, _) = pattern_file(8192);
        let file = std::fs::File::open(tmp.path()).expect("reopen");

        assert!(read_aligned(&file, 512, 8192, 0, 0).unwrap().is_empty());
        assert!(read_aligned(&file, 512, 8192, 8192, 16).unwrap().is_empty());
        assert!(read_aligned(&file, 512, 8192, 9000, 16).unwrap().is_empty());

        // 1 MiB single-shot read against a smaller file: must clamp.
        let got = read_aligned(&file, 4096, 8192, 0, 1024 * 1024).unwrap();
        assert_eq!(got.len(), 8192);
    }
}
