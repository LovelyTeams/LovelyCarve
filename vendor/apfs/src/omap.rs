use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek};

use crate::btree;
use crate::error::{ApfsError, Result};
use crate::object;

/// OMAP key: (oid: u64, xid: u64) — 16 bytes, fixed-size.
/// OMAP value: (flags: u32, size: u32, paddr: u64) — 16 bytes, fixed-size.
const OMAP_KEY_SIZE: u32 = 16;
const OMAP_VAL_SIZE: u32 = 16;

/// Read the OMAP structure at a given physical block and return the
/// physical block number of the OMAP B-tree root.
pub fn read_omap_tree_root<R: Read + Seek>(
    reader: &mut R,
    omap_block: u64,
    block_size: u32,
) -> Result<u64> {
    let (_, block_data) = object::read_object(reader, omap_block, block_size)?;

    // omap_phys_t layout after obj_phys_t (32 bytes):
    //   om_flags: u32 (4)
    //   om_snap_count: u32 (4)
    //   om_tree_type: u32 (4)
    //   om_snapshot_tree_type: u32 (4)
    //   om_tree_oid: u64 (8)  <- B-tree root physical block
    let mut cursor = Cursor::new(&block_data[object::ObjectHeader::SIZE..]);
    let _om_flags = cursor.read_u32::<LittleEndian>()?;
    let _om_snap_count = cursor.read_u32::<LittleEndian>()?;
    let _om_tree_type = cursor.read_u32::<LittleEndian>()?;
    let _om_snap_tree_type = cursor.read_u32::<LittleEndian>()?;
    let om_tree_oid = cursor.read_u64::<LittleEndian>()?;

    Ok(om_tree_oid)
}

/// A physical OMAP tree viewed at a committed transaction.
#[derive(Debug, Clone, Copy)]
pub struct OmapTree {
    pub root: u64,
    pub xid: u64,
}
impl From<u64> for OmapTree {
    fn from(root: u64) -> Self {
        Self {
            root,
            xid: u64::MAX,
        }
    }
}
fn key_fields(key: &[u8]) -> Result<(u64, u64)> {
    if key.len() != 16 {
        return Err(ApfsError::InvalidBTree("OMAP key must be 16 bytes".into()));
    }
    let mut cursor = Cursor::new(key);
    Ok((
        cursor.read_u64::<LittleEndian>()?,
        cursor.read_u64::<LittleEndian>()?,
    ))
}
fn mapping(value: &[u8], block_size: u32) -> Result<Option<u64>> {
    if value.len() != 16 {
        return Err(ApfsError::InvalidBTree(
            "OMAP value must be 16 bytes".into(),
        ));
    }
    let mut cursor = Cursor::new(value);
    let flags = cursor.read_u32::<LittleEndian>()?;
    let size = cursor.read_u32::<LittleEndian>()?;
    let address = cursor.read_u64::<LittleEndian>()?;
    if flags & 1 != 0 {
        return Ok(None);
    }
    if flags & 4 != 0 {
        return Err(ApfsError::Unsupported("encrypted OMAP object".into()));
    }
    if flags & !0x1f != 0 || flags & 0x18 != 0 {
        return Err(ApfsError::Unsupported(format!("OMAP flags {flags:#x}")));
    }
    if address == 0 || size != block_size {
        return Err(ApfsError::CorruptedData(format!(
            "invalid OMAP address {address} or size {size}"
        )));
    }
    Ok(Some(address))
}
/// Resolve the last (OID, XID) mapping at or before the selected transaction.
pub fn omap_lookup<R: Read + Seek>(
    reader: &mut R,
    tree: impl Into<OmapTree>,
    block_size: u32,
    target_oid: u64,
) -> Result<u64> {
    let tree = tree.into();
    let entry = btree::btree_predecessor(
        reader,
        tree.root,
        block_size,
        OMAP_KEY_SIZE,
        OMAP_VAL_SIZE,
        |key| Ok(key_fields(key)?.cmp(&(target_oid, tree.xid))),
    )?;
    if let Some((key, value)) = entry
        && key_fields(&key)?.0 == target_oid
        && let Some(address) = mapping(&value, block_size)?
    {
        return Ok(address);
    }
    Err(ApfsError::CorruptedData(format!(
        "OMAP: OID {target_oid} absent at XID {}",
        tree.xid
    )))
}
/// Cache the same committed mappings used by random lookups, including deletions.
pub fn load_omap<R: Read + Seek>(
    reader: &mut R,
    tree: impl Into<OmapTree>,
    block_size: u32,
) -> Result<std::collections::HashMap<u64, u64>> {
    let tree = tree.into();
    let mut entries: std::collections::HashMap<u64, (u64, Vec<u8>)> =
        std::collections::HashMap::new();
    let mut failure = None;
    btree::btree_walk_leaves(reader, tree.root, block_size, None, &mut |key, value| {
        let (oid, xid) = match key_fields(key) {
            Ok(fields) => fields,
            Err(error) => {
                failure = Some(error);
                return false;
            }
        };
        if xid <= tree.xid
            && entries
                .get(&oid)
                .is_none_or(|(previous, _)| xid > *previous)
        {
            entries.insert(oid, (xid, value.to_vec()));
        }
        true
    })?;
    if let Some(error) = failure {
        return Err(error);
    }
    let mut map = std::collections::HashMap::new();
    for (oid, (_, value)) in entries {
        if let Some(address) = mapping(&value, block_size)? {
            map.insert(oid, address);
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::superblock;
    use std::io::BufReader;

    /// Requires ../tests/appfs.raw fixture. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn test_omap_lookup() {
        let file = std::fs::File::open("../tests/appfs.raw").unwrap();
        let mut reader = BufReader::new(file);

        let nxsb = superblock::read_nxsb(&mut reader).unwrap();
        let latest = superblock::find_latest_nxsb(&mut reader, &nxsb).unwrap();

        let omap_root =
            read_omap_tree_root(&mut reader, latest.omap_oid, latest.block_size).unwrap();

        let vol_oid = latest.fs_oids.iter().find(|&&o| o != 0).copied().unwrap();

        let vol_block = omap_lookup(&mut reader, omap_root, latest.block_size, vol_oid).unwrap();
        assert!(
            vol_block > 0 && vol_block < latest.block_count,
            "Physical block {} should be within container",
            vol_block
        );

        let vol_data = object::read_block(&mut reader, vol_block, latest.block_size).unwrap();
        let vol_sb = superblock::ApfsSuperblock::parse(&vol_data).unwrap();
        assert_eq!(vol_sb.magic, superblock::APSB_MAGIC);
    }
}
