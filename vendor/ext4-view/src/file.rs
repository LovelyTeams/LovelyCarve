// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::Ext4;
use crate::block_index::FsBlockIndex;
use crate::error::Ext4Error;
use crate::inode::{Inode, InodeFlags};
use crate::iters::file_blocks::FileBlocks;
use crate::metadata::Metadata;
use crate::path::Path;
use crate::resolve::FollowSymlinks;
use crate::util::usize_from_u32;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "std")]
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};

/// An open file within an [`Ext4`] filesystem.
pub struct File {
    fs: Ext4,
    inode: Inode,
    file_blocks: FileBlocks,
    resident: Option<Vec<u8>>,

    /// Current byte offset within the file.
    position: u64,

    /// Current block within the file. This is an absolute block index
    /// within the filesystem.
    ///
    /// If `None`, either the next block needs to be fetched from the
    /// `file_blocks` iterator, or the end of the file has been reached.
    block_index: Option<FsBlockIndex>,
}

impl File {
    /// Open the file at `path`.
    pub(crate) fn open(fs: &Ext4, path: Path<'_>) -> Result<Self, Ext4Error> {
        let inode = fs.path_to_inode(path, FollowSymlinks::All)?;

        if inode.metadata.is_dir() {
            return Err(Ext4Error::IsADirectory);
        }
        if !inode.metadata.file_type.is_regular_file() {
            return Err(Ext4Error::IsASpecialFile);
        }

        Self::open_inode(fs, inode)
    }

    /// Open `inode`. Note that unlike `File::open`, this allows any
    /// type of `inode` to be opened, including directories and
    /// symlinks. This is used by `Ext4::read_inode_file`.
    pub(crate) fn open_inode(fs: &Ext4, inode: Inode) -> Result<Self, Ext4Error> {
        if inode.flags.contains(InodeFlags::DIRECTORY_ENCRYPTED) {
            return Err(Ext4Error::Encrypted);
        }
        let resident = if inode.flags.contains(InodeFlags::INLINE_DATA) {
            Some(crate::xattr::inline_payload(fs, &inode)?)
        } else {
            None
        };
        Ok(Self {
            resident,
            fs: fs.clone(),
            position: 0,
            file_blocks: FileBlocks::new(fs.clone(), &inode)?,
            inode,
            block_index: None,
        })
    }

    /// Get the file metadata.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.inode.metadata
    }

    /// Read bytes from the file into `buf`, returning how many bytes
    /// were read. The number may be smaller than the length of the
    /// input buffer.
    ///
    /// This advances the position of the file by the number of bytes
    /// read, so calling `read_bytes` repeatedly can be used to read the
    /// entire file.
    ///
    /// Returns `Ok(0)` if the end of the file has been reached.
    pub fn read_bytes(&mut self, mut buf: &mut [u8]) -> Result<usize, Ext4Error> {
        // Nothing to do if output buffer is empty.
        if buf.is_empty() {
            return Ok(0);
        }

        // Nothing to do if already at the end of the file.
        if self.position >= self.inode.metadata.size_in_bytes {
            return Ok(0);
        }

        // Get the number of bytes remaining in the file, starting from
        // the current `position`.
        //
        // OK to unwrap: just checked that `position` is less than the
        // file size.
        let bytes_remaining = self
            .inode
            .metadata
            .size_in_bytes
            .checked_sub(self.position)
            .unwrap();

        // If the the number of bytes remaining is less than the output
        // buffer length, shrink the buffer.
        //
        // If the conversion to `usize` fails, the output buffer is
        // definitely not larger than the remaining bytes to read.
        if let Ok(bytes_remaining) = usize::try_from(bytes_remaining) {
            if buf.len() > bytes_remaining {
                buf = &mut buf[..bytes_remaining];
            }
        }

        if let Some(data) = &self.resident {
            let start = usize::try_from(self.position).map_err(|_| Ext4Error::FileTooLarge)?;
            let source = data
                .get(start..start + buf.len())
                .ok_or(crate::error::CorruptKind::InlineData(self.inode.index))?;
            buf.copy_from_slice(source);
            self.position += buf.len() as u64;
            return Ok(buf.len());
        }

        let block_size = self.fs.0.superblock.block_size;

        // Get the block to read from.
        let block_index = if let Some(block_index) = self.block_index {
            block_index
        } else {
            // OK to unwrap: already checked that the position is not at
            // the end of the file, so there must be at least one more
            // block to read.
            let block_index = self
                .file_blocks
                .next()
                .ok_or(crate::error::CorruptKind::ExtentBlock(self.inode.index))??;

            self.block_index = Some(block_index);

            block_index
        };

        // Byte offset within the current block.
        //
        // OK to unwrap: block size fits in a `u32`, so an offset within
        // the block will as well.
        let offset_within_block: u32 =
            u32::try_from(self.position % block_size.to_nz_u64()).unwrap();

        // OK to unwrap: `offset_within_block` is always less than or
        // equal to the block length.
        //
        // Note that if this block is at the end of the file, the block
        // may extend past the actual number of bytes in the file. This
        // does not matter because the output buffer's length was
        // already capped earlier against the number of bytes remaining
        // in the file.
        let bytes_remaining_in_block: u32 = block_size
            .to_u32()
            .checked_sub(offset_within_block)
            .unwrap();

        // If the output buffer is larger than the number of bytes
        // remaining in the block, shink the buffer.
        if buf.len() > usize_from_u32(bytes_remaining_in_block) {
            buf = &mut buf[..usize_from_u32(bytes_remaining_in_block)];
        }

        // OK to unwrap: the buffer length has been capped so that it
        // cannot be larger than the block size, and the block size fits
        // in a `u32`.
        let buf_len_u32: u32 = buf.len().try_into().unwrap();

        // Read the block data, or zeros if in a hole.
        if block_index == 0 {
            buf.fill(0);
        } else {
            self.fs
                .read_from_block(block_index, offset_within_block, buf)?;
        }

        // OK to unwrap: reads don't extend past a block, so this is at
        // most `block_size`, which always fits in a `u32`.
        let new_offset_within_block: u32 = offset_within_block.checked_add(buf_len_u32).unwrap();

        // If the end of this block has been reached, clear
        // `self.block_index` so that the next call fetches a new block
        // from the iterator.
        if new_offset_within_block >= block_size {
            self.block_index = None;
        }

        // OK to unwrap: the buffer length is capped such that this
        // calculation is at most the length of the file, which fits in
        // a `u64`.
        self.position = self.position.checked_add(u64::from(buf_len_u32)).unwrap();

        Ok(buf.len())
    }

    /// Current position within the file.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Seek from the start of the file to `position`.
    ///
    /// Seeking past the end of the file is allowed.
    pub fn seek_to(&mut self, position: u64) -> Result<(), Ext4Error> {
        if self.resident.is_some() {
            self.position = position;
            return Ok(());
        }
        // Reset iteration.
        self.file_blocks = FileBlocks::new(self.fs.clone(), &self.inode)?;
        self.block_index = None;

        // Advance the block iterator by the number of whole blocks in
        // `position`.
        let num_blocks = (position / self.fs.0.superblock.block_size.to_nz_u64())
            .min(u64::from(self.inode.file_size_in_blocks()));
        for _ in 0..num_blocks {
            match self.file_blocks.next() {
                Some(block) => {
                    block?;
                }
                None => break,
            }
        }

        self.position = position;

        Ok(())
    }
}

impl Debug for File {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("File")
            // Just show the index from `self.inode`, the full `Inode`
            // output is verbose.
            .field("inode", &self.inode.index)
            .field("position", &self.position)
            // Don't show all fields, as that would make the output less
            // readable.
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "std")]
impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Ok(self.read_bytes(buf)?)
    }
}

#[cfg(feature = "std")]
impl Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let pos = match pos {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(offset) => {
                // file_size + offset:
                i64::try_from(self.inode.metadata.size_in_bytes)
                    .ok()
                    .and_then(|pos| pos.checked_add(offset))
                    .and_then(|pos| pos.try_into().ok())
                    .ok_or(ErrorKind::InvalidInput)?
            }
            SeekFrom::Current(offset) => {
                // current_pos + offset:
                i64::try_from(self.position)
                    .ok()
                    .and_then(|pos| pos.checked_add(offset))
                    .and_then(|pos| pos.try_into().ok())
                    .ok_or(ErrorKind::InvalidInput)?
            }
        };

        self.seek_to(pos)?;

        Ok(self.position)
    }
}
