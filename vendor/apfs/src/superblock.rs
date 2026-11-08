use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek, SeekFrom};

use crate::error::{ApfsError, Result};
use crate::fletcher;
use crate::object::{OBJECT_TYPE_NX_SUPERBLOCK, ObjectHeader};

/// NX_MAGIC = "NXSB" as little-endian u32
pub const NX_MAGIC: u32 = 0x4253584E;

/// APSB_MAGIC = "APSB" as little-endian u32
pub const APSB_MAGIC: u32 = 0x42535041;

/// Maximum number of volume OIDs in a container
pub const NX_MAX_FILE_SYSTEMS: usize = 100;

/// Container superblock (NXSB) — the root structure of an APFS container.
#[derive(Debug, Clone)]
pub struct NxSuperblock {
    pub header: ObjectHeader,
    pub magic: u32,
    pub block_size: u32,
    pub block_count: u64,
    pub features: u64,
    pub readonly_compatible_features: u64,
    pub incompatible_features: u64,
    pub uuid: [u8; 16],
    pub next_oid: u64,
    pub next_xid: u64,
    pub xp_desc_blocks: u32,
    pub xp_data_blocks: u32,
    pub xp_desc_base: u64, // paddr_t — physical block of checkpoint descriptor area
    pub xp_data_base: u64,
    pub xp_desc_next: u32,
    pub xp_data_next: u32,
    pub xp_desc_index: u32,
    pub xp_desc_len: u32,
    pub xp_data_index: u32,
    pub xp_data_len: u32,
    pub spaceman_oid: u64,
    pub omap_oid: u64, // Physical block of container object map
    pub reaper_oid: u64,
    pub max_file_systems: u32,
    pub fs_oids: Vec<u64>, // Volume superblock OIDs (virtual)
}

impl NxSuperblock {
    /// Parse the container superblock from a raw block.
    pub fn parse(block: &[u8]) -> Result<Self> {
        let header = ObjectHeader::parse(block)?;
        let mut cursor = Cursor::new(block);
        cursor.set_position(ObjectHeader::SIZE as u64);

        let magic = cursor.read_u32::<LittleEndian>()?;
        if magic != NX_MAGIC {
            return Err(ApfsError::InvalidMagic(magic));
        }

        let block_size = cursor.read_u32::<LittleEndian>()?;
        let block_count = cursor.read_u64::<LittleEndian>()?;
        let features = cursor.read_u64::<LittleEndian>()?;
        let readonly_compatible_features = cursor.read_u64::<LittleEndian>()?;
        let incompatible_features = cursor.read_u64::<LittleEndian>()?;

        let mut uuid = [0u8; 16];
        std::io::Read::read_exact(&mut cursor, &mut uuid)?;

        let next_oid = cursor.read_u64::<LittleEndian>()?;
        let next_xid = cursor.read_u64::<LittleEndian>()?;

        let xp_desc_blocks = cursor.read_u32::<LittleEndian>()?;
        let xp_data_blocks = cursor.read_u32::<LittleEndian>()?;
        let xp_desc_base = cursor.read_u64::<LittleEndian>()?;
        let xp_data_base = cursor.read_u64::<LittleEndian>()?;
        let xp_desc_next = cursor.read_u32::<LittleEndian>()?;
        let xp_data_next = cursor.read_u32::<LittleEndian>()?;
        let xp_desc_index = cursor.read_u32::<LittleEndian>()?;
        let xp_desc_len = cursor.read_u32::<LittleEndian>()?;
        let xp_data_index = cursor.read_u32::<LittleEndian>()?;
        let xp_data_len = cursor.read_u32::<LittleEndian>()?;

        let spaceman_oid = cursor.read_u64::<LittleEndian>()?;
        let omap_oid = cursor.read_u64::<LittleEndian>()?;
        let reaper_oid = cursor.read_u64::<LittleEndian>()?;

        let _test_type = cursor.read_u32::<LittleEndian>()?; // nx_test_type
        let max_file_systems = cursor.read_u32::<LittleEndian>()?;

        let fs_count = std::cmp::min(max_file_systems as usize, NX_MAX_FILE_SYSTEMS);
        let mut fs_oids = Vec::with_capacity(fs_count);
        for _ in 0..fs_count {
            fs_oids.push(cursor.read_u64::<LittleEndian>()?);
        }

        Ok(NxSuperblock {
            header,
            magic,
            block_size,
            block_count,
            features,
            readonly_compatible_features,
            incompatible_features,
            uuid,
            next_oid,
            next_xid,
            xp_desc_blocks,
            xp_data_blocks,
            xp_desc_base,
            xp_data_base,
            xp_desc_next,
            xp_data_next,
            xp_desc_index,
            xp_desc_len,
            xp_data_index,
            xp_data_len,
            spaceman_oid,
            omap_oid,
            reaper_oid,
            max_file_systems,
            fs_oids,
        })
    }
}

/// Volume superblock (APSB) — one per filesystem within a container.
#[derive(Debug, Clone)]
pub struct ApfsSuperblock {
    pub header: ObjectHeader,
    pub magic: u32,
    pub fs_index: u32,
    pub features: u64,
    pub readonly_compatible_features: u64,
    pub incompatible_features: u64,
    pub unmount_time: u64,
    pub fs_reserve_block_count: u64,
    pub fs_quota_block_count: u64,
    pub fs_alloc_count: u64,
    // Wrapped metadata crypto state (20 bytes).
    pub root_tree_type: u32,
    pub extentref_tree_type: u32,
    pub snap_meta_tree_type: u32,
    pub omap_oid: u64,      // Physical block of volume object map
    pub root_tree_oid: u64, // Virtual OID of the catalog (fs root) B-tree
    pub extentref_tree_oid: u64,
    pub snap_meta_tree_oid: u64,
    pub revert_to_xid: u64,
    pub revert_to_sblock_oid: u64,
    pub next_obj_id: u64,
    pub num_files: u64,
    pub num_directories: u64,
    pub num_symlinks: u64,
    pub num_other_fsobjects: u64,
    pub num_snapshots: u64,
    pub total_blocks_alloced: u64,
    pub total_blocks_freed: u64,
    pub uuid: [u8; 16],
    pub last_mod_time: u64,
    pub fs_flags: u64,
    pub volume_name: String,
    pub role: u16,
    pub volume_group_id: [u8; 16],
}

impl ApfsSuperblock {
    /// Parse volume superblock from a raw block.
    pub fn parse(block: &[u8]) -> Result<Self> {
        let header = ObjectHeader::parse(block)?;
        let mut cursor = Cursor::new(block);
        cursor.set_position(ObjectHeader::SIZE as u64);

        let magic = cursor.read_u32::<LittleEndian>()?;
        if magic != APSB_MAGIC {
            return Err(ApfsError::InvalidMagic(magic));
        }

        let fs_index = cursor.read_u32::<LittleEndian>()?;
        let features = cursor.read_u64::<LittleEndian>()?;
        let readonly_compatible_features = cursor.read_u64::<LittleEndian>()?;
        let incompatible_features = cursor.read_u64::<LittleEndian>()?;
        let unmount_time = cursor.read_u64::<LittleEndian>()?;
        let fs_reserve_block_count = cursor.read_u64::<LittleEndian>()?;
        let fs_quota_block_count = cursor.read_u64::<LittleEndian>()?;
        let fs_alloc_count = cursor.read_u64::<LittleEndian>()?;

        // Skip wrapped_meta_crypto_state_t (20 bytes):
        // { major_version: u16, minor_version: u16, cpflags: u32, persistent_class: u32,
        //   key_os_version: u32, key_revision: u16, unused: u16 } = 20 bytes
        // We're at APSB + 0x40, need to reach APSB + 0x54 (root_tree_type)
        let mut _skip = [0u8; 20];
        std::io::Read::read_exact(&mut cursor, &mut _skip)?;

        let root_tree_type = cursor.read_u32::<LittleEndian>()?;
        let extentref_tree_type = cursor.read_u32::<LittleEndian>()?;
        let snap_meta_tree_type = cursor.read_u32::<LittleEndian>()?;

        let omap_oid = cursor.read_u64::<LittleEndian>()?;
        let root_tree_oid = cursor.read_u64::<LittleEndian>()?;
        let extentref_tree_oid = cursor.read_u64::<LittleEndian>()?;
        let snap_meta_tree_oid = cursor.read_u64::<LittleEndian>()?;

        let revert_to_xid = cursor.read_u64::<LittleEndian>()?;
        let revert_to_sblock_oid = cursor.read_u64::<LittleEndian>()?;

        let next_obj_id = cursor.read_u64::<LittleEndian>()?;
        let num_files = cursor.read_u64::<LittleEndian>()?;
        let num_directories = cursor.read_u64::<LittleEndian>()?;
        let num_symlinks = cursor.read_u64::<LittleEndian>()?;
        let num_other_fsobjects = cursor.read_u64::<LittleEndian>()?;
        let num_snapshots = cursor.read_u64::<LittleEndian>()?;
        let total_blocks_alloced = cursor.read_u64::<LittleEndian>()?;
        let total_blocks_freed = cursor.read_u64::<LittleEndian>()?;

        let mut uuid = [0u8; 16];
        std::io::Read::read_exact(&mut cursor, &mut uuid)?;

        let last_mod_time = cursor.read_u64::<LittleEndian>()?;
        let fs_flags = cursor.read_u64::<LittleEndian>()?;

        // formatted_by (apfs_modified_by_t: 32-byte name + 8-byte timestamp + 8-byte last_xid)
        let mut _formatted_by = [0u8; 48];
        std::io::Read::read_exact(&mut cursor, &mut _formatted_by)?;

        // modified_by array: 8 entries of apfs_modified_by_t (48 bytes each) = 384 bytes
        let mut _modified_by = [0u8; 48];
        for _ in 0..8 {
            std::io::Read::read_exact(&mut cursor, &mut _modified_by)?;
        }

        // volume_name: null-terminated UTF-8, up to 256 bytes
        let mut name_buf = [0u8; 256];
        std::io::Read::read_exact(&mut cursor, &mut name_buf)?;
        let volume_name = {
            let nul_pos = name_buf.iter().position(|&b| b == 0).unwrap_or(256);
            String::from_utf8_lossy(&name_buf[..nul_pos]).to_string()
        };

        let role = u16::from_le_bytes(
            block
                .get(964..966)
                .ok_or_else(|| ApfsError::CorruptedData("short APSB role".into()))?
                .try_into()
                .map_err(|_| ApfsError::CorruptedData("short APSB role".into()))?,
        );
        let mut volume_group_id = [0u8; 16];
        volume_group_id.copy_from_slice(
            block
                .get(1008..1024)
                .ok_or_else(|| ApfsError::CorruptedData("short APSB volume group".into()))?,
        );
        Ok(ApfsSuperblock {
            header,
            magic,
            fs_index,
            features,
            readonly_compatible_features,
            incompatible_features,
            unmount_time,
            fs_reserve_block_count,
            fs_quota_block_count,
            fs_alloc_count,
            root_tree_type,
            extentref_tree_type,
            snap_meta_tree_type,
            omap_oid,
            root_tree_oid,
            extentref_tree_oid,
            snap_meta_tree_oid,
            revert_to_xid,
            revert_to_sblock_oid,
            next_obj_id,
            num_files,
            num_directories,
            num_symlinks,
            num_other_fsobjects,
            num_snapshots,
            total_blocks_alloced,
            total_blocks_freed,
            uuid,
            last_mod_time,
            fs_flags,
            volume_name,
            role,
            volume_group_id,
        })
    }
}

/// Scan the checkpoint descriptor area for the latest valid NX superblock.
///
/// The checkpoint descriptor area starts at `xp_desc_base` and contains
/// `xp_desc_blocks` blocks. We scan all of them looking for NX_SUPERBLOCK
/// objects and return the one with the highest transaction ID (xid).
pub fn find_latest_nxsb<R: Read + Seek>(
    reader: &mut R,
    nxsb: &NxSuperblock,
) -> Result<NxSuperblock> {
    let block_size = nxsb.block_size;
    let base = nxsb.xp_desc_base;
    let count = nxsb.xp_desc_blocks;
    if count & 0x8000_0000 != 0 {
        return Err(ApfsError::Unsupported(
            "B-tree checkpoint descriptor area".into(),
        ));
    }
    if base
        .checked_add(u64::from(count))
        .is_none_or(|end| end > nxsb.block_count)
    {
        return Err(ApfsError::CorruptedData(
            "checkpoint descriptor range outside container".into(),
        ));
    }

    let mut best: Option<NxSuperblock> = None;
    let mut best_xid: u64 = 0;

    for i in 0..count as u64 {
        let block_num = base + i;
        let offset = block_num * block_size as u64;

        reader.seek(SeekFrom::Start(offset))?;
        let mut block = vec![0u8; block_size as usize];
        if reader.read_exact(&mut block).is_err() {
            continue;
        }

        // Verify checksum
        if !fletcher::verify_object(&block) {
            continue;
        }

        // Parse header to check type
        let header = match ObjectHeader::parse(&block) {
            Ok(h) => h,
            Err(_) => continue,
        };

        if header.object_type() != OBJECT_TYPE_NX_SUPERBLOCK {
            continue;
        }

        // Parse the full superblock
        let candidate = match NxSuperblock::parse(&block) {
            Ok(sb) => sb,
            Err(_) => continue,
        };

        if candidate.magic != NX_MAGIC
            || candidate.uuid != nxsb.uuid
            || candidate.block_size != nxsb.block_size
            || candidate.block_count != nxsb.block_count
            || candidate.incompatible_features != nxsb.incompatible_features
        {
            continue;
        }

        if candidate.header.xid > best_xid {
            best_xid = candidate.header.xid;
            best = Some(candidate);
        }
    }

    // If we found a newer one in the checkpoint area, use it.
    // Otherwise fall back to the block-0 superblock.
    match best {
        Some(sb) if sb.header.xid > nxsb.header.xid => Ok(sb),
        _ => Ok(nxsb.clone()),
    }
}

/// Read and parse the container superblock from block 0.
pub fn read_nxsb<R: Read + Seek>(reader: &mut R) -> Result<NxSuperblock> {
    reader.seek(SeekFrom::Start(0))?;

    // Read geometry before verifying the full object. A checksum over the
    // first 4096 bytes cannot validate a larger APFS object.
    let mut prefix = [0u8; 40];
    reader.read_exact(&mut prefix)?;
    let size = u32::from_le_bytes(
        prefix[36..40]
            .try_into()
            .map_err(|_| ApfsError::CorruptedData("short NX header".into()))?,
    );
    if !(4096..=65536).contains(&size) || !size.is_power_of_two() {
        return Err(ApfsError::CorruptedData(format!(
            "invalid APFS block size {size}"
        )));
    }
    reader.seek(SeekFrom::Start(0))?;
    let mut block = vec![0u8; size as usize];
    reader.read_exact(&mut block)?;
    if !fletcher::verify_object(&block) {
        return Err(ApfsError::InvalidChecksum);
    }
    let superblock = NxSuperblock::parse(&block)?;
    if superblock.header.object_type() != OBJECT_TYPE_NX_SUPERBLOCK {
        return Err(ApfsError::CorruptedData(
            "block zero is not an NX superblock".into(),
        ));
    }
    if superblock.incompatible_features & 1 != 0 {
        return Err(ApfsError::Unsupported("APFS version 1".into()));
    }
    if superblock.incompatible_features & 0x100 != 0 {
        return Err(ApfsError::Unsupported("APFS Fusion drive".into()));
    }
    if superblock.incompatible_features & !2 != 0 {
        return Err(ApfsError::Unsupported(format!(
            "container incompatible features {:#x}",
            superblock.incompatible_features
        )));
    }
    let length = reader.seek(SeekFrom::End(0))?;
    if superblock.block_count == 0
        || superblock
            .block_count
            .checked_mul(u64::from(size))
            .is_none_or(|bytes| bytes > length)
        || superblock.max_file_systems as usize > NX_MAX_FILE_SYSTEMS
    {
        return Err(ApfsError::CorruptedData(
            "invalid or truncated APFS container geometry".into(),
        ));
    }
    Ok(superblock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nxsb_invalid_magic() {
        // Build a block that has wrong NXSB magic at offset 32
        let mut block = vec![0u8; 4096];
        // ObjectHeader: checksum [0..8], oid [8..16], xid [16..24], type [24..28], subtype [28..32]
        block[24..28].copy_from_slice(&0x01u32.to_le_bytes()); // type = NX_SUPERBLOCK
        // Wrong magic at offset 32
        block[32..36].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());

        let result = NxSuperblock::parse(&block);
        assert!(matches!(result, Err(ApfsError::InvalidMagic(0xDEADBEEF))));
    }
}
