//! Expose a byte range of an existing file as a single virtual file via Dokan.
//!
//! Use case: ewfmount exposes an E01 as `X:\EWF1` (whole disk). Tools like
//! refsprogs need to read a single ReFS partition directly, but they don't
//! accept an `-o offset` flag. This mount creates `<drive>:\volume.bin` which
//! is a read-only window into `<backing>` starting at `<offset>` for `<length>`
//! bytes — point `refsinfo` / `refsls` at that file.
//!
//! Usage:
//!   cargo run --example probe_dokan_slice -- <backing-file> <offset> <length> <drive-letter>
//!
//! Example (ReFS partition #2 of the Win Server E01):
//!   cargo run --example probe_dokan_slice -- X:\EWF1 227540992 272501833728 Y

#![cfg(target_os = "windows")]

use std::{
    env,
    fs::File,
    os::windows::fs::FileExt,
    sync::Arc,
    time::{Duration, SystemTime},
};

use dokan::{
    CreateFileInfo, DiskSpaceInfo, FileInfo, FileSystemHandler, FileSystemMounter, FillDataResult,
    FindData, MountFlags, MountOptions, OperationInfo, OperationResult, VolumeInfo,
    IO_SECURITY_CONTEXT,
};
use widestring::{U16CStr, U16CString};
use winapi::{
    shared::ntstatus::{STATUS_ACCESS_DENIED, STATUS_INVALID_PARAMETER, STATUS_NO_SUCH_FILE},
    um::winnt::{
        ACCESS_MASK, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY, FILE_CASE_PRESERVED_NAMES,
        FILE_READ_ONLY_VOLUME,
    },
};

const VOLUME_FILE: &str = "volume.bin";

struct SliceFs {
    backing: Arc<File>,
    base: u64,
    length: u64,
}

#[derive(Clone, Copy)]
enum Node {
    Root,
    Volume,
}

impl SliceFs {
    fn resolve(&self, name: &U16CStr) -> OperationResult<Node> {
        let s = name.to_string_lossy();
        let trimmed = s.trim_start_matches('\\');
        if trimmed.is_empty() {
            Ok(Node::Root)
        } else if trimmed.eq_ignore_ascii_case(VOLUME_FILE) {
            Ok(Node::Volume)
        } else {
            Err(STATUS_NO_SUCH_FILE)
        }
    }

    fn file_info(&self, node: Node) -> FileInfo {
        let (attributes, file_size) = match node {
            Node::Root => (FILE_ATTRIBUTE_DIRECTORY, 0),
            Node::Volume => (FILE_ATTRIBUTE_READONLY, self.length),
        };
        FileInfo {
            attributes,
            creation_time: SystemTime::UNIX_EPOCH,
            last_access_time: SystemTime::UNIX_EPOCH,
            last_write_time: SystemTime::UNIX_EPOCH,
            file_size,
            number_of_links: 1,
            file_index: matches!(node, Node::Volume) as u64,
        }
    }
}

impl<'c, 'h: 'c> FileSystemHandler<'c, 'h> for SliceFs {
    type Context = Node;

    fn create_file(
        &'h self,
        file_name: &U16CStr,
        _security: &IO_SECURITY_CONTEXT,
        _desired_access: ACCESS_MASK,
        _file_attributes: u32,
        _share_access: u32,
        _create_disposition: u32,
        _create_options: u32,
        _info: &mut OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<CreateFileInfo<Self::Context>> {
        let node = self.resolve(file_name)?;
        Ok(CreateFileInfo {
            is_dir: matches!(node, Node::Root),
            context: node,
            new_file_created: false,
        })
    }

    fn read_file(
        &'h self,
        _file_name: &U16CStr,
        offset: i64,
        buffer: &mut [u8],
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        if !matches!(context, Node::Volume) {
            return Err(STATUS_ACCESS_DENIED);
        }
        if offset < 0 {
            return Err(STATUS_INVALID_PARAMETER);
        }
        let off = offset as u64;
        if off >= self.length || buffer.is_empty() {
            return Ok(0);
        }
        let take = (self.length - off).min(buffer.len() as u64) as usize;
        let n = self
            .backing
            .seek_read(&mut buffer[..take], self.base + off)
            .map_err(|_| STATUS_INVALID_PARAMETER)?;
        Ok(n as u32)
    }

    fn get_file_information(
        &'h self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<FileInfo> {
        Ok(self.file_info(*context))
    }

    fn find_files(
        &'h self,
        _file_name: &U16CStr,
        mut fill: impl FnMut(&FindData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        if !matches!(context, Node::Root) {
            return Err(STATUS_INVALID_PARAMETER);
        }
        let info = self.file_info(Node::Volume);
        let _ = fill(&FindData {
            attributes: info.attributes,
            creation_time: info.creation_time,
            last_access_time: info.last_access_time,
            last_write_time: info.last_write_time,
            file_size: info.file_size,
            file_name: U16CString::from_str(VOLUME_FILE).unwrap(),
        });
        Ok(())
    }

    fn get_disk_free_space(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<DiskSpaceInfo> {
        Ok(DiskSpaceInfo {
            byte_count: self.length.max(1),
            free_byte_count: 0,
            available_byte_count: 0,
        })
    }

    fn get_volume_information(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<VolumeInfo> {
        Ok(VolumeInfo {
            name: U16CString::from_str("slice").unwrap(),
            serial_number: 0xdead_beef,
            max_component_length: 255,
            fs_flags: FILE_CASE_PRESERVED_NAMES | FILE_READ_ONLY_VOLUME,
            fs_name: U16CString::from_str("LCSLICE").unwrap(),
        })
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 5 {
        anyhow::bail!("usage: probe_dokan_slice <backing-file> <offset> <length> <drive-letter>");
    }
    let backing_path = &args[1];
    let base: u64 = args[2].parse().map_err(|_| anyhow::anyhow!("bad offset"))?;
    let length: u64 = args[3].parse().map_err(|_| anyhow::anyhow!("bad length"))?;
    let drive = args[4].trim_end_matches(':').trim_end_matches('\\');
    if drive.len() != 1 || !drive.chars().next().unwrap().is_ascii_alphabetic() {
        anyhow::bail!("drive-letter must be a single letter (e.g. Y)");
    }
    let mount = format!("{drive}:\\");

    let backing = Arc::new(File::open(backing_path)?);
    println!(
        "mounting {backing_path} [{base}..{end}] ({length} bytes) at {mount}\\{VOLUME_FILE}",
        end = base + length
    );

    dokan::init();
    let fs = SliceFs {
        backing,
        base,
        length,
    };
    let mount_u16 = U16CString::from_str(&mount)?;
    let options = MountOptions {
        flags: MountFlags::WRITE_PROTECT | MountFlags::CURRENT_SESSION,
        timeout: Duration::from_secs(30),
        allocation_unit_size: 4096,
        sector_size: 4096,
        ..Default::default()
    };
    let mut mounter = FileSystemMounter::new(&fs, &mount_u16, &options);
    let _handle = mounter.mount()?;
    println!("ready — Ctrl+C to unmount (auto-unmounts after 10 min)");
    std::thread::sleep(Duration::from_secs(600));
    Ok(())
}
