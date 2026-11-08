// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::Ext4Read;
use crate::block_index::FsBlockIndex;
use crate::checksum::Checksum;
use crate::error::{CorruptKind, Ext4Error};
use crate::features::{IncompatibleFeatures, ReadOnlyCompatibleFeatures};
use crate::superblock::Superblock;
use crate::util::{read_u16le, read_u32le, u64_from_hilo, usize_from_u32};
use alloc::vec;
use alloc::vec::Vec;

pub(crate) type BlockGroupIndex = u32;

#[derive(Debug)]
pub(crate) struct BlockGroupDescriptor {
    pub(crate) inode_table_first_block: FsBlockIndex,
    checksum: u16,
}

impl BlockGroupDescriptor {
    const BG_CHECKSUM_OFFSET: usize = 0x1e;

    fn from_bytes(superblock: &Superblock, bytes: &[u8]) -> Self {
        const BG_INODE_TABLE_HI_OFFSET: usize = 0x28;

        let bg_inode_table_lo = read_u32le(bytes, 0x8);
        let bg_checksum = read_u16le(bytes, Self::BG_CHECKSUM_OFFSET);

        // Get the high bits of the inode table block.
        let bg_inode_table_hi = if superblock
            .incompatible_features
            .contains(IncompatibleFeatures::IS_64BIT)
        {
            read_u32le(bytes, BG_INODE_TABLE_HI_OFFSET)
        } else {
            0
        };

        let inode_table_first_block = u64_from_hilo(bg_inode_table_hi, bg_inode_table_lo);

        Self {
            inode_table_first_block,
            checksum: bg_checksum,
        }
    }

    /// Map from a block group descriptor index to the absolute byte
    /// within the file where the descriptor starts.
    fn get_start_byte(sb: &Superblock, bgd_index: BlockGroupIndex) -> Option<u64> {
        let bgd_start_block: u32 = if sb.block_size == 1024 { 2 } else { 1 };
        let bgd_per_block = sb
            .block_size
            .to_u32()
            .checked_div(u32::from(sb.block_group_descriptor_size))?;
        let descriptor_block = bgd_index.checked_div(bgd_per_block)?;
        let block_index = if sb
            .incompatible_features
            .contains(IncompatibleFeatures::META_BLOCK_GROUPS)
            && descriptor_block >= sb.first_meta_bg
        {
            let first_group = descriptor_block.checked_mul(bgd_per_block)?;
            let has_super = if first_group == 0 {
                true
            } else if sb.sparse_super2 {
                sb.backup_bgs.contains(&first_group)
            } else if !sb
                .read_only_compatible_features
                .contains(ReadOnlyCompatibleFeatures::SPARSE_SUPERBLOCKS)
            {
                true
            } else {
                first_group == 1
                    || [3u32, 5, 7].into_iter().any(|base| {
                        let mut group = first_group;
                        while group > 1 && group % base == 0 {
                            group /= base;
                        }
                        group == 1
                    })
            };
            u64::from(first_group)
                .checked_mul(u64::from(sb.blocks_per_group))?
                .checked_add(u64::from(sb.first_data_block))?
                .checked_add(u64::from(has_super))?
        } else {
            u64::from(bgd_start_block.checked_add(descriptor_block)?)
        };

        let offset_within_block = (bgd_index.checked_rem(bgd_per_block)?)
            .checked_mul(u32::from(sb.block_group_descriptor_size))?;

        block_index
            .checked_mul(sb.block_size.to_u64())?
            .checked_add(u64::from(offset_within_block))
    }

    /// Read a block group descriptor.
    fn read(
        sb: &Superblock,
        reader: &mut dyn Ext4Read,
        bgd_index: BlockGroupIndex,
    ) -> Result<Self, Ext4Error> {
        // Allocate a byte vec to read the raw data into.
        let block_group_descriptor_size = usize::from(sb.block_group_descriptor_size);
        let mut data = vec![0; block_group_descriptor_size];

        let start = Self::get_start_byte(sb, bgd_index)
            .ok_or(CorruptKind::BlockGroupDescriptor(bgd_index))?;
        reader.read(start, &mut data).map_err(Ext4Error::Io)?;

        let block_group_descriptor = Self::from_bytes(sb, &data);

        let has_metadata_checksums = sb
            .read_only_compatible_features
            .contains(ReadOnlyCompatibleFeatures::METADATA_CHECKSUMS);

        // Verify the descriptor checksum.
        if has_metadata_checksums {
            let mut checksum = Checksum::with_seed(sb.checksum_seed);
            checksum.update_u32_le(bgd_index);
            // Up to the checksum field.
            checksum.update(&data[..Self::BG_CHECKSUM_OFFSET]);
            // Zero'd checksum field.
            checksum.update_u16_le(0);
            // Rest of the block group descriptor.
            checksum.update(&data[Self::BG_CHECKSUM_OFFSET + 2..]);
            // Truncate to the lower 16 bits.
            let checksum = u16::try_from(checksum.finalize() & 0xffff).unwrap();

            if checksum != block_group_descriptor.checksum {
                return Err(CorruptKind::BlockGroupDescriptorChecksum(bgd_index).into());
            }
        } else if sb
            .read_only_compatible_features
            .contains(ReadOnlyCompatibleFeatures::GROUP_DESCRIPTOR_CHECKSUMS)
        {
            const ALGORITHM: crc::Algorithm<u16> = crc::Algorithm {
                init: 0xffff,
                ..crc::CRC_16_ARC
            };
            let crc = crc::Crc::<u16>::new(&ALGORITHM);
            let mut checksum = crc.digest();
            checksum.update(&sb.uuid.0);
            checksum.update(&bgd_index.to_le_bytes());
            checksum.update(&data[..Self::BG_CHECKSUM_OFFSET]);
            // Legacy CRC16 skips, rather than zeroes, the checksum field.
            checksum.update(&data[Self::BG_CHECKSUM_OFFSET + 2..]);
            if checksum.finalize() != block_group_descriptor.checksum {
                return Err(CorruptKind::BlockGroupDescriptorChecksum(bgd_index).into());
            }
        }

        Ok(block_group_descriptor)
    }

    /// Read all block group descriptors.
    pub(crate) fn read_all(
        sb: &Superblock,
        reader: &mut dyn Ext4Read,
    ) -> Result<Vec<Self>, Ext4Error> {
        let mut block_group_descriptors = Vec::with_capacity(usize_from_u32(sb.num_block_groups));

        for bgd_index in 0..sb.num_block_groups {
            let bgd = Self::read(sb, reader, bgd_index)?;
            block_group_descriptors.push(bgd);
        }

        Ok(block_group_descriptors)
    }
}
