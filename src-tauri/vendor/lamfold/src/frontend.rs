//! The `FoldFrontend` trait — the contract every member of the flock implements,
//! plus the value types the substrate hands across the L2↔L3 boundary.
//!
//! A frontend contains **only** on-disk structure parsing and traversal.
//! Decompression ([`crate::codec`]), the immutable-block cache
//! ([`crate::BlockCache`]), bounded parsing ([`crate::read_cap`]), and integrity
//! verification ([`crate::Verifier`]) all come from the substrate, handed in via
//! [`SubstrateCtx`]. This keeps the per-format code thin and the shared,
//! security-critical work in one audited place.

use alloc::string::String;
use alloc::vec::Vec;

use crate::cache::BlockCache;
use crate::error::Result;
use crate::source::BlockSource;
use crate::verify::Verifier;

/// An opaque per-volume node handle (an inode number, a directory-record LBA, an
/// erofs nid — whatever the frontend uses internally).
pub type NodeId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Regular,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Debug)]
pub struct Metadata {
    pub kind: FileKind,
    pub size: u64,
    /// POSIX-style mode bits where the format records them (0 otherwise).
    pub mode: u32,
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub node: NodeId,
    pub kind: FileKind,
}

/// The shared substrate handed to a frontend on every call: the decompressed-
/// block cache and the integrity verifier (the shepherd). Borrowed, never owned
/// by the frontend, so one cache/verifier can serve nested frontends (a squashfs
/// mounted over a file inside an iso).
pub struct SubstrateCtx<'a> {
    pub cache: &'a mut BlockCache,
    pub verifier: &'a dyn Verifier,
}

/// A read-only media format frontend (a member of the flock). Generic over the
/// [`BlockSource`] it reads, so the same frontend serves a whole partition, a
/// logical volume, or an `.iso` loopback region — including recursively.
pub trait FoldFrontend<S: BlockSource>: Sized {
    /// Stable tag surfaced through trust events ("iso9660", "udf", "squashfs", …).
    const TAG: &'static str;

    /// Cheap, bounded superblock/magic sniff — must not over-read. Returns
    /// `true` if `src` looks like this format.
    fn probe(src: &mut S) -> Result<bool>;

    /// Mount the volume over `src`, given the shared substrate.
    fn open(src: S, cx: &mut SubstrateCtx<'_>) -> Result<Self>;

    /// The root directory node.
    fn root(&self) -> NodeId;

    /// Resolve one path component within `dir`.
    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        cx: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>>;

    /// List the entries of a directory (advisory: re-resolve by name before any
    /// security decision — same contract as LamBoot's `read_dir`).
    fn read_dir(&mut self, dir: NodeId, cx: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>>;

    /// Metadata for a node.
    fn metadata(&mut self, node: NodeId, cx: &mut SubstrateCtx<'_>) -> Result<Metadata>;

    /// Read up to `buf.len()` bytes of a regular file starting at `off`. Returns
    /// the number of bytes read (0 at EOF).
    fn read_at(
        &mut self,
        node: NodeId,
        off: u64,
        buf: &mut [u8],
        cx: &mut SubstrateCtx<'_>,
    ) -> Result<usize>;

    /// The target of a symbolic link (raw bytes), or `None` if `node` is not a
    /// symlink. Default: the format has no symlinks. Frontends that carry POSIX
    /// semantics (ISO9660 Rock Ridge, squashfs, erofs) override this; the boot
    /// path uses it to resolve e.g. `/boot/vmlinuz` → the real kernel.
    fn read_link(&mut self, node: NodeId, cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let _ = (node, cx);
        Ok(None)
    }
}
