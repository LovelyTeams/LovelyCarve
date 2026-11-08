//! Superblock information used by forensic callers.

use crate::util::{read_u16le, read_u32le};
use crate::{Ext4, Ext4Error};
use alloc::vec::Vec;

/// Raw superblock identity, feature masks, counts and mount history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeInfo {
    /// ext family version: 2, 3 or 4, inferred from feature masks.
    pub format_version: u8,
    /// On-disk revision (0 is the original ext2 layout; 1 is dynamic).
    pub revision: u32,
    /// Compatible feature mask.
    pub compatible_features: u32,
    /// Incompatible feature mask.
    pub incompatible_features: u32,
    /// Read-only compatible feature mask.
    pub read_only_compatible_features: u32,
    /// Total inode slots, including unallocated slots.
    pub inode_count: u32,
    /// Free inode slots recorded by the superblock.
    pub free_inode_count: u32,
    /// Total filesystem blocks.
    pub block_count: u64,
    /// Free blocks; these are not individual bigalloc clusters.
    pub free_block_count: u64,
    /// Filesystem block size in bytes.
    pub block_size: u32,
    /// Size of each on-disk inode in bytes.
    pub inode_size: u16,
    /// Raw last mount path, without NUL padding.
    pub last_mount_path: Vec<u8>,
    /// Last mount time, unsigned seconds since the Unix epoch.
    pub last_mount_time: u64,
    /// Last write time, unsigned seconds since the Unix epoch.
    pub last_written_time: u64,
    /// Raw filesystem state flags.
    pub state: u16,
    /// Internal journal inode; zero means no internal journal.
    pub journal_inode: u32,
}

impl Ext4 {
    /// Get volume information. The filesystem must first pass `Ext4::load`.
    pub fn volume_info(&self) -> Result<VolumeInfo, Ext4Error> {
        let mut raw = [0; 1024];
        let block_size = self.block_size();
        self.read_from_block(1024 / u64::from(block_size), 1024 % block_size, &mut raw)?;
        let compatible = read_u32le(&raw, 0x5c);
        let incompatible = read_u32le(&raw, 0x60);
        let read_only = read_u32le(&raw, 0x64);
        let is_64bit = incompatible & 0x80 != 0;
        let path = &raw[0x88..0xc8];
        Ok(VolumeInfo {
            format_version: if compatible & 0x200 != 0
                || incompatible & 0x1f7c0 != 0
                || read_only & 0x378 != 0
            {
                4
            } else if compatible & 4 != 0 {
                3
            } else {
                2
            },
            revision: read_u32le(&raw, 0x4c),
            compatible_features: compatible,
            incompatible_features: incompatible,
            read_only_compatible_features: read_only,
            inode_count: read_u32le(&raw, 0),
            free_inode_count: read_u32le(&raw, 16),
            block_count: self.0.superblock.blocks_count,
            free_block_count: u64::from(read_u32le(&raw, 12))
                | if is_64bit {
                    u64::from(read_u32le(&raw, 0x158)) << 32
                } else {
                    0
                },
            block_size,
            inode_size: self.0.superblock.inode_size,
            last_mount_path: path[..path
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(path.len())]
                .to_vec(),
            last_mount_time: u64::from(read_u32le(&raw, 44)) | (u64::from(raw[0x275]) << 32),
            last_written_time: u64::from(read_u32le(&raw, 48)) | (u64::from(raw[0x274]) << 32),
            state: read_u16le(&raw, 58),
            journal_inode: if compatible & 4 != 0 {
                read_u32le(&raw, 0xe0)
            } else {
                0
            },
        })
    }
}
