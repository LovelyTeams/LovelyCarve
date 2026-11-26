//! Read-only VMware VMFS datastore structures.

mod descriptor;
mod directory;
mod filesystem;
mod lvm;
#[cfg(test)]
mod tests;
mod volume;

use std::io::{self, Read, Seek, SeekFrom};

pub use descriptor::{FileInfo, FileKind, ROOT};
pub use directory::Entry;
pub use filesystem::{FileReader, Filesystem};
pub use lvm::{MemberInfo, probe_member};
pub use volume::{Geometry, SourceSpan, Version, Volume};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] io::Error),
    #[error("VMFS signature absent")]
    Signature,
    #[error("corrupt VMFS: {0}")]
    Corrupt(&'static str),
    #[error("unsupported VMFS: {0}")]
    Unsupported(&'static str),
    #[error("VMFS requires all extent members")]
    MissingMembers,
    #[error("VMFS entry not found")]
    NotFound,
    #[error("VMFS entry is not a directory")]
    NotDirectory,
}

pub type Result<T> = std::result::Result<T, Error>;

fn read_exact_at<R: Read + Seek>(reader: &mut R, offset: u64, bytes: &mut [u8]) -> io::Result<()> {
    reader.seek(SeekFrom::Start(offset))?;
    reader.read_exact(bytes)
}

fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn le64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn name(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes.split(|&byte| byte == 0).next().unwrap_or_default()).into_owned()
}
