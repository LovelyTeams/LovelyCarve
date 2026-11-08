//! Ext4 backup-superblock anchors distributed across the volume.
use super::*;

/// Check the recorded sparse-superblock locations, including their group
/// numbers. Early directories may all fit in one stripe; these independent
/// copies distinguish layouts that happen to reproduce that stripe.
pub(super) fn ext4_backups_match(
    source: &(dyn SectorSource + Send + Sync),
    base: u64,
    len: u64,
) -> bool {
    let Some(primary) = read_rel(source, base, len, 1024, 1024) else {
        return false;
    };
    if primary.get(0x38..0x3a) != Some(&[0x53, 0xef]) {
        return false;
    }
    let Some(log_block_size) = le32(&primary, 0x18).filter(|value| *value <= 6) else {
        return false;
    };
    let block_size = 1024u64 << log_block_size;
    let first_block = le32(&primary, 0x14).unwrap_or(0) as u64;
    let blocks_per_group = le32(&primary, 0x20).unwrap_or(0) as u64;
    if blocks_per_group == 0 {
        return false;
    }
    let incompat = le32(&primary, 0x60).unwrap_or(0);
    let mut blocks = le32(&primary, 4).unwrap_or(0) as u64;
    if incompat & 0x80 != 0 {
        blocks |= (le32(&primary, 0x150).unwrap_or(0) as u64) << 32;
    }
    if blocks <= first_block || blocks.checked_mul(block_size).is_none_or(|size| size > len) {
        return false;
    }
    let groups = (blocks - first_block).div_ceil(blocks_per_group);
    let mut backups = std::collections::BTreeSet::new();
    if le32(&primary, 0x5c).unwrap_or(0) & 0x200 != 0 {
        // sparse_super2 explicitly records its two backup groups.
        backups.extend([
            le32(&primary, 0x24c).unwrap_or(0) as u64,
            le32(&primary, 0x250).unwrap_or(0) as u64,
        ]);
    } else if le32(&primary, 0x64).unwrap_or(0) & 1 != 0 {
        // sparse_super: group 1 and powers of 3, 5 and 7.
        backups.insert(1);
        for factor in [3u64, 5, 7] {
            let mut group = factor;
            while group < groups && group <= u16::MAX as u64 {
                backups.insert(group);
                group = group.saturating_mul(factor);
            }
        }
    } else {
        backups.extend([
            1,
            groups / 4,
            groups / 2,
            groups * 3 / 4,
            groups.saturating_sub(1),
        ]);
    }
    backups.retain(|group| *group > 0 && *group < groups && *group <= u16::MAX as u64);
    if backups.len() < 2 {
        return false;
    }
    for group in backups {
        let Some(offset) = group
            .checked_mul(blocks_per_group)
            .and_then(|block| block.checked_add(first_block))
            .and_then(|block| block.checked_mul(block_size))
        else {
            return false;
        };
        let Some(backup) = read_rel(source, base, len, offset, 1024) else {
            return false;
        };
        if backup[0x38..0x3a] != primary[0x38..0x3a]
            || backup[0x68..0x78] != primary[0x68..0x78]
            || backup[4..8] != primary[4..8]
            || backup[0x14..0x2c] != primary[0x14..0x2c]
            || le16(&backup, 0x5a) != Some(group as u16)
        {
            return false;
        }
    }
    true
}
