//! Cross-platform compatibility shim for the vendored XFS reader.
//!
//! The upstream library reaches for `fuser::FileAttr` / `fuser::FileType`,
//! `libc::S_IF*` / `libc::E*` / `libc::SEEK_*`, and `os::unix::ffi::OsStr/OsString`
//! conversions. None of those are available — or have stable values — on
//! Windows, so this module provides POSIX-compatible portable replacements:
//!
//! - `FileType` and `FileAttr` mirror the small subset of `fuser` types the
//!   parser actually needs.
//! - `S_IF*` constants use the POSIX-defined octal values (`0o170000` &c.) so
//!   the mode-bit math in `dinode.rs` works identically on every host.
//! - `errno!` constants are the standard POSIX numbers, returned upward through
//!   `Result<_, i32>` exactly as upstream does — they never reach the OS, so
//!   collisions with the host's libc are harmless.
//! - `SEEK_DATA` / `SEEK_HOLE` are `3` / `4` (Linux numbering); the parser
//!   uses them only as opaque sentinel values inside its own bmap walker.

use std::time::SystemTime;

// POSIX file mode bits. Values are fixed by POSIX, so they're identical to
// libc::S_IF* on every Unix and just unavailable in Windows libc.
pub const S_IFMT: u32 = 0o170000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFBLK: u32 = 0o060000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFIFO: u32 = 0o010000;
pub const S_IFSOCK: u32 = 0o140000;

// errno values used internally by the parser. We surface them as plain `i32`
// to mirror the upstream Result<_, libc::c_int> shape; callers translate to
// host-appropriate errors at the boundary.
pub const ENOENT: i32 = 2;
pub const EIO: i32 = 5;
pub const ENXIO: i32 = 6;
pub const EINVAL: i32 = 22;
pub const EDESTADDRREQ: i32 = 89;
pub const ENOATTR: i32 = 93;

// lseek whence values for the parser's hole-vs-data bmap walker. Linux's
// numbering — opaque sentinels here, never passed to the host kernel.
pub const SEEK_DATA: i32 = 3;
pub const SEEK_HOLE: i32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileType {
    NamedPipe,
    CharDevice,
    BlockDevice,
    Directory,
    RegularFile,
    Symlink,
    Socket,
}

/// File attributes — the small subset the XFS parser produces from a dinode.
/// Mirrors `fuser::FileAttr` field-for-field (minus FUSE-specific `flags`
/// padding) so the upstream `DinodeCore::stat` body needed no logic edits.
#[derive(Clone, Copy, Debug)]
pub struct FileAttr {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime: SystemTime,
    pub mtime: SystemTime,
    pub ctime: SystemTime,
    pub crtime: Option<SystemTime>,
    pub mode: u16,
    pub inode_version: i8,
    pub raw_atime: (i64, u32),
    pub raw_mtime: (i64, u32),
    pub raw_ctime: (i64, u32),
    pub raw_crtime: Option<(i64, u32)>,
    pub kind: FileType,
    pub perm: u16,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u32,
    pub blksize: u32,
    pub flags: u32,
}

impl FileAttr {
    /// Decode XFS's on-disk SysV device identifier (14-bit major, 18-bit minor).
    pub fn device_numbers(&self) -> Option<(u32, u32)> {
        if matches!(self.kind, FileType::CharDevice | FileType::BlockDevice) {
            Some((self.rdev >> 18, self.rdev & 0x3ffff))
        } else {
            None
        }
    }
}

/// Map a POSIX file-mode word to a `FileType`. Returns `ENOENT` for unknown
/// type bits, mirroring the upstream behaviour where the FUSE server would
/// have served up `ENOENT` to the kernel.
pub fn posix_mode_to_file_type(mode: u16) -> Result<FileType, i32> {
    match (mode as u32) & S_IFMT {
        S_IFREG => Ok(FileType::RegularFile),
        S_IFDIR => Ok(FileType::Directory),
        S_IFLNK => Ok(FileType::Symlink),
        S_IFSOCK => Ok(FileType::Socket),
        S_IFCHR => Ok(FileType::CharDevice),
        S_IFBLK => Ok(FileType::BlockDevice),
        S_IFIFO => Ok(FileType::NamedPipe),
        _ => Err(ENOENT),
    }
}
