//! Raw FAT directory reads and directory-stream to image-offset mapping.

use std::sync::Arc;

use anyhow::{anyhow, Result};

use super::{FatGeom, FatTable, DENTRY_LEN};
use crate::core::evidence::image::DiskImage;
use crate::core::filesystem::tree::fs_tree::FsDataRun;

/// Bound directory traversal when a corrupt FAT never reaches an end marker.
const MAX_DIR_CLUSTERS: usize = 1 << 20;

/// Directory bytes in traversal order, with the physical source of each range.
pub struct DirectoryData {
    pub bytes: Vec<u8>,
    runs: Vec<FsDataRun>,
}

impl DirectoryData {
    /// Translate a complete, aligned directory slot into an absolute image offset.
    pub fn entry_offset(&self, slot_offset: u64) -> Result<u64> {
        if !slot_offset.is_multiple_of(DENTRY_LEN as u64) {
            anyhow::bail!("FAT directory slot offset is not aligned");
        }
        let range_index = self
            .runs
            .partition_point(|run| run.logical_offset <= slot_offset);
        let run = range_index
            .checked_sub(1)
            .and_then(|index| self.runs.get(index))
            .ok_or_else(|| anyhow!("FAT directory slot is outside the source ranges"))?;
        let relative_offset = slot_offset - run.logical_offset;
        if relative_offset
            .checked_add(DENTRY_LEN as u64)
            .is_none_or(|end| end > run.length)
        {
            anyhow::bail!("FAT directory slot is outside the source ranges");
        }
        run.physical_offset
            .checked_add(relative_offset)
            .ok_or_else(|| anyhow!("FAT directory entry offset overflow"))
    }
}

/// Read raw directory bytes and their source ranges. FAT12/16 root directories
/// use the fixed root region; other directories follow the FAT cluster chain.
pub fn read_dir_bytes(
    image: &Arc<DiskImage>,
    geom: &FatGeom,
    fat: &FatTable,
    first_cluster: u32,
) -> Result<DirectoryData> {
    if first_cluster < 2 {
        if geom.is_fat32 {
            // FAT32 root lives at root_cluster, never the fixed region.
            return read_cluster_chain(image, geom, fat, geom.root_cluster);
        }
        // FAT12/16 fixed root region.
        let root_start_sector = geom.reserved_sectors + geom.num_fats * geom.sectors_per_fat;
        let offset = geom.base_offset + root_start_sector as u64 * geom.bytes_per_sector as u64;
        let len = geom.root_entries as usize * DENTRY_LEN;
        let bytes = image.read_range(offset, len)?;
        if bytes.len() != len {
            anyhow::bail!("FAT fixed root directory short read");
        }
        return Ok(DirectoryData {
            bytes,
            runs: vec![FsDataRun {
                logical_offset: 0,
                physical_offset: offset,
                length: len as u64,
            }],
        });
    }
    read_cluster_chain(image, geom, fat, first_cluster)
}

fn read_cluster_chain(
    image: &Arc<DiskImage>,
    geom: &FatGeom,
    fat: &FatTable,
    first_cluster: u32,
) -> Result<DirectoryData> {
    let mut bytes = Vec::new();
    let mut runs = Vec::new();
    let cluster_size = geom.cluster_size() as usize;
    let max_cluster = geom.cluster_count() + 2;
    let mut cluster = first_cluster;
    let mut guard = 0usize;
    let mut visited = std::collections::HashSet::new();
    while cluster >= 2 && cluster < max_cluster && guard < MAX_DIR_CLUSTERS {
        if !visited.insert(cluster) {
            anyhow::bail!("FAT cyclic directory chain");
        }
        let offset = geom.cluster_offset(cluster);
        let cluster_bytes = image.read_range(offset, cluster_size)?;
        if cluster_bytes.len() != cluster_size {
            anyhow::bail!("FAT directory short read");
        }
        runs.push(FsDataRun {
            logical_offset: bytes.len() as u64,
            physical_offset: offset,
            length: cluster_size as u64,
        });
        bytes.extend_from_slice(&cluster_bytes);
        // The end-of-chain marker lives on the LAST cluster, so it must be read
        // *before* we stop — testing it at the top of the loop would drop the
        // final cluster, and read nothing at all for a single-cluster directory
        // such as the FAT32 root (whose own FAT entry is already EOC). That was
        // why root-directory `0xE5` deletions never surfaced.
        if fat.is_end_of_chain(cluster) {
            break;
        }
        let next = fat.next_cluster(cluster);
        if next < 2 || next >= max_cluster {
            anyhow::bail!("FAT invalid directory chain");
        }
        cluster = next;
        guard += 1;
    }
    if guard >= MAX_DIR_CLUSTERS {
        anyhow::bail!("FAT directory exceeds recovery limit");
    }
    Ok(DirectoryData { bytes, runs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_offset_requires_a_complete_aligned_source_slot() {
        let directory = DirectoryData {
            bytes: vec![0; 64],
            runs: vec![FsDataRun {
                logical_offset: 0,
                physical_offset: 4096,
                length: 64,
            }],
        };
        assert_eq!(directory.entry_offset(32).unwrap(), 4128);
        for offset in [1, 33, 64, u64::MAX - 31] {
            assert!(directory.entry_offset(offset).is_err());
        }
    }
}
