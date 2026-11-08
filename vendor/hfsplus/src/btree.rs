use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek, SeekFrom};

use crate::error::{HfsPlusError, Result};
use crate::volume::ForkData;

/// B-tree node kinds
pub const NODE_KIND_LEAF: u8 = 0xFF; // -1 as i8
pub const NODE_KIND_INDEX: u8 = 0x00;
pub const NODE_KIND_HEADER: u8 = 0x01;
pub const NODE_KIND_MAP: u8 = 0x02;

/// B-tree header record (from the header node, record 0)
#[derive(Debug, Clone)]
pub struct BTreeHeaderRecord {
    pub tree_depth: u16,
    pub root_node: u32,
    pub leaf_records: u32,
    pub first_leaf_node: u32,
    pub last_leaf_node: u32,
    pub node_size: u16,
    pub max_key_length: u16,
    pub total_nodes: u32,
    pub free_nodes: u32,
    pub key_compare_type: u32,
    /// Fork data for computing byte offsets into the B-tree file
    pub fork: ForkData,
    /// Block size of the volume
    pub block_size: u32,
    /// Complete allocation map; populated through the extents overflow tree.
    pub extents: Vec<crate::volume::ExtentDescriptor>,
}

/// A B-tree node descriptor (14 bytes at the start of each node)
#[derive(Debug, Clone)]
pub struct NodeDescriptor {
    pub forward_link: u32,
    pub backward_link: u32,
    pub kind: u8,
    pub height: u8,
    pub num_records: u16,
    pub reserved: u16,
}

/// A parsed B-tree node with its raw data
#[derive(Debug)]
pub struct BTreeNode {
    pub descriptor: NodeDescriptor,
    /// Raw node data (node_size bytes)
    pub data: Vec<u8>,
    /// Record offsets (from the offset table at the end of the node)
    pub record_offsets: Vec<u16>,
}

/// Read the B-tree header record from the first node of a fork
pub fn read_btree_header<R: Read + Seek>(
    reader: &mut R,
    fork: &ForkData,
    block_size: u32,
) -> Result<BTreeHeaderRecord> {
    // The header node is always node 0
    let node_data = read_raw_node(reader, fork, block_size, 0, 512)?;

    // First, we need to figure out the real node size from the header record.
    // The node descriptor is 14 bytes, then record 0 is the header record.
    let mut cursor = Cursor::new(&node_data);

    // Parse node descriptor
    let desc = parse_node_descriptor(&mut cursor)?;
    if desc.kind != NODE_KIND_HEADER {
        return Err(HfsPlusError::InvalidBTree(format!(
            "expected header node, got kind {}",
            desc.kind
        )));
    }

    // Header record starts at offset 14
    let tree_depth = cursor.read_u16::<BigEndian>()?;
    let root_node = cursor.read_u32::<BigEndian>()?;
    let leaf_records = cursor.read_u32::<BigEndian>()?;
    let first_leaf_node = cursor.read_u32::<BigEndian>()?;
    let last_leaf_node = cursor.read_u32::<BigEndian>()?;
    let node_size = cursor.read_u16::<BigEndian>()?;
    let max_key_length = cursor.read_u16::<BigEndian>()?;
    let total_nodes = cursor.read_u32::<BigEndian>()?;
    let free_nodes = cursor.read_u32::<BigEndian>()?;
    let _reserved = cursor.read_u16::<BigEndian>()?;
    let _clump_size = cursor.read_u32::<BigEndian>()?;
    let _btree_type = cursor.read_u8()?;
    let key_compare_type = cursor.read_u8()? as u32;
    let _attributes = cursor.read_u32::<BigEndian>()?;
    // Skip reserved bytes (16 * 4 = 64 bytes)

    if !node_size.is_power_of_two()
        || node_size < 512
        || total_nodes == 0
        || u64::from(total_nodes) * u64::from(node_size) > fork.logical_size
        || free_nodes > total_nodes
        || root_node >= total_nodes
        || first_leaf_node >= total_nodes
        || last_leaf_node >= total_nodes
    {
        return Err(HfsPlusError::InvalidBTree("invalid header geometry".into()));
    }
    Ok(BTreeHeaderRecord {
        tree_depth,
        root_node,
        leaf_records,
        first_leaf_node,
        last_leaf_node,
        node_size,
        max_key_length,
        total_nodes,
        free_nodes,
        key_compare_type,
        fork: fork.clone(),
        block_size,
        extents: fork.extents.to_vec(),
    })
}

/// Read raw bytes for a node. We read `read_size` bytes at the node's offset.
/// If `read_size` is less than the actual node size, we read what we can
/// (used for initial header read where we don't know node_size yet).
fn read_raw_node<R: Read + Seek>(
    reader: &mut R,
    fork: &ForkData,
    block_size: u32,
    node_number: u32,
    read_size: u16,
) -> Result<Vec<u8>> {
    let mut fork_reader =
        crate::extents::ForkReader::from_extents(reader, fork, block_size, &fork.extents)?;
    fork_reader.seek(SeekFrom::Start(
        u64::from(node_number) * u64::from(read_size),
    ))?;
    let mut buf = vec![0u8; read_size as usize];
    fork_reader.read_exact(&mut buf)?;
    Ok(buf)
}

/// Read and parse a B-tree node
pub fn read_node<R: Read + Seek>(
    reader: &mut R,
    btree_header: &BTreeHeaderRecord,
    node_number: u32,
) -> Result<BTreeNode> {
    if node_number >= btree_header.total_nodes {
        return Err(HfsPlusError::InvalidBTree(
            "node number outside tree".into(),
        ));
    }
    let node_size = btree_header.node_size;
    let mut fork_reader = crate::extents::ForkReader::from_extents(
        reader,
        &btree_header.fork,
        btree_header.block_size,
        &btree_header.extents,
    )?;
    fork_reader.seek(SeekFrom::Start(
        u64::from(node_number) * u64::from(node_size),
    ))?;
    let mut data = vec![0u8; node_size as usize];
    fork_reader.read_exact(&mut data)?;

    // Parse the node descriptor
    let mut cursor = Cursor::new(&data);
    let descriptor = parse_node_descriptor(&mut cursor)?;

    // Read the record offset table from the end of the node
    // Offsets are stored as u16 values at the very end, growing backwards
    let num_offsets = descriptor.num_records as usize + 1; // +1 for the free space offset
    let mut record_offsets = Vec::with_capacity(num_offsets);

    for i in 0..num_offsets {
        let slot = (i + 1)
            .checked_mul(2)
            .ok_or_else(|| HfsPlusError::CorruptedData("offset table index overflow".into()))?;
        let offset_pos = (node_size as usize)
            .checked_sub(slot)
            .ok_or_else(|| HfsPlusError::CorruptedData("offset table out of bounds".into()))?;
        let pair = data
            .get(
                offset_pos..offset_pos.checked_add(2).ok_or_else(|| {
                    HfsPlusError::CorruptedData("offset table out of bounds".into())
                })?,
            )
            .ok_or_else(|| HfsPlusError::InvalidBTree("offset table out of bounds".into()))?;
        let offset = u16::from_be_bytes([pair[0], pair[1]]);
        record_offsets.push(offset);
    }

    let table_start = usize::from(node_size)
        .checked_sub(num_offsets * 2)
        .ok_or_else(|| HfsPlusError::InvalidBTree("offset table exceeds node".into()))?;
    if record_offsets
        .iter()
        .any(|offset| usize::from(*offset) < 14 || usize::from(*offset) > table_start)
        || record_offsets.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(HfsPlusError::InvalidBTree(
            "invalid record offset table".into(),
        ));
    }
    Ok(BTreeNode {
        descriptor,
        data,
        record_offsets,
    })
}

impl BTreeNode {
    /// Get the raw bytes for record `index` in this node
    pub fn record_data(&self, index: usize) -> Result<&[u8]> {
        if index >= self.descriptor.num_records as usize {
            return Err(HfsPlusError::InvalidBTree(format!(
                "record index {} >= num_records {}",
                index, self.descriptor.num_records
            )));
        }
        let start = *self.record_offsets.get(index).ok_or_else(|| {
            HfsPlusError::InvalidBTree(format!("record offset index {} out of bounds", index))
        })? as usize;
        let end = *self.record_offsets.get(index + 1).ok_or_else(|| {
            HfsPlusError::InvalidBTree(format!("record offset index {} out of bounds", index + 1))
        })? as usize;
        if start > end || end > self.data.len() {
            return Err(HfsPlusError::InvalidBTree(format!(
                "invalid record offsets: start={}, end={}, len={}",
                start,
                end,
                self.data.len()
            )));
        }
        self.data.get(start..end).ok_or_else(|| {
            HfsPlusError::InvalidBTree(format!(
                "record data out of bounds: start={}, end={}, len={}",
                start,
                end,
                self.data.len()
            ))
        })
    }
}

fn parse_node_descriptor<R: Read>(reader: &mut R) -> Result<NodeDescriptor> {
    Ok(NodeDescriptor {
        forward_link: reader.read_u32::<BigEndian>()?,
        backward_link: reader.read_u32::<BigEndian>()?,
        kind: reader.read_u8()?,
        height: reader.read_u8()?,
        num_records: reader.read_u16::<BigEndian>()?,
        reserved: reader.read_u16::<BigEndian>()?,
    })
}

/// Compute the absolute byte offset in the volume for a given byte offset within a fork.
/// Walks through the fork's extent descriptors to find the right allocation block.
pub fn compute_fork_offset(fork: &ForkData, block_size: u32, offset_in_fork: u64) -> Result<u64> {
    if block_size == 0 || offset_in_fork >= fork.logical_size {
        return Err(HfsPlusError::InvalidBTree(
            "fork offset outside logical size".into(),
        ));
    }
    let block_size = block_size as u64;
    let mut remaining = offset_in_fork;

    for extent in &fork.extents {
        if extent.block_count == 0 {
            break;
        }
        let extent_bytes = extent.block_count as u64 * block_size;
        if remaining < extent_bytes {
            let block_within_extent = remaining / block_size;
            let offset_within_block = remaining % block_size;
            let absolute_block = extent.start_block as u64 + block_within_extent;
            return Ok(absolute_block * block_size + offset_within_block);
        }
        remaining -= extent_bytes;
    }

    Err(HfsPlusError::InvalidBTree(format!(
        "fork offset {} exceeds extent capacity",
        offset_in_fork
    )))
}

/// Search a B-tree for a key. Returns the leaf node and record index where the key
/// was found, or where it would be inserted.
///
/// `compare_key` takes raw record bytes and returns Ordering relative to the search key:
/// - Less means the record key is less than the search key
/// - Greater means the record key is greater than the search key
/// - Equal means exact match
///
/// A comparator that cannot decode a key must return `Err` rather than an
/// ordering. Any ordering it guessed would steer the descent or end the leaf
/// scan on a damaged key, reporting a record that exists as absent — a false
/// negative indistinguishable from a genuine miss.
pub fn search_btree<R, F>(
    reader: &mut R,
    btree_header: &BTreeHeaderRecord,
    compare_key: &F,
) -> Result<Option<(BTreeNode, usize)>>
where
    R: Read + Seek,
    F: Fn(&[u8]) -> Result<std::cmp::Ordering>,
{
    if btree_header.root_node == 0 {
        return Ok(None);
    }

    let mut current_node_num = btree_header.root_node;

    let mut visited = std::collections::HashSet::new();
    loop {
        if !visited.insert(current_node_num)
            || visited.len() > usize::from(btree_header.tree_depth).max(1)
        {
            return Err(HfsPlusError::InvalidBTree(
                "cyclic index chain or excessive tree depth".into(),
            ));
        }
        let node = read_node(reader, btree_header, current_node_num)?;

        match node.descriptor.kind {
            NODE_KIND_LEAF => {
                // Search through leaf records for an exact match
                for i in 0..node.descriptor.num_records as usize {
                    let record_data = node.record_data(i)?;
                    match compare_key(record_data)? {
                        std::cmp::Ordering::Equal => return Ok(Some((node, i))),
                        std::cmp::Ordering::Greater => return Ok(None),
                        std::cmp::Ordering::Less => continue,
                    }
                }
                return Ok(None);
            }
            NODE_KIND_INDEX => {
                // In index nodes, find the last record whose key is <= search key.
                // Each record's data (after the key) is a u32 child node number.
                let mut child_node = 0u32;
                let mut found = false;

                for i in 0..node.descriptor.num_records as usize {
                    let record_data = node.record_data(i)?;
                    match compare_key(record_data)? {
                        std::cmp::Ordering::Less | std::cmp::Ordering::Equal => {
                            // Extract child node pointer from the end of the record
                            child_node = extract_index_child(record_data)?;
                            found = true;
                        }
                        std::cmp::Ordering::Greater => {
                            break;
                        }
                    }
                }

                if !found {
                    return Ok(None);
                }

                current_node_num = child_node;
            }
            other => {
                return Err(HfsPlusError::InvalidBTree(format!(
                    "unexpected node kind {} during search",
                    other
                )));
            }
        }
    }
}

/// Scan leaf nodes to find all records matching a predicate.
/// Starts from the first leaf and scans forward.
/// `match_fn` returns:
/// - Some(true) to include this record
/// - Some(false) to skip
/// - None to stop scanning (all subsequent records will also not match)
pub fn scan_leaves<R, F, T, P>(
    reader: &mut R,
    btree_header: &BTreeHeaderRecord,
    start_node: u32,
    match_fn: &F,
    parse_fn: &P,
) -> Result<Vec<T>>
where
    R: Read + Seek,
    F: Fn(&[u8]) -> Result<Option<bool>>,
    P: Fn(&[u8]) -> Result<T>,
{
    let mut results = Vec::new();
    let mut current_node_num = start_node;

    let mut visited = std::collections::HashSet::new();
    while current_node_num != 0 {
        if !visited.insert(current_node_num) {
            return Err(HfsPlusError::InvalidBTree("cyclic leaf chain".into()));
        }
        let node = read_node(reader, btree_header, current_node_num)?;

        if node.descriptor.kind != NODE_KIND_LEAF {
            return Err(HfsPlusError::InvalidBTree(format!(
                "expected leaf node, got kind {}",
                node.descriptor.kind
            )));
        }

        for i in 0..node.descriptor.num_records as usize {
            let record_data = node.record_data(i)?;
            match match_fn(record_data)? {
                Some(true) => {
                    results.push(parse_fn(record_data)?);
                }
                Some(false) => continue,
                None => return Ok(results), // stop scanning
            }
        }

        current_node_num = node.descriptor.forward_link;
    }

    Ok(results)
}

/// Extract the child node number from an index node record (public alias).
pub fn extract_index_child_pub(record_data: &[u8]) -> Result<u32> {
    extract_index_child(record_data)
}

/// Extract the child node number from an index node record.
/// In HFS+ B-trees, the key is followed by a u32 node number.
fn extract_index_child(record_data: &[u8]) -> Result<u32> {
    // Record format: [key_length: u16] [key_data: key_length bytes] [child_node: u32]
    if record_data.len() < 2 {
        return Err(HfsPlusError::InvalidBTree("index record too short".into()));
    }
    let key_length = u16::from_be_bytes([record_data[0], record_data[1]]) as usize;
    let child_offset = 2 + key_length;
    if child_offset + 4 > record_data.len() {
        return Err(HfsPlusError::InvalidBTree(format!(
            "index record too short for child pointer: key_len={}, record_len={}",
            key_length,
            record_data.len()
        )));
    }
    Ok(u32::from_be_bytes([
        record_data[child_offset],
        record_data[child_offset + 1],
        record_data[child_offset + 2],
        record_data[child_offset + 3],
    ]))
}

#[cfg(test)]
mod tests {}
