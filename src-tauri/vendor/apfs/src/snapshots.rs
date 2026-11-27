//! Snapshot metadata enumeration and read-only transaction views.
use crate::{
    ApfsError, ApfsVolume, Result, btree, catalog, object, omap, superblock::ApfsSuperblock,
    validate_readable_volume, volume_info_from_superblock,
};
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
    /// Open a historical catalog using its own APSB and the volume object's
    /// mappings at the snapshot XID. The current view's reader is consumed.
    pub fn into_snapshot(mut self, xid: u64) -> Result<Self> {
        let mut matches = self
            .list_snapshots()?
            .into_iter()
            .filter(|snapshot| snapshot.transaction_id == xid);
        let snapshot = matches
            .next()
            .ok_or_else(|| ApfsError::FileNotFound(format!("APFS snapshot XID {xid}")))?;
        if matches.next().is_some() {
            return Err(ApfsError::CorruptedData(format!(
                "duplicate snapshot XID {xid}"
            )));
        }
        if snapshot.flags != 0 {
            return Err(ApfsError::Unsupported(format!(
                "snapshot XID {xid} has incomplete or unsupported state {:#x}",
                snapshot.flags
            )));
        }
        let (header, bytes) =
            object::read_object(&mut self.reader, snapshot.superblock_block, self.block_size)?;
        let volume = ApfsSuperblock::parse(&bytes)?;
        if header.object_type() != object::OBJECT_TYPE_FS
            || header.xid != xid
            || !header.is_physical()
            || header.oid != snapshot.superblock_block
            || volume.uuid != self.info.uuid
            || volume.fs_index as usize != self.info.index
            || volume.omap_oid != 0
            || volume
                .fs_alloc_count
                .checked_mul(u64::from(self.block_size))
                .is_none()
        {
            return Err(ApfsError::CorruptedData(format!(
                "snapshot APSB identity or geometry does not match XID {xid}"
            )));
        }
        validate_readable_volume(&volume)?;
        let historical_omap = omap::OmapTree {
            xid,
            ..self.vol_omap_root_block.omap
        };
        let root = omap::lookup_mapping(
            &mut self.reader,
            historical_omap,
            self.block_size,
            volume.root_tree_oid,
        )?;
        let encrypted = volume.fs_flags & crate::crypto::FS_UNENCRYPTED == 0;
        if encrypted != root.encrypted {
            return Err(ApfsError::CorruptedData(
                "snapshot catalog encryption flags disagree".into(),
            ));
        }
        if !encrypted {
            self.key = None;
        }
        let (catalog_header, _) = object::read_mapped_object(
            &mut self.reader,
            root,
            self.block_size,
            self.key.as_ref().map(crate::ApfsVolumeKey::cipher),
        )?;
        if catalog_header.object_type() != object::OBJECT_TYPE_BTREE
            || catalog_header.oid != volume.root_tree_oid
            || catalog_header.xid > xid
        {
            return Err(ApfsError::CorruptedData(format!(
                "snapshot catalog root does not match XID {xid}"
            )));
        }
        self.vol_omap_root_block = catalog::CatalogContext {
            omap: historical_omap,
            incompatible_features: volume.incompatible_features,
            cipher: None,
        };
        self.catalog_root_block = root;
        self.snapshot_tree_oid = 0;
        self.snapshot_tree_type = 0;
        self.info =
            volume_info_from_superblock(self.info.index, self.info.oid, self.block_size, &volume);
        self.info.num_snapshots = 0;
        Ok(self)
    }

    /// Enumerate the snapshot metadata tree, preserving snapshot XIDs and source blocks.
    pub fn list_snapshots(&mut self) -> Result<Vec<SnapshotInfo>> {
        if self.info.num_snapshots == 0 {
            return Ok(Vec::new());
        }
        if self.snapshot_tree_oid == 0 {
            return Err(ApfsError::CorruptedData(
                "snapshot count without metadata tree".into(),
            ));
        }
        let physical = match self.snapshot_tree_type & object::OBJ_STORAGE_TYPE_MASK {
            object::OBJ_PHYSICAL => true,
            // macOS stores a physical snapshot tree with no separate type override.
            _ if self.snapshot_tree_type == 0 => true,
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
            Some(omap::load_mappings(
                &mut self.reader,
                self.vol_omap_root_block.omap,
                self.block_size,
            )?)
        };
        let root = if physical {
            self.snapshot_tree_oid.into()
        } else {
            omap::lookup_mapping(
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
            self.key.as_ref().map(crate::ApfsVolumeKey::cipher),
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
