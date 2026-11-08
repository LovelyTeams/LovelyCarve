//! Fallible, bounded catalog traversal and GUID-based pointer pairing.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

use crate::bytes::le_u64;
use crate::catalog::{
    catalog_next_block_offset, is_catalog_block, parse_catalog_entry, CatalogEntry,
    StoreDescriptor, BLOCK_SIZE, CATALOG_BLOCK_HEADER_LEN, CATALOG_ENTRY_LEN, MAX_CATALOG_BLOCKS,
};
use crate::VssError;

pub(crate) fn walk_catalog<R: Read + Seek>(
    reader: &mut R,
    first: u64,
    volume_size: u64,
) -> Result<Vec<StoreDescriptor>, VssError> {
    let mut stores = Vec::new();
    let mut identities = HashSet::new();
    let mut pointers = HashMap::new();
    let mut visited = HashSet::new();
    let mut next = first;
    let mut block = vec![0; BLOCK_SIZE];
    while next != 0 {
        let corrupt = |reason| VssError::CorruptVolumeMetadata {
            offset: next,
            reason,
        };
        if visited.len() == MAX_CATALOG_BLOCKS {
            return Err(corrupt("catalog exceeds the block limit"));
        }
        if !visited.insert(next) {
            return Err(corrupt("catalog block is reached twice"));
        }
        if next
            .checked_add(BLOCK_SIZE as u64)
            .is_none_or(|end| end > volume_size)
        {
            return Err(corrupt("catalog block runs past the volume"));
        }
        reader.seek(SeekFrom::Start(next))?;
        reader.read_exact(&mut block)?;
        if !is_catalog_block(&block) {
            return Err(corrupt("not a version-1 VSS catalog block"));
        }
        for record in block[CATALOG_BLOCK_HEADER_LEN..]
            .as_chunks::<CATALOG_ENTRY_LEN>()
            .0
        {
            if le_u64(record, 0) > 3 {
                return Err(corrupt("unsupported catalog entry type"));
            }
            match parse_catalog_entry(record) {
                CatalogEntry::Snapshot(descriptor) => {
                    if !identities.insert(descriptor.store_id) {
                        return Err(corrupt("duplicate store identifier"));
                    }
                    stores.push(descriptor);
                }
                pointer @ CatalogEntry::StorePointer { store_id, .. } => {
                    if pointers.insert(store_id, pointer).is_some() {
                        return Err(corrupt("duplicate store pointer"));
                    }
                }
                CatalogEntry::Empty | CatalogEntry::Other => {}
            }
        }
        next = catalog_next_block_offset(&block);
    }
    // Pair after traversal: pointers can be separated from descriptors, even
    // across catalog blocks. Never attach an unmatched pointer to another GUID.
    for descriptor in &mut stores {
        if let Some(CatalogEntry::StorePointer {
            store_block_list_offset,
            store_header_offset,
            store_bitmap_offset,
            store_previous_bitmap_offset,
            store_block_range_list_offset,
            ntfs_file_reference,
            allocated_size,
            ..
        }) = pointers.remove(&descriptor.store_id)
        {
            descriptor.store_header_offset = Some(store_header_offset);
            descriptor.store_block_list_offset = Some(store_block_list_offset);
            descriptor.store_bitmap_offset = Some(store_bitmap_offset);
            descriptor.store_previous_bitmap_offset =
                (store_previous_bitmap_offset != 0).then_some(store_previous_bitmap_offset);
            descriptor.store_block_range_list_offset = Some(store_block_range_list_offset);
            descriptor.ntfs_file_reference = Some(ntfs_file_reference);
            descriptor.allocated_size = Some(allocated_size);
        }
    }
    if !pointers.is_empty() {
        return Err(VssError::CorruptVolumeMetadata {
            offset: first,
            reason: "store pointer has no matching snapshot descriptor",
        });
    }
    stores.sort_by_key(|descriptor| descriptor.creation_time);
    Ok(stores)
}
