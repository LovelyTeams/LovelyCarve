//! Cross-platform read-only XFS reader, vendored from `xfuse` and stripped of
//! FUSE / Unix-only deps so it can be linked into a Tauri Windows build.
//!
//! Portable APIs include fallible open, raw-byte path lookup, inode metadata,
//! checked data/attribute extents, complete directory enumeration and seekable
//! reads. LovelyCarve's local format work is recorded in LOVELYCARVE.md.
//! Inherited xfuse types remain for compatibility with inode fork decoding.
#![allow(dead_code)]

mod libxfuse;

pub use libxfuse::attributes::ExtendedAttribute;
pub use libxfuse::block_reader::BlockReader;
pub use libxfuse::compat::{
    posix_mode_to_file_type, FileAttr, FileType, S_IFBLK, S_IFCHR, S_IFDIR, S_IFIFO, S_IFLNK,
    S_IFMT, S_IFREG, S_IFSOCK,
};
pub use libxfuse::definitions::XfsIno;
pub use libxfuse::dir3::{
    XFS_DIR3_FT_BLKDEV, XFS_DIR3_FT_CHRDEV, XFS_DIR3_FT_DIR, XFS_DIR3_FT_FIFO,
    XFS_DIR3_FT_REG_FILE, XFS_DIR3_FT_SOCK, XFS_DIR3_FT_SYMLINK,
};
pub use libxfuse::sb::Sb;
pub use libxfuse::volume::{DirEntry, Extent, Volume};
