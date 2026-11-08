//! HFS+ image synthesizer for testing.
//!
//! Generates minimal valid HFSX filesystem images in memory, allowing tests
//! to exercise the full parser without external fixture files.

use crate::catalog::{CNID_ROOT_FOLDER, CNID_ROOT_PARENT};

const BLOCK_SIZE: usize = 4096;
const HFSX_SIGNATURE: u16 = 0x4858;
const HFSX_VERSION: u16 = 5;
const CNID_FIRST_USER: u32 = 16;

/// First block available for file data: blocks 0-5 hold the volume header and
/// the extents, catalog and attributes B-trees.
const FIRST_DATA_BLOCK: u32 = 6;

// Catalog record types
const RECORD_TYPE_FOLDER: u16 = 0x0001;
const RECORD_TYPE_FILE: u16 = 0x0002;
const RECORD_TYPE_FOLDER_THREAD: u16 = 0x0003;
const RECORD_TYPE_FILE_THREAD: u16 = 0x0004;

// B-tree node kinds
const NODE_KIND_HEADER: u8 = 0x01;
const NODE_KIND_LEAF: u8 = 0xFF;

struct FileEntry {
    name: String,
    content: Vec<u8>,
    mode: u16,
    cnid: u32,
    /// Overrides the fork's declared `logical_size`. `None` means "state the
    /// truth", which is what a real volume does.
    declared_size: Option<u64>,
    /// Resource fork contents, empty for a file without one.
    resource_fork: Vec<u8>,
    /// Extended attributes, written as inline Attributes B-tree records.
    xattrs: Vec<(String, Vec<u8>)>,
}

/// Builds a minimal valid HFSX filesystem image in memory.
///
/// The generated image uses 4096-byte blocks with the following layout:
/// - Block 0: Volume header (at byte offset 1024)
/// - Block 1: Extents overflow B-tree (header node, empty tree)
/// - Block 2: Catalog B-tree header node
/// - Block 3: Catalog B-tree leaf node (all catalog records)
/// - Block 4: Attributes B-tree header node
/// - Block 5: Attributes B-tree leaf node (all attribute records)
/// - Block 6+: File data blocks, then resource fork blocks
pub struct HfsPlusImageBuilder {
    volume_name: String,
    files: Vec<FileEntry>,
    next_cnid: u32,
}

impl Default for HfsPlusImageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HfsPlusImageBuilder {
    /// Create a new builder with volume name "TestVolume".
    pub fn new() -> Self {
        Self {
            volume_name: "TestVolume".to_string(),
            files: Vec::new(),
            next_cnid: CNID_FIRST_USER,
        }
    }

    /// Add a file to the root directory. `mode` is the Unix permission bits
    /// (e.g., 0o644). The S_IFREG bit is added automatically.
    pub fn add_file(&mut self, name: &str, content: &[u8], mode: u16) -> &mut Self {
        let cnid = self.next_cnid;
        self.next_cnid += 1;
        self.files.push(FileEntry {
            name: name.to_string(),
            content: content.to_vec(),
            mode,
            cnid,
            declared_size: None,
            resource_fork: Vec::new(),
            xattrs: Vec::new(),
        });
        self
    }

    /// Add a file carrying extended attributes and, optionally, a resource
    /// fork. Together these build a decmpfs-compressed file: the attribute
    /// holds the `com.apple.decmpfs` header, the fork holds the blocks.
    pub fn add_file_with_xattrs(
        &mut self,
        name: &str,
        content: &[u8],
        mode: u16,
        xattrs: &[(&str, &[u8])],
        resource_fork: &[u8],
    ) -> &mut Self {
        self.add_file(name, content, mode);
        let entry = self
            .files
            .last_mut()
            .expect("add_file just pushed an entry");
        entry.resource_fork = resource_fork.to_vec();
        entry.xattrs = xattrs
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.to_vec()))
            .collect();
        self
    }

    /// Add a file whose fork claims to be `declared_size` bytes while only
    /// `content` is actually allocated, modelling a volume whose extent chain
    /// ends before the file does. Used to test that readers do not present a
    /// short read as a complete file.
    pub fn add_file_with_declared_size(
        &mut self,
        name: &str,
        content: &[u8],
        mode: u16,
        declared_size: u64,
    ) -> &mut Self {
        self.add_file(name, content, mode);
        self.files
            .last_mut()
            .expect("add_file just pushed an entry")
            .declared_size = Some(declared_size);
        self
    }

    /// Build the HFSX image and return the raw bytes.
    pub fn build(&self) -> Vec<u8> {
        // Calculate block allocation for file data and resource forks
        let mut data_block = FIRST_DATA_BLOCK;
        let mut file_blocks: Vec<(u32, u32)> = Vec::new();
        let mut rsrc_blocks: Vec<(u32, u32)> = Vec::new();

        let allocate = |content: &[u8], next: &mut u32| {
            let blocks = if content.is_empty() {
                0
            } else {
                content.len().div_ceil(BLOCK_SIZE) as u32
            };
            let start = *next;
            *next += blocks;
            (start, blocks)
        };

        for file in &self.files {
            file_blocks.push(allocate(&file.content, &mut data_block));
            rsrc_blocks.push(allocate(&file.resource_fork, &mut data_block));
        }

        let total_blocks = data_block;
        let mut image = vec![0u8; total_blocks as usize * BLOCK_SIZE];

        // Block 0: Volume header at offset 1024
        write_volume_header(
            &mut image,
            total_blocks,
            self.files.len() as u32,
            self.next_cnid,
        );

        // Block 1: Extents overflow B-tree header node (empty tree)
        write_btree_header_node(&mut image, BLOCK_SIZE, 0, 0, 10, 0);

        // Block 2: Catalog B-tree header node
        let num_catalog_records = 2 + self.files.len() * 2;
        write_btree_header_node(
            &mut image,
            2 * BLOCK_SIZE,
            1,
            num_catalog_records as u32,
            516,
            0xbc,
        );

        // Block 3: Catalog B-tree leaf node
        self.write_catalog_leaf(&mut image, &file_blocks, &rsrc_blocks);

        // Blocks 4 and 5: Attributes B-tree
        self.write_attributes_btree(&mut image);

        // Block 6+: File data, then resource forks
        for (i, file) in self.files.iter().enumerate() {
            for (content, (start_block, _)) in [
                (&file.content, file_blocks[i]),
                (&file.resource_fork, rsrc_blocks[i]),
            ] {
                if content.is_empty() {
                    continue;
                }
                let offset = start_block as usize * BLOCK_SIZE;
                image[offset..offset + content.len()].copy_from_slice(content);
            }
        }

        image
    }

    /// Write the Attributes B-tree header and its single leaf of inline
    /// records, sorted by `(fileID, name)` as `hfs_attrkeycompare` requires.
    fn write_attributes_btree(&self, image: &mut [u8]) {
        let mut records: Vec<(u32, Vec<u16>, Vec<u8>)> = Vec::new();
        for file in &self.files {
            for (name, value) in &file.xattrs {
                records.push((
                    file.cnid,
                    name.encode_utf16().collect(),
                    build_inline_attribute(file.cnid, name, value),
                ));
            }
        }
        records.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let count = records.len() as u32;
        // Node numbers are relative to the attributes fork, which starts at
        // block 4, so the leaf in block 5 is node 1.
        let root = if count == 0 { 0 } else { 1 };
        write_btree_header_node(image, 4 * BLOCK_SIZE, root, count, 266, 0);

        if count > 0 {
            let records: Vec<Vec<u8>> = records.into_iter().map(|(_, _, r)| r).collect();
            write_leaf_node(image, 5 * BLOCK_SIZE, &records);
        }
    }

    fn write_catalog_leaf(
        &self,
        image: &mut [u8],
        file_blocks: &[(u32, u32)],
        rsrc_blocks: &[(u32, u32)],
    ) {
        // Build all catalog records in sorted order: (parent_id, name)
        let mut records: Vec<Vec<u8>> = Vec::new();

        // 1. Root folder record: key=(1, volume_name)
        records.push(build_catalog_entry(
            CNID_ROOT_PARENT,
            &self.volume_name,
            &build_folder_record(CNID_ROOT_FOLDER, self.files.len() as u32, 0o040755),
        ));

        // 2. Root folder thread: key=(2, "")
        records.push(build_catalog_entry(
            CNID_ROOT_FOLDER,
            "",
            &build_thread_record(
                RECORD_TYPE_FOLDER_THREAD,
                CNID_ROOT_PARENT,
                &self.volume_name,
            ),
        ));

        // 3. File records: key=(2, name) sorted by name
        let mut sorted: Vec<(usize, &FileEntry)> = self.files.iter().enumerate().collect();
        sorted.sort_by(|a, b| a.1.name.cmp(&b.1.name));

        for &(orig_idx, file) in &sorted {
            let (start_block, block_count) = file_blocks[orig_idx];
            let (rsrc_start, rsrc_count) = rsrc_blocks[orig_idx];
            records.push(build_catalog_entry(
                CNID_ROOT_FOLDER,
                &file.name,
                &build_file_record(
                    file.cnid,
                    file.declared_size.unwrap_or(file.content.len() as u64),
                    start_block,
                    block_count,
                    0o100000 | file.mode,
                    file.resource_fork.len() as u64,
                    rsrc_start,
                    rsrc_count,
                ),
            ));
        }

        // 4. File thread records: key=(cnid, "") in CNID order
        for file in &self.files {
            records.push(build_catalog_entry(
                file.cnid,
                "",
                &build_thread_record(RECORD_TYPE_FILE_THREAD, CNID_ROOT_FOLDER, &file.name),
            ));
        }

        // Write the leaf node at block 3
        write_leaf_node(image, 3 * BLOCK_SIZE, &records);
    }
}

// ---------------------------------------------------------------------------
// Binary layout helpers
// ---------------------------------------------------------------------------

fn write_volume_header(image: &mut [u8], total_blocks: u32, file_count: u32, next_cnid: u32) {
    let mut buf = Vec::with_capacity(512);
    push_u16(&mut buf, HFSX_SIGNATURE);
    push_u16(&mut buf, HFSX_VERSION);
    push_u32(&mut buf, 0); // attributes
    push_u32(&mut buf, 0); // last_mounted_version
    push_u32(&mut buf, 0); // journal_info_block
    push_u32(&mut buf, 0); // create_date
    push_u32(&mut buf, 0); // modify_date
    push_u32(&mut buf, 0); // backup_date
    push_u32(&mut buf, 0); // checked_date
    push_u32(&mut buf, file_count);
    push_u32(&mut buf, 1); // folder_count (root only)
    push_u32(&mut buf, BLOCK_SIZE as u32);
    push_u32(&mut buf, total_blocks);
    push_u32(&mut buf, 0); // free_blocks
    push_u32(&mut buf, 0); // next_allocation
    push_u32(&mut buf, 0); // rsrc_clump_size
    push_u32(&mut buf, 0); // data_clump_size
    push_u32(&mut buf, next_cnid);
    push_u32(&mut buf, 0); // write_count
    push_u64(&mut buf, 0); // encoding_bitmap
    for _ in 0..8 {
        push_u32(&mut buf, 0); // finder_info
    }
    // allocation_file (empty)
    push_fork_data(&mut buf, 0, 0, 0, 0);
    // extents_file: 1 block at block 1
    push_fork_data(&mut buf, BLOCK_SIZE as u64, 1, 1, 1);
    // catalog_file: 2 blocks at blocks 2-3
    push_fork_data(&mut buf, 2 * BLOCK_SIZE as u64, 2, 2, 2);
    // attributes_file: 2 blocks at blocks 4-5
    push_fork_data(&mut buf, 2 * BLOCK_SIZE as u64, 2, 4, 2);
    // startup_file (empty)
    push_fork_data(&mut buf, 0, 0, 0, 0);

    buf.resize(512, 0);
    image[1024..1024 + 512].copy_from_slice(&buf);
}

fn write_btree_header_node(
    image: &mut [u8],
    offset: usize,
    root_node: u32,
    leaf_records: u32,
    max_key_length: u16,
    key_compare_type: u8,
) {
    let node = &mut image[offset..offset + BLOCK_SIZE];

    // Node descriptor (14 bytes)
    let mut hdr = Vec::with_capacity(64);
    push_u32(&mut hdr, 0); // forward_link
    push_u32(&mut hdr, 0); // backward_link
    hdr.push(NODE_KIND_HEADER);
    hdr.push(0); // height
    push_u16(&mut hdr, 1); // num_records
    push_u16(&mut hdr, 0); // reserved

    // Header record (starts at offset 14)
    let depth = if root_node > 0 { 1u16 } else { 0u16 };
    let total_nodes = if root_node > 0 { 2u32 } else { 1u32 };
    let first_leaf = root_node;
    let last_leaf = root_node;

    push_u16(&mut hdr, depth);
    push_u32(&mut hdr, root_node);
    push_u32(&mut hdr, leaf_records);
    push_u32(&mut hdr, first_leaf);
    push_u32(&mut hdr, last_leaf);
    push_u16(&mut hdr, BLOCK_SIZE as u16); // node_size
    push_u16(&mut hdr, max_key_length);
    push_u32(&mut hdr, total_nodes);
    push_u32(&mut hdr, 0); // free_nodes
    push_u16(&mut hdr, 0); // reserved
    push_u32(&mut hdr, 0); // clump_size
    hdr.push(0); // btree_type
    hdr.push(key_compare_type);
    push_u32(&mut hdr, 0); // attributes

    let record_end = hdr.len() as u16;
    node[..hdr.len()].copy_from_slice(&hdr);

    // Offset table: 2 entries for num_records=1
    let record_start: u16 = 14;
    write_u16_at(node, BLOCK_SIZE - 2, record_start); // record_offsets[0]
    write_u16_at(node, BLOCK_SIZE - 4, record_end); // record_offsets[1]
}

fn write_leaf_node(image: &mut [u8], offset: usize, records: &[Vec<u8>]) {
    let node = &mut image[offset..offset + BLOCK_SIZE];
    let num_records = records.len() as u16;

    // Node descriptor
    let mut desc = Vec::with_capacity(14);
    push_u32(&mut desc, 0); // forward_link
    push_u32(&mut desc, 0); // backward_link
    desc.push(NODE_KIND_LEAF);
    desc.push(1); // height (1 for leaf)
    push_u16(&mut desc, num_records);
    push_u16(&mut desc, 0); // reserved
    node[..14].copy_from_slice(&desc);

    // Write records and collect offsets
    let mut offsets: Vec<u16> = Vec::with_capacity(records.len() + 1);
    let mut pos: u16 = 14;

    for record in records {
        offsets.push(pos);
        let end = pos as usize + record.len();
        node[pos as usize..end].copy_from_slice(record);
        pos = end as u16;
    }
    offsets.push(pos); // free space offset

    // Write offset table at end of node (entry 0 at node_end-2, entry 1 at node_end-4, ...)
    for (i, &off) in offsets.iter().enumerate() {
        write_u16_at(node, BLOCK_SIZE - (i + 1) * 2, off);
    }
}

// ---------------------------------------------------------------------------
// Catalog record builders
// ---------------------------------------------------------------------------

fn build_catalog_entry(parent_id: u32, name: &str, record_data: &[u8]) -> Vec<u8> {
    let name_bytes = encode_utf16be(name);
    let name_len = name_bytes.len() / 2;
    let key_length = 4 + 2 + name_bytes.len(); // parent_id + name_length + name

    let mut buf = Vec::new();
    push_u16(&mut buf, key_length as u16);
    push_u32(&mut buf, parent_id);
    push_u16(&mut buf, name_len as u16);
    buf.extend_from_slice(&name_bytes);

    // Record data starts at offset 2 + key_length, aligned to even
    let mut record_offset = 2 + key_length;
    if !record_offset.is_multiple_of(2) {
        record_offset += 1;
    }
    buf.resize(record_offset, 0);
    buf.extend_from_slice(record_data);
    buf
}

fn build_folder_record(folder_id: u32, valence: u32, mode: u16) -> Vec<u8> {
    let mut buf = Vec::new();
    push_u16(&mut buf, RECORD_TYPE_FOLDER);
    push_u16(&mut buf, 0); // flags
    push_u32(&mut buf, valence);
    push_u32(&mut buf, folder_id);
    for _ in 0..5 {
        push_u32(&mut buf, 0); // dates
    }
    push_bsd_info(&mut buf, mode);
    buf.extend_from_slice(&[0u8; 32]); // user_info + finder_info
    push_u32(&mut buf, 0); // text_encoding
    buf
}

#[allow(clippy::too_many_arguments)]
fn build_file_record(
    file_id: u32,
    logical_size: u64,
    start_block: u32,
    block_count: u32,
    mode: u16,
    rsrc_size: u64,
    rsrc_start_block: u32,
    rsrc_block_count: u32,
) -> Vec<u8> {
    let mut buf = Vec::new();
    push_u16(&mut buf, RECORD_TYPE_FILE);
    push_u16(&mut buf, 0); // flags
    push_u32(&mut buf, 0); // reserved
    push_u32(&mut buf, file_id);
    for _ in 0..5 {
        push_u32(&mut buf, 0); // dates
    }
    push_bsd_info(&mut buf, mode);
    buf.extend_from_slice(&[0u8; 32]); // user_info + finder_info
    push_u32(&mut buf, 0); // text_encoding
    push_u32(&mut buf, 0); // reserved2
    push_fork_data(
        &mut buf,
        logical_size,
        block_count,
        start_block,
        block_count,
    );
    push_fork_data(
        &mut buf,
        rsrc_size,
        rsrc_block_count,
        rsrc_start_block,
        rsrc_block_count,
    );
    buf
}

/// Build a `kHFSPlusAttrInlineData` record: the attribute key followed by
/// `recordType u32 | reserved[2] u32 | attrSize u32 | attrData`.
fn build_inline_attribute(file_id: u32, name: &str, value: &[u8]) -> Vec<u8> {
    let name_bytes = encode_utf16be(name);
    // keyLength counts everything after itself: pad(2), fileID(4),
    // startBlock(4), attrNameLen(2), then the name — Apple's
    // `kHFSPlusAttrKeyMinimumLength` of 12, plus the name.
    let key_length = 12 + name_bytes.len();

    let mut buf = Vec::new();
    push_u16(&mut buf, key_length as u16);
    push_u16(&mut buf, 0); // pad
    push_u32(&mut buf, file_id);
    push_u32(&mut buf, 0); // start_block
    push_u16(&mut buf, (name_bytes.len() / 2) as u16);
    buf.extend_from_slice(&name_bytes);
    if !buf.len().is_multiple_of(2) {
        buf.push(0);
    }

    push_u32(&mut buf, 0x10); // kHFSPlusAttrInlineData
    push_u32(&mut buf, 0); // reserved[0]
    push_u32(&mut buf, 0); // reserved[1]
    push_u32(&mut buf, value.len() as u32);
    buf.extend_from_slice(value);
    buf
}

fn build_thread_record(record_type: u16, parent_id: u32, name: &str) -> Vec<u8> {
    let name_bytes = encode_utf16be(name);
    let name_len = name_bytes.len() / 2;

    let mut buf = Vec::new();
    push_u16(&mut buf, record_type);
    push_u16(&mut buf, 0); // reserved
    push_u32(&mut buf, parent_id);
    push_u16(&mut buf, name_len as u16);
    buf.extend_from_slice(&name_bytes);
    buf
}

// ---------------------------------------------------------------------------
// Primitive helpers
// ---------------------------------------------------------------------------

fn push_bsd_info(buf: &mut Vec<u8>, mode: u16) {
    push_u32(buf, 0); // owner_id
    push_u32(buf, 0); // group_id
    buf.push(0); // admin_flags
    buf.push(0); // owner_flags
    push_u16(buf, mode);
    push_u32(buf, 0); // special
}

fn push_fork_data(
    buf: &mut Vec<u8>,
    logical_size: u64,
    total_blocks: u32,
    start_block: u32,
    block_count: u32,
) {
    push_u64(buf, logical_size);
    push_u32(buf, 0); // clump_size
    push_u32(buf, total_blocks);
    push_u32(buf, if block_count == 0 { 0 } else { start_block });
    push_u32(buf, block_count);
    // Extents 1-7 (empty)
    for _ in 1..8 {
        push_u32(buf, 0);
        push_u32(buf, 0);
    }
}

fn encode_utf16be(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|cp| cp.to_be_bytes()).collect()
}

fn push_u16(buf: &mut Vec<u8>, val: u16) {
    buf.extend_from_slice(&val.to_be_bytes());
}

fn push_u32(buf: &mut Vec<u8>, val: u32) {
    buf.extend_from_slice(&val.to_be_bytes());
}

fn push_u64(buf: &mut Vec<u8>, val: u64) {
    buf.extend_from_slice(&val.to_be_bytes());
}

fn write_u16_at(buf: &mut [u8], offset: usize, val: u16) {
    let bytes = val.to_be_bytes();
    buf[offset] = bytes[0];
    buf[offset + 1] = bytes[1];
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod regressions;
#[cfg(test)]
mod tests;
