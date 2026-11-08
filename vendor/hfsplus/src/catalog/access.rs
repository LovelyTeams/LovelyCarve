//! CNID access preserves entry identity and resolves HFS backing hardlink records.
use super::{CatalogFile, CatalogRecord};
use crate::{HfsPlusError, Result, VolumeHeader, btree::BTreeHeaderRecord};
use std::io::{Read, Seek};

/// Find the raw catalog entry, its name and parent by CNID.
/// Thread records give the indexed route; missing threads fall back to a bounded leaf scan.
pub fn entry_by_id<R: Read + Seek>(
    reader: &mut R,
    volume: &VolumeHeader,
    tree: &BTreeHeaderRecord,
    identifier: u32,
) -> Result<(CatalogRecord, String, u32)> {
    if let Some(CatalogRecord::FolderThread(thread) | CatalogRecord::FileThread(thread)) =
        super::lookup_catalog(reader, volume, tree, identifier, "")?
    {
        if let Some(record) = super::lookup_catalog_utf16(
            reader,
            volume,
            tree,
            thread.parent_id,
            &thread.node_name.encode_utf16().collect::<Vec<_>>(),
        )? {
            let actual = match &record {
                CatalogRecord::File(file) => file.file_id,
                CatalogRecord::Folder(folder) => folder.folder_id,
                CatalogRecord::FileThread(_) | CatalogRecord::FolderThread(_) => 0,
            };
            if actual != identifier {
                return Err(HfsPlusError::CorruptedData(
                    "catalog thread identity mismatch".into(),
                ));
            }
            return Ok((record, thread.node_name, thread.parent_id));
        }
        return Err(HfsPlusError::CorruptedData(
            "catalog thread target missing".into(),
        ));
    }
    let predicate = |bytes: &[u8]| -> Result<Option<bool>> {
        let (_, offset) = super::parse_catalog_key(bytes)?;
        let record = super::parse_catalog_record(
            bytes
                .get(offset..)
                .ok_or_else(|| HfsPlusError::InvalidBTree("missing catalog value".into()))?,
        )?;
        Ok(Some(match record {
            CatalogRecord::File(file) => file.file_id == identifier,
            CatalogRecord::Folder(folder) => folder.folder_id == identifier,
            CatalogRecord::FileThread(_) | CatalogRecord::FolderThread(_) => false,
        }))
    };
    let parse = |bytes: &[u8]| {
        let (key, offset) = super::parse_catalog_key(bytes)?;
        let name = String::from_utf16(&key.node_name)
            .map_err(|error| HfsPlusError::CorruptedData(format!("invalid name: {error}")))?;
        Ok((
            super::parse_catalog_record(&bytes[offset..])?,
            name,
            key.parent_id,
        ))
    };
    let mut records =
        crate::btree::scan_leaves(reader, tree, tree.first_leaf_node, &predicate, &parse)?;
    if records.len() > 1 {
        return Err(HfsPlusError::CorruptedData("duplicate catalog CNID".into()));
    }
    records
        .pop()
        .ok_or_else(|| HfsPlusError::FileNotFound(format!("CNID {identifier}")))
}

/// Backing reference in an indirect regular-file or directory hardlink.
pub fn link_reference(file: &CatalogFile) -> Option<u32> {
    let signature = &file.finder_info[..8];
    ((signature == b"hlnkhfs+" || signature == b"fdrpMACS")
        && file.flags & 0x20 != 0
        && file.permissions.special > 2)
        .then_some(file.permissions.special)
}

/// Resolve a backing inode without replacing the caller's directory entry identity.
pub fn resolve_link<R: Read + Seek>(
    reader: &mut R,
    volume: &VolumeHeader,
    tree: &BTreeHeaderRecord,
    mut record: CatalogRecord,
) -> Result<CatalogRecord> {
    let mut visited = std::collections::HashSet::new();
    loop {
        let CatalogRecord::File(file) = &record else {
            return Ok(record);
        };
        let Some(reference) = link_reference(file) else {
            return Ok(record);
        };
        if !visited.insert(file.file_id) || visited.len() > 64 {
            return Err(HfsPlusError::CorruptedData(
                "hardlink cycle or resolution limit".into(),
            ));
        }
        let directory = &file.finder_info[..8] == b"fdrpMACS";
        let private_name = if directory {
            ".HFS+ Private Directory Data\r"
        } else {
            "\0\0\0\0HFS+ Private Data"
        };
        let Some(CatalogRecord::Folder(private)) =
            super::lookup_catalog(reader, volume, tree, 2, private_name)?
        else {
            return Err(HfsPlusError::CorruptedData(
                "hardlink private directory missing".into(),
            ));
        };
        let name = if directory {
            format!("dir_{reference}")
        } else {
            format!("iNode{reference}")
        };
        record = super::lookup_catalog(reader, volume, tree, private.folder_id, &name)?
            .ok_or_else(|| HfsPlusError::CorruptedData("hardlink backing inode missing".into()))?;
    }
}
