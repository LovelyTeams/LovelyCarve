//! Snapshot metadata enumeration; this does not mount a historical catalog.
use crate::{ApfsError, ApfsVolume, Result, btree, catalog, object, omap};
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek};

/// Original snapshot identity and provenance from j_snap_metadata_val_t.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub transaction_id: u64,
    pub name: String,
    pub superblock_block: u64,
    pub extentref_tree_oid: u64,
    pub create_time: i64,
    pub change_time: i64,
    pub inode_number: u64,
    pub extentref_tree_type: u32,
    pub flags: u32,
}

impl SnapshotInfo {
    fn parse(transaction_id: u64, value: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(value);
        let extentref_tree_oid = cursor.read_u64::<LittleEndian>()?;
        let superblock_block = cursor.read_u64::<LittleEndian>()?;
        let create_time = cursor.read_i64::<LittleEndian>()?;
        let change_time = cursor.read_i64::<LittleEndian>()?;
        let inode_number = cursor.read_u64::<LittleEndian>()?;
        let extentref_tree_type = cursor.read_u32::<LittleEndian>()?;
        let flags = cursor.read_u32::<LittleEndian>()?;
        let length = usize::from(cursor.read_u16::<LittleEndian>()?);
        let name = value
            .get(50..50 + length)
            .and_then(|bytes| bytes.strip_suffix(&[0]))
            .ok_or_else(|| {
                ApfsError::CorruptedData("invalid snapshot name length or terminator".into())
            })?;
        if name.contains(&0) || name.is_empty() {
            return Err(ApfsError::CorruptedData("invalid snapshot name".into()));
        }
        let name = String::from_utf8(name.to_vec())
            .map_err(|error| ApfsError::CorruptedData(format!("snapshot name: {error}")))?;
        Ok(Self {
            transaction_id,
            name,
            superblock_block,
            extentref_tree_oid,
            create_time,
            change_time,
            inode_number,
            extentref_tree_type,
            flags,
        })
    }
}

impl<R: Read + Seek> ApfsVolume<R> {
    /// Enumerate the snapshot metadata tree, preserving snapshot XIDs and source blocks.
    /// Historical file browsing is intentionally a separate, unsupported operation.
    pub fn list_snapshots(&mut self) -> Result<Vec<SnapshotInfo>> {
        if self.snapshot_tree_oid == 0 {
            if self.info.num_snapshots != 0 {
                return Err(ApfsError::CorruptedData(
                    "snapshot count without metadata tree".into(),
                ));
            }
            return Ok(Vec::new());
        }
        let physical = match self.snapshot_tree_type & object::OBJ_STORAGE_TYPE_MASK {
            object::OBJ_PHYSICAL => true,
            object::OBJ_VIRTUAL => false,
            _ => {
                return Err(ApfsError::Unsupported(
                    "ephemeral snapshot metadata tree".into(),
                ));
            }
        };
        let map = if physical {
            None
        } else {
            Some(omap::load_omap(
                &mut self.reader,
                self.vol_omap_root_block.omap,
                self.block_size,
            )?)
        };
        let root = if physical {
            self.snapshot_tree_oid
        } else {
            omap::omap_lookup(
                &mut self.reader,
                self.vol_omap_root_block.omap,
                self.block_size,
                self.snapshot_tree_oid,
            )?
        };
        let mut snapshots = Vec::new();
        let mut failure = None;
        btree::btree_walk_leaves(
            &mut self.reader,
            root,
            self.block_size,
            map.as_ref(),
            &mut |key, value| {
                let parsed = catalog::decode_catalog_key(key).and_then(|(xid, kind)| {
                    if kind != catalog::J_TYPE_SNAP_METADATA {
                        return Ok(None);
                    }
                    if xid > self.info.transaction_id {
                        return Err(ApfsError::CorruptedData(
                            "snapshot newer than selected volume transaction".into(),
                        ));
                    }
                    SnapshotInfo::parse(xid, value).map(Some)
                });
                match parsed {
                    Ok(Some(snapshot)) => snapshots.push(snapshot),
                    Ok(None) => {}
                    Err(error) => {
                        failure = Some(error);
                        return false;
                    }
                }
                true
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        if snapshots.len() as u64 != self.info.num_snapshots {
            return Err(ApfsError::CorruptedData(
                "snapshot metadata count mismatch".into(),
            ));
        }
        Ok(snapshots)
    }
}
