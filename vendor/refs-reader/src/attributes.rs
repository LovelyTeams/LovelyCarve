// File attribute lists can be inline, external, or indexed metadata trees.
use std::collections::HashSet;

use crate::bytes::u16_at;
use crate::metadata::{read_node, BlockRange};
use crate::table::{Row, TablePage};
use crate::{corrupt, BootSector, FileEntry, NodeReference, ReadAt, Result};

pub(crate) fn read_attributes(
    entry: &FileEntry,
    boot: &BootSector,
    mappings: &[BlockRange],
    source: &dyn ReadAt,
) -> Result<Vec<Row>> {
    if entry.entry_type != 1 {
        return Err(corrupt("short file entry has no attribute list"));
    }
    let standard_size = u16_at(&entry.value, 0)? as usize;
    let inline = TablePage::parse_allocation(&entry.value, standard_size, 0, boot)?;
    let mut pending = vec![inline];
    let mut visited = HashSet::new();
    let mut attributes = Vec::new();
    while let Some(page) = pending.pop() {
        let page_object_id = page.object_id;
        let indexed = page.index;
        for row in page.rows {
            if indexed || row.key.is_empty() {
                let reference = NodeReference::parse(&row.value, 0, boot.modern())?;
                if !visited.insert(reference.blocks[0]) {
                    return Err(corrupt("cycle or duplicate node in attribute list"));
                }
                let bytes = read_node(source, boot, mappings, reference).map_err(|error| {
                    corrupt(format!(
                        "attribute node {:#x}: {error}",
                        reference.blocks[0]
                    ))
                })?;
                let child = TablePage::parse(&bytes, reference.blocks[0], boot)?;
                if indexed && page_object_id != 0 && child.object_id != page_object_id {
                    return Err(corrupt("attribute index object identity mismatch"));
                }
                pending.push(child);
            } else {
                attributes.push(row);
            }
        }
    }
    Ok(attributes)
}

pub(crate) fn attribute_type(row: &Row, boot: &BootSector) -> Result<Option<u16>> {
    let offset = if (boot.major, boot.minor) >= (3, 5) {
        12
    } else {
        8
    };
    if row.key.len() < offset + 2 {
        return Ok(None);
    }
    Ok(Some(u16_at(&row.key, offset)?))
}
