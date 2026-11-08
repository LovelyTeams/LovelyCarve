/*
 * BSD 2-Clause License
 *
 * Copyright (c) 2021, Khaled Emara
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions are met:
 *
 * 1. Redistributions of source code must retain the above copyright notice, this
 *    list of conditions and the following disclaimer.
 *
 * 2. Redistributions in binary form must reproduce the above copyright notice,
 *    this list of conditions and the following disclaimer in the documentation
 *    and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
 * AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
 * DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
 * SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
 * CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
 * OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
 * OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */

//! Read-only XFS volume API.
//!
//! Portable read-only API. Each operation binds its own thread-local superblock.
//! Opening and new metadata/read paths return errors for malformed evidence;
//! inherited directory decoders retain a final unwind guard.

use std::cell::Cell;
use std::io::{Read, Seek};
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::block_reader::BlockReader;
use super::compat::{FileAttr, FileType, EIO, ENOENT};
use super::definitions::XfsIno;
use super::dinode::Dinode;
use super::sb::Sb;

thread_local! {
    static CURRENT_SUPERBLOCK: Cell<Option<Sb>> = const { Cell::new(None) };
}

/// Thread-local superblock slot. The XFS bincode `Decode` impls reach in here
/// mid-decode (e.g. `Dir2DataEntry::Decode` checks `sb.has_ftype()`,
/// `XfsDa3Blkinfo::Decode` cross-checks `sb.sb_uuid`). Each public `Volume`
/// operation binds its own superblock before decoding, so a process can browse
/// multiple XFS volumes with different UUIDs without the first opened volume
/// poisoning later lookups on the same image.
pub(crate) struct SuperblockSlot;

impl SuperblockSlot {
    pub(crate) fn set(&self, superblock: Sb) {
        CURRENT_SUPERBLOCK.with(|slot| slot.set(Some(superblock)));
    }

    pub(crate) fn get(&self) -> Option<Sb> {
        CURRENT_SUPERBLOCK.with(Cell::get)
    }
}

pub(crate) static SUPERBLOCK: SuperblockSlot = SuperblockSlot;

/// One directory entry surfaced by `Volume::readdir`.
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub ino: XfsIno,
    /// Cookie that callers feed back into `readdir(parent_ino, offset)` to
    /// resume after this entry. Internal to the on-disk format — opaque to
    /// the rest of the system.
    pub offset: i64,
    /// `None` if the on-disk directory was created without ftype recording
    /// (older XFS without `XFS_SB_FEAT_INCOMPAT_FTYPE`); the caller can
    /// recover the real kind via `getattr`.
    pub kind: Option<FileType>,
    /// Raw on-disk filename bytes. POSIX semantics — caller decides how to
    /// interpret as UTF-8 / WTF-8 / Latin-1.
    pub name: Vec<u8>,
}

struct DirectoryCache {
    inode: XfsIno,
    entries: Vec<DirEntry>,
    names: std::collections::HashMap<Vec<u8>, XfsIno>,
}

/// Read-only XFS volume backed by a buffered, block-aligned reader.
pub struct Volume<R: Read + Seek> {
    pub device: BlockReader<R>,
    pub sb: Sb,
    // One validated regular-file map, bounded by logical::data_extents limits.
    read_plan: Option<(XfsIno, i64, Vec<super::bmbt_rec::BmbtRec>)>,
    directories: std::collections::VecDeque<DirectoryCache>,
}

impl<R: Read + Seek> Volume<R> {
    /// Build a `Volume` over the given block device.
    pub fn open(reader: R) -> Result<Self, i32> {
        let mut device = BlockReader::new(reader);
        let superblock = Sb::read(&mut device)?;
        SUPERBLOCK.set(superblock);
        device.set_bufsize(superblock.inode_size());
        let root =
            catch_xfs_panic(|| Dinode::from(&mut device, &superblock, superblock.sb_rootino))?;
        if root.di_core.stat(superblock.sb_rootino)?.kind != FileType::Directory {
            return Err(EIO);
        }
        Ok(Volume {
            device,
            sb: superblock,
            read_plan: None,
            directories: Default::default(),
        })
    }

    /// Read the dinode for `ino`, mapping the FUSE-style root sentinel `1`
    /// to `sb.sb_rootino` for caller convenience. `Err(errno)` when the inode
    /// cannot be parsed (a freed/zeroed inode still referenced by a directory
    /// entry, an unsupported inode type/format, or an out-of-range number),
    /// so a whole-tree walk degrades per inode instead of panicking.
    fn read_dinode(&mut self, ino: XfsIno) -> Result<Dinode, i32> {
        SUPERBLOCK.set(self.sb);
        if ino == 1 || ino == self.sb.sb_rootino {
            return self.root_dinode();
        }
        self.device.set_bufsize(self.sb.inode_size());
        catch_xfs_panic(|| Dinode::from(&mut self.device, &self.sb, ino))
    }

    fn root_dinode(&mut self) -> Result<Dinode, i32> {
        SUPERBLOCK.set(self.sb);
        self.device.set_bufsize(self.sb.inode_size());
        catch_xfs_panic(|| Dinode::from(&mut self.device, &self.sb, self.sb.sb_rootino))
    }

    /// Resolve a single name within the directory inode `parent_ino`.
    pub fn lookup(&mut self, parent_ino: XfsIno, name: &[u8]) -> Result<XfsIno, i32> {
        let ascii_case_insensitive =
            self.sb.version() == 4 && self.sb.feature_flags() & 0x4000 != 0;
        self.directory_entries(parent_ino)?;
        let directory = self
            .directories
            .iter()
            .find(|directory| directory.inode == parent_ino)
            .ok_or(EIO)?;
        if let Some(inode) = directory.names.get(name) {
            return Ok(*inode);
        }
        if ascii_case_insensitive {
            return directory
                .entries
                .iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(name))
                .map(|entry| entry.ino)
                .ok_or(ENOENT);
        }
        Err(ENOENT)
    }

    fn directory_entries(&mut self, parent_ino: XfsIno) -> Result<&[DirEntry], i32> {
        SUPERBLOCK.set(self.sb);
        let position = match self
            .directories
            .iter()
            .position(|directory| directory.inode == parent_ino)
        {
            Some(position) => position,
            None => {
                let entries = catch_xfs_panic(|| {
                    let inode = self.read_dinode(parent_ino)?;
                    if inode.di_core.stat(parent_ino)?.kind != FileType::Directory {
                        return Err(EIO);
                    }
                    super::directory::read(
                        &mut self.device,
                        &self.sb,
                        &inode,
                        if parent_ino == 1 {
                            self.sb.sb_rootino
                        } else {
                            parent_ino
                        },
                    )
                })?;
                while self.directories.len() >= 32
                    || self
                        .directories
                        .iter()
                        .map(|directory| directory.entries.len())
                        .sum::<usize>()
                        + entries.len()
                        > 250_000
                {
                    if self.directories.pop_front().is_none() {
                        break;
                    }
                }
                let names = entries
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.ino))
                    .collect();
                self.directories.push_back(DirectoryCache {
                    inode: parent_ino,
                    entries,
                    names,
                });
                self.directories.len() - 1
            }
        };
        Ok(&self.directories.get(position).ok_or(EIO)?.entries)
    }

    /// Walk a `/`-rooted UTF-8 path from the root, returning the leaf inode.
    pub fn lookup_path(&mut self, path: &str) -> Result<XfsIno, i32> {
        self.lookup_path_bytes(path.as_bytes())
    }

    /// Resolve POSIX paths without lossy UTF-8 conversion.
    pub fn lookup_path_bytes(&mut self, path: &[u8]) -> Result<XfsIno, i32> {
        let mut current = self.sb.sb_rootino;
        for component in path.split(|byte| *byte == b'/').filter(|c| !c.is_empty()) {
            current = self.lookup(current, component)?;
        }
        Ok(current)
    }

    /// Build a `FileAttr` for the given inode.
    pub fn getattr(&mut self, ino: XfsIno) -> Result<FileAttr, i32> {
        SUPERBLOCK.set(self.sb);
        catch_xfs_panic(|| {
            let dinode = self.read_dinode(ino)?;
            let mut attributes = dinode.di_core.stat(ino)?;
            attributes.blksize = self.sb.sb_blocksize;
            if matches!(
                attributes.kind,
                FileType::CharDevice | FileType::BlockDevice
            ) {
                let offset =
                    self.sb
                        .inode_offset(if ino == 1 { self.sb.sb_rootino } else { ino })?
                        + dinode.di_core.literal_area_offset() as u64;
                self.device
                    .seek(std::io::SeekFrom::Start(offset))
                    .map_err(|_| EIO)?;
                let mut device = [0; 4];
                self.device.read_exact(&mut device).map_err(|_| EIO)?;
                attributes.rdev = u32::from_be_bytes(device);
            }
            Ok(attributes)
        })
    }

    /// List all directory entries or fail explicitly at the resource limit.
    pub fn readdir(&mut self, parent_ino: XfsIno) -> Result<Vec<DirEntry>, i32> {
        Ok(self.directory_entries(parent_ino)?.to_vec())
    }

    /// Read up to `size` bytes at `offset` from a regular file. Returns the
    /// owned bytes (possibly less than `size` near EOF).
    pub fn read(&mut self, ino: XfsIno, offset: i64, size: u32) -> Result<Vec<u8>, i32> {
        SUPERBLOCK.set(self.sb);
        catch_xfs_panic(|| {
            if self
                .read_plan
                .as_ref()
                .is_none_or(|(cached, _, _)| *cached != ino)
            {
                let inode = self.read_dinode(ino)?;
                if inode.di_core.di_mode as u32 & super::compat::S_IFMT != super::compat::S_IFREG {
                    return Err(super::compat::EINVAL);
                }
                if inode.di_core.is_realtime() {
                    return Err(super::compat::EINVAL);
                }
                let records = super::logical::data_extents(&mut self.device, &self.sb, &inode)?;
                self.read_plan = Some((ino, inode.di_core.di_size, records));
            }
            self.device.set_bufsize(self.sb.sb_blocksize as usize);
            let (_, file_size, records) = self.read_plan.as_ref().ok_or(EIO)?;
            super::logical::read(
                &mut self.device,
                &self.sb,
                records,
                *file_size,
                offset,
                size,
            )
        })
    }

    /// Read the target of a symlink inode, returned as raw bytes (no NUL).
    pub fn readlink(&mut self, ino: XfsIno) -> Result<Vec<u8>, i32> {
        SUPERBLOCK.set(self.sb);
        catch_xfs_panic(|| {
            let dinode = self.read_dinode(ino)?;
            super::symlink_extent::read(
                &mut self.device,
                &self.sb,
                &dinode,
                if ino == 1 { self.sb.sb_rootino } else { ino },
            )
        })
    }

    /// Read complete attributes, retaining raw name/value bytes.
    pub fn extended_attributes(
        &mut self,
        ino: XfsIno,
    ) -> Result<Vec<super::attributes::ExtendedAttribute>, i32> {
        SUPERBLOCK.set(self.sb);
        catch_xfs_panic(|| {
            let inode = self.read_dinode(ino)?;
            super::attributes::read(
                &mut self.device,
                &self.sb,
                &inode,
                if ino == 1 { self.sb.sb_rootino } else { ino },
            )
        })
    }

    /// Exact namespace-qualified attribute lookup; hash collisions cannot select a different name.
    pub fn get_extended_attribute(&mut self, ino: XfsIno, name: &[u8]) -> Result<Vec<u8>, i32> {
        let mut matches = self
            .extended_attributes(ino)?
            .into_iter()
            .filter(|attribute| attribute.name == name);
        let value = matches.next().ok_or(super::compat::ENOATTR)?.value;
        if matches.next().is_some() {
            return Err(EIO);
        }
        Ok(value)
    }

    /// Bounded random access to attribute logical bytes (values are at most 64 KiB).
    pub fn read_extended_attribute(
        &mut self,
        ino: XfsIno,
        name: &[u8],
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, i32> {
        let bytes = self.get_extended_attribute(ino, name)?;
        let start = offset.min(bytes.len() as u64) as usize;
        Ok(bytes[start..start.saturating_add(length).min(bytes.len())].to_vec())
    }

    /// Written and unwritten allocated extents, with volume-relative physical byte offsets.
    pub fn extents(&mut self, ino: XfsIno) -> Result<Vec<Extent>, i32> {
        SUPERBLOCK.set(self.sb);
        catch_xfs_panic(|| {
            let inode = self.read_dinode(ino)?;
            if inode.di_core.is_realtime() {
                return Err(super::compat::EINVAL);
            }
            let records = super::logical::data_extents(&mut self.device, &self.sb, &inode)?;
            records
                .iter()
                .map(|record| {
                    Ok(Extent {
                        logical_offset: record
                            .br_startoff
                            .checked_mul(u64::from(self.sb.sb_blocksize))
                            .ok_or(EIO)?,
                        physical_offset: self
                            .sb
                            .extent_offset(record.br_startblock, record.br_blockcount)?,
                        length: record
                            .br_blockcount
                            .checked_mul(u64::from(self.sb.sb_blocksize))
                            .ok_or(EIO)?,
                        unwritten: record.br_flag,
                    })
                })
                .collect()
        })
    }

    /// Borrow the parsed superblock (for callers that need to render volume
    /// metadata in their own UI).
    pub fn superblock(&self) -> &Sb {
        &self.sb
    }
}

fn catch_xfs_panic<T>(op: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> {
    match catch_unwind(AssertUnwindSafe(op)) {
        Ok(result) => result,
        Err(_) => Err(EIO),
    }
}

/// Allocated extent. Holes have no physical provenance; unwritten data reads as zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extent {
    pub logical_offset: u64,
    pub physical_offset: u64,
    pub length: u64,
    pub unwritten: bool,
}
