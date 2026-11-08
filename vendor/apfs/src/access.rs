//! Metadata and seekable access share the same inode and extent interpretation.
use crate::{
    ApfsError, ApfsVolume, EntryKind, FileStat, ReadSeek, Result, catalog, extents, superblock,
};
use std::io::{Cursor, Read, Seek, SeekFrom};

/// Identity and geometry of the selected committed container checkpoint.
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub uuid: [u8; 16],
    pub transaction_id: u64,
    pub block_size: u32,
    pub block_count: u64,
    pub size: u64,
    pub features: u64,
    pub readonly_compatible_features: u64,
    pub incompatible_features: u64,
    pub volume_slots: Vec<(usize, u64)>,
}

/// Attribute identity, logical size and original source extents (if non-resident).
#[derive(Debug, Clone)]
pub struct XattrInfo {
    pub name: String,
    pub stream_id: Option<u64>,
    pub size: u64,
    pub extents: Vec<catalog::FileExtentRecord>,
}

impl FileStat {
    /// Decode the legacy 8:8 or Darwin 8:24 device identifier layout.
    /// Only character/block devices have device numbers; other inodes return None.
    pub fn device_numbers(&self) -> Option<(u32, u32)> {
        if !matches!(self.mode & 0o170000, 0o020000 | 0o060000) {
            return None;
        }
        self.device_number.map(|number| {
            if number & 0xffff0000 == 0 {
                ((number >> 8) & 0xff, number & 0xff)
            } else {
                (number >> 24, number & 0xffffff)
            }
        })
    }
}

impl<R: Read + Seek> ApfsVolume<R> {
    /// Inspect a container without opening any filesystem, including locked volumes.
    pub fn container_info(mut reader: R) -> Result<ContainerInfo> {
        let initial = superblock::read_nxsb(&mut reader)?;
        let block = superblock::find_latest_nxsb(&mut reader, &initial)?;
        let size = block
            .block_count
            .checked_mul(u64::from(block.block_size))
            .ok_or_else(|| ApfsError::CorruptedData("container size overflow".into()))?;
        Ok(ContainerInfo {
            uuid: block.uuid,
            transaction_id: block.header.xid,
            block_size: block.block_size,
            block_count: block.block_count,
            size,
            features: block.features,
            readonly_compatible_features: block.readonly_compatible_features,
            incompatible_features: block.incompatible_features,
            volume_slots: block
                .fs_oids
                .into_iter()
                .enumerate()
                .filter(|(_, oid)| *oid != 0)
                .collect(),
        })
    }

    /// Inspect an inode by its catalog identifier; no guessed parent path is required.
    pub fn stat_by_id(&mut self, oid: u64) -> Result<FileStat> {
        let inode = catalog::lookup_inode(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            oid,
        )?;
        self.stat_inode(oid, inode)
    }

    /// Open a logical file by its inode identifier (including clone private IDs).
    pub fn open_file_by_id(&mut self, oid: u64) -> Result<Box<dyn ReadSeek + '_>> {
        let inode = catalog::lookup_inode(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            oid,
        )?;
        self.open_inode(oid, inode)
    }

    pub(super) fn stat_inode(&mut self, oid: u64, inode: catalog::InodeVal) -> Result<FileStat> {
        let compression = if inode.kind() == catalog::INODE_FILE_TYPE {
            self.compression(oid)?
        } else {
            None
        };
        let size = if inode.kind() == catalog::INODE_SYMLINK_TYPE {
            self.symlink_target(oid)?
                .map_or(inode.size(), |target| target.len() as u64)
        } else {
            compression.map_or(inode.size(), |header| header.uncompressed_size)
        };
        Ok(FileStat {
            oid,
            parent_oid: inode.parent_id,
            private_id: inode.private_id,
            bsd_flags: inode.bsd_flags,
            device_number: inode.device_number,
            kind: match inode.kind() {
                catalog::INODE_DIR_TYPE => EntryKind::Directory,
                catalog::INODE_SYMLINK_TYPE => EntryKind::Symlink,
                _ => EntryKind::File,
            },
            size,
            create_time: inode.create_time,
            modify_time: inode.modify_time,
            change_time: inode.change_time,
            access_time: inode.access_time,
            uid: inode.uid,
            gid: inode.gid,
            mode: inode.mode,
            nlink: inode.nlink(),
            compression,
        })
    }

    pub(super) fn open_inode(
        &mut self,
        oid: u64,
        inode: catalog::InodeVal,
    ) -> Result<Box<dyn ReadSeek + '_>> {
        match inode.kind() {
            catalog::INODE_DIR_TYPE => {
                return Err(ApfsError::Unsupported(
                    "directory is not a file stream".into(),
                ));
            }
            catalog::INODE_FILE_TYPE => {
                if let Some(header) = self.compression(oid)? {
                    return Ok(Box::new(Cursor::new(self.read_compressed(oid, &header)?)));
                }
            }
            catalog::INODE_SYMLINK_TYPE => {
                if let Some(target) = self.symlink_target(oid)? {
                    return Ok(Box::new(Cursor::new(target)));
                }
            }
            _ => {
                return Err(ApfsError::Unsupported(
                    "special inode has no regular data stream".into(),
                ));
            }
        }
        let records = catalog::lookup_extents(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            inode.private_id,
        )?;
        Ok(Box::new(extents::ApfsForkReader::new(
            &mut self.reader,
            self.block_size,
            records,
            inode.size(),
        )?))
    }

    /// Read a logical range. Offset past EOF returns an empty buffer.
    pub fn read_file_range(&mut self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        let mut file = self.open_file(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut output = Vec::new();
        file.take(length as u64).read_to_end(&mut output)?;
        Ok(output)
    }

    /// Raw data-fork extents. Compressed/symlink logical bytes have another backing.
    pub fn file_extents(&mut self, path: &str) -> Result<Option<Vec<catalog::FileExtentRecord>>> {
        let (oid, inode) = catalog::resolve_path(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            path,
        )?;
        if inode.kind() != catalog::INODE_FILE_TYPE || self.compression(oid)?.is_some() {
            return Ok(None);
        }
        let records = catalog::lookup_extents(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            inode.private_id,
        )?;
        // Validate before advertising these ranges as authentic source locations.
        extents::ApfsForkReader::new(
            &mut self.reader,
            self.block_size,
            records.clone(),
            inode.size(),
        )?;
        Ok(Some(records))
    }

    /// Open an attribute independently of the file's transparent compression.
    /// Data-stream attributes seek directly, without loading the whole resource fork.
    pub fn open_xattr(&mut self, path: &str, name: &str) -> Result<Option<Box<dyn ReadSeek + '_>>> {
        let (oid, _) = catalog::resolve_path(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            path,
        )?;
        let value = catalog::lookup_xattr_value(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            oid,
            name,
        )?;
        match value {
            None => Ok(None),
            Some(catalog::XattrValue::Embedded(bytes)) => Ok(Some(Box::new(Cursor::new(bytes)))),
            Some(catalog::XattrValue::DataStream { obj_id, size }) => {
                let records = catalog::lookup_extents(
                    &mut self.reader,
                    self.catalog_root_block,
                    self.vol_omap_root_block,
                    self.block_size,
                    obj_id,
                )?;
                Ok(Some(Box::new(extents::ApfsForkReader::new(
                    &mut self.reader,
                    self.block_size,
                    records,
                    size,
                )?)))
            }
        }
    }

    /// Inspect an attribute without materializing its data-stream bytes.
    pub fn xattr_info(&mut self, path: &str, name: &str) -> Result<Option<XattrInfo>> {
        let (oid, _) = catalog::resolve_path(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            path,
        )?;
        let value = catalog::lookup_xattr_value(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            oid,
            name,
        )?;
        let (stream_id, size, records) = match value {
            None => return Ok(None),
            Some(catalog::XattrValue::Embedded(bytes)) => (None, bytes.len() as u64, Vec::new()),
            Some(catalog::XattrValue::DataStream { obj_id, size }) => {
                let records = catalog::lookup_extents(
                    &mut self.reader,
                    self.catalog_root_block,
                    self.vol_omap_root_block,
                    self.block_size,
                    obj_id,
                )?;
                extents::ApfsForkReader::new(
                    &mut self.reader,
                    self.block_size,
                    records.clone(),
                    size,
                )?;
                (Some(obj_id), size, records)
            }
        };
        Ok(Some(XattrInfo {
            name: name.to_owned(),
            stream_id,
            size,
            extents: records,
        }))
    }
}
