use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek};

use crate::btree;
use crate::error::{ApfsError, Result};
use crate::{DirEntry, EntryKind};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

/// Catalog lookup policy belongs to the selected volume, never the host OS.
#[derive(Debug, Clone, Copy)]
pub struct CatalogContext {
    pub omap: crate::omap::OmapTree,
    pub incompatible_features: u64,
}
impl From<crate::omap::OmapTree> for CatalogContext {
    fn from(omap: crate::omap::OmapTree) -> Self {
        Self {
            omap,
            incompatible_features: 8,
        }
    }
}
impl From<u64> for CatalogContext {
    fn from(root: u64) -> Self {
        crate::omap::OmapTree::from(root).into()
    }
}
impl CatalogContext {
    pub(crate) fn name_key(self, name: &str) -> String {
        let name = if self.incompatible_features & 8 != 0 {
            name.nfd().collect()
        } else {
            name.to_owned()
        };
        if self.incompatible_features & 1 != 0 {
            name.case_fold().collect()
        } else {
            name
        }
    }
}

// Catalog record types (j_obj_types), stored in top 4 bits of key's obj_id_and_type
pub const J_TYPE_SNAP_METADATA: u8 = 1;
pub const J_TYPE_EXTENT: u8 = 2;
pub const J_TYPE_INODE: u8 = 3;
pub const J_TYPE_XATTR: u8 = 4;
pub const J_TYPE_SIBLING_LINK: u8 = 5;
pub const J_TYPE_DSTREAM_ID: u8 = 6;
pub const J_TYPE_CRYPTO_STATE: u8 = 7;
pub const J_TYPE_FILE_EXTENT: u8 = 8;
pub const J_TYPE_DIR_REC: u8 = 9;
pub const J_TYPE_DIR_STATS: u8 = 10;
pub const J_TYPE_SNAP_NAME: u8 = 11;
pub const J_TYPE_SIBLING_MAP: u8 = 12;

// Well-known OIDs
pub const ROOT_DIR_PARENT: u64 = 1; // Parent OID of root directory
pub const ROOT_DIR_RECORD: u64 = 2; // OID of the root directory inode

// Inode types (from BSD mode)
pub const INODE_DIR_TYPE: u16 = 0o040000; // S_IFDIR
pub const INODE_FILE_TYPE: u16 = 0o100000; // S_IFREG
pub const INODE_SYMLINK_TYPE: u16 = 0o120000; // S_IFLNK

/// Name of the extended attribute that stores symlink targets on APFS.
pub const SYMLINK_XATTR_NAME: &str = "com.apple.fs.symlink";

/// Offset of the attribute name within an xattr catalog key: the 8-byte
/// `obj_id_and_type` followed by the 2-byte `name_len`.
const XATTR_KEY_NAME_OFFSET: usize = 10;

// Xattr record flags (j_xattr_flags)
const XATTR_DATA_STREAM: u16 = 0x0001;

/// `j_xattr_dstream_t`: `xattr_obj_id u64` plus a five-field `j_dstream_t`.
const XATTR_DSTREAM_SIZE: usize = 8 + 40;

mod inode;
pub use inode::InodeVal;

/// Directory record value (j_drec_val_t)
#[derive(Debug, Clone)]
pub struct DrecVal {
    pub file_id: u64,
    pub date_added: i64,
    pub flags: u16,
}

impl DrecVal {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 18 {
            return Err(ApfsError::CorruptedData(format!(
                "drec value too short: {} bytes",
                data.len()
            )));
        }
        let mut cursor = Cursor::new(data);
        let file_id = cursor.read_u64::<LittleEndian>()?;
        let date_added = cursor.read_i64::<LittleEndian>()?;
        let flags = cursor.read_u16::<LittleEndian>()?;

        Ok(DrecVal {
            file_id,
            date_added,
            flags,
        })
    }

    /// Get the file type from the flags field (DT_* from dirent.h)
    pub fn file_type(&self) -> u16 {
        self.flags & 0x000F
    }
}

// DT_* constants for directory entry types
pub const DT_REG: u16 = 8; // Regular file
pub const DT_DIR: u16 = 4; // Directory
pub const DT_LNK: u16 = 10; // Symbolic link

/// File extent value (j_file_extent_val_t)
#[derive(Debug, Clone)]
pub struct FileExtentVal {
    pub flags_and_length: u64,
    pub phys_block_num: u64,
    pub crypto_id: u64,
}

impl FileExtentVal {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 24 {
            return Err(ApfsError::CorruptedData(format!(
                "file extent value too short: {} bytes",
                data.len()
            )));
        }
        let mut cursor = Cursor::new(data);
        let flags_and_length = cursor.read_u64::<LittleEndian>()?;
        let phys_block_num = cursor.read_u64::<LittleEndian>()?;
        let crypto_id = cursor.read_u64::<LittleEndian>()?;

        Ok(FileExtentVal {
            flags_and_length,
            phys_block_num,
            crypto_id,
        })
    }

    /// Get the logical length in bytes (lower 56 bits)
    pub fn length(&self) -> u64 {
        self.flags_and_length & 0x00FFFFFFFFFFFFFF
    }
}

/// Byte offset of `logical_addr` within a `j_file_extent_key_t`, which is a
/// `j_key_t` (one `u64` of packed OID and type) followed by the address.
const FILE_EXTENT_KEY_LOGICAL_ADDR_OFFSET: usize = 8;

/// A file extent record: the value paired with the logical address from its
/// key.
///
/// The logical offset of an extent within its file lives *only* in the key.
/// The value carries length, physical block and crypto id, so a reader that
/// keeps values alone cannot place them and has to assume the file is dense
/// and in order — which is wrong for every sparse file.
#[derive(Debug, Clone)]
pub struct FileExtentRecord {
    /// Logical byte offset of this extent within the file, from the key.
    pub logical_addr: u64,
    /// The extent's on-disk value.
    pub value: FileExtentVal,
}

/// Extract `logical_addr` from a file extent record's key.
fn parse_file_extent_logical_addr(key: &[u8]) -> Result<u64> {
    let bytes = key
        .get(FILE_EXTENT_KEY_LOGICAL_ADDR_OFFSET..FILE_EXTENT_KEY_LOGICAL_ADDR_OFFSET + 8)
        .ok_or_else(|| {
            ApfsError::CorruptedData(format!(
                "file extent key too short for a logical address: {} bytes",
                key.len()
            ))
        })?;
    Ok(u64::from_le_bytes(
        bytes.try_into().expect("slice is exactly 8 bytes"),
    ))
}

/// Decode a catalog key: extract obj_id and type from the combined j_key_t.
pub(crate) fn decode_catalog_key(key_bytes: &[u8]) -> Result<(u64, u8)> {
    if key_bytes.len() < 8 {
        return Err(ApfsError::InvalidBTree("catalog key too short".into()));
    }
    let obj_id_and_type = u64::from_le_bytes([
        key_bytes[0],
        key_bytes[1],
        key_bytes[2],
        key_bytes[3],
        key_bytes[4],
        key_bytes[5],
        key_bytes[6],
        key_bytes[7],
    ]);

    let obj_id = obj_id_and_type & 0x0FFFFFFFFFFFFFFF;
    let j_type = ((obj_id_and_type >> 60) & 0xF) as u8;

    Ok((obj_id, j_type))
}

/// Extract the name from a directory record key (j_drec_hashed_key_t or j_drec_key_t).
/// After the 8-byte obj_id_and_type, there's a 4-byte name_len_and_hash (for hashed keys)
/// followed by the UTF-8 name.
pub(crate) fn decode_drec_name(key: &[u8], hashed: bool) -> Result<String> {
    let (offset, length) = if hashed {
        let bytes: [u8; 4] = key
            .get(8..12)
            .ok_or_else(|| ApfsError::InvalidBTree("short hashed drec key".into()))?
            .try_into()
            .map_err(|_| ApfsError::InvalidBTree("short hashed drec key".into()))?;
        (12, (u32::from_le_bytes(bytes) & 0x3ff) as usize)
    } else {
        let bytes: [u8; 2] = key
            .get(8..10)
            .ok_or_else(|| ApfsError::InvalidBTree("short drec key".into()))?
            .try_into()
            .map_err(|_| ApfsError::InvalidBTree("short drec key".into()))?;
        (10, u16::from_le_bytes(bytes) as usize)
    };
    let raw = key
        .get(offset..offset + length)
        .ok_or_else(|| ApfsError::InvalidBTree("drec name exceeds key".into()))?;
    let raw = raw
        .strip_suffix(&[0])
        .ok_or_else(|| ApfsError::InvalidBTree("unterminated drec name".into()))?;
    if raw.is_empty() || raw.contains(&0) || raw.contains(&b'/') {
        return Err(ApfsError::InvalidBTree("invalid directory name".into()));
    }
    String::from_utf8(raw.to_vec())
        .map_err(|error| ApfsError::CorruptedData(format!("invalid UTF-8 directory name: {error}")))
}

/// List directory entries for a given parent OID.
///
/// Scans the catalog B-tree for all J_TYPE_DIR_REC entries whose obj_id matches
/// the parent directory OID. For each, looks up the inode to get size/timestamps.
pub fn list_directory<R: Read + Seek>(
    reader: &mut R,
    catalog_root: u64,
    omap_root: impl Into<CatalogContext>,
    block_size: u32,
    parent_oid: u64,
) -> Result<Vec<DirEntry>> {
    let omap_root = omap_root.into();
    let compare_fn = catalog_key(parent_oid, J_TYPE_DIR_REC);

    let entries = btree::btree_scan(
        reader,
        catalog_root,
        block_size,
        0,
        0, // variable-size keys and values
        &compare_fn,
        Some(omap_root.omap),
    )?;

    let mut dir_entries = Vec::new();
    for (key, val) in &entries {
        let name = match decode_drec_name(key, omap_root.incompatible_features & 9 != 0) {
            Ok(n) => n,
            Err(_) => continue,
        };

        let drec = match DrecVal::parse(val) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let kind = match drec.file_type() {
            DT_DIR => EntryKind::Directory,
            DT_LNK => EntryKind::Symlink,
            _ => EntryKind::File,
        };

        // Look up the inode for size/timestamps
        let (size, create_time, modify_time, change_time, access_time) =
            match lookup_inode(reader, catalog_root, omap_root, block_size, drec.file_id) {
                Ok(inode) => (
                    inode.size(),
                    inode.create_time,
                    inode.modify_time,
                    inode.change_time,
                    inode.access_time,
                ),
                Err(_) => (0, 0, 0, 0, 0),
            };

        dir_entries.push(DirEntry {
            name,
            date_added: drec.date_added,
            link_target: None,
            oid: drec.file_id,
            kind,
            size,
            create_time,
            modify_time,
            change_time,
            access_time,
        });
    }

    Ok(dir_entries)
}

/// Look up an inode record in the catalog B-tree.
pub fn lookup_inode<R: Read + Seek>(
    reader: &mut R,
    catalog_root: u64,
    omap_root: impl Into<CatalogContext>,
    block_size: u32,
    oid: u64,
) -> Result<InodeVal> {
    let omap_root = omap_root.into();
    let compare_fn = catalog_key(oid, J_TYPE_INODE);

    let val = btree::btree_lookup(
        reader,
        catalog_root,
        block_size,
        0,
        0,
        &compare_fn,
        Some(omap_root.omap),
    )?;

    match val {
        Some(data) => InodeVal::parse(&data),
        None => Err(ApfsError::FileNotFound(format!("inode OID {}", oid))),
    }
}

/// Look up all file extent records for a given file OID (private_id).
pub fn lookup_extents<R: Read + Seek>(
    reader: &mut R,
    catalog_root: u64,
    omap_root: impl Into<CatalogContext>,
    block_size: u32,
    file_oid: u64,
) -> Result<Vec<FileExtentRecord>> {
    let omap_root = omap_root.into();
    let compare_fn = catalog_key(file_oid, J_TYPE_FILE_EXTENT);

    let entries = btree::btree_scan(
        reader,
        catalog_root,
        block_size,
        0,
        0,
        &compare_fn,
        Some(omap_root.omap),
    )?;

    let mut extents = Vec::new();
    for (key, val) in &entries {
        extents.push(FileExtentRecord {
            logical_addr: parse_file_extent_logical_addr(key)?,
            value: FileExtentVal::parse(val)?,
        });
    }

    Ok(extents)
}

/// Order an on-disk xattr catalog key against the `(oid, name)` being searched for.
///
/// Catalog records sort by OID, then by record type, and xattr records then sort
/// by the NUL-terminated attribute name. The name is preceded in the key by a
/// `name_len` field, so the tie-break skips it rather than comparing key bytes
/// straight through.
fn xattr_name_bytes(key: &[u8]) -> Result<&[u8]> {
    let length = key
        .get(8..10)
        .ok_or_else(|| ApfsError::CorruptedData("short xattr key".into()))?;
    let length = usize::from(u16::from_le_bytes([length[0], length[1]]));
    let bytes = key
        .get(XATTR_KEY_NAME_OFFSET..XATTR_KEY_NAME_OFFSET + length)
        .ok_or_else(|| ApfsError::CorruptedData("xattr name exceeds key".into()))?;
    let name = bytes
        .strip_suffix(&[0])
        .ok_or_else(|| ApfsError::CorruptedData("unterminated xattr name".into()))?;
    if name.is_empty() || name.contains(&0) {
        return Err(ApfsError::CorruptedData("invalid xattr name".into()));
    }
    std::str::from_utf8(name)
        .map_err(|error| ApfsError::CorruptedData(format!("invalid xattr UTF-8: {error}")))?;
    Ok(bytes)
}

fn compare_xattr_key(key: &[u8], oid: u64, name_with_nul: &[u8]) -> Result<std::cmp::Ordering> {
    Ok(match compare_key_to(key, oid, J_TYPE_XATTR)? {
        std::cmp::Ordering::Equal => xattr_name_bytes(key)?.cmp(name_with_nul),
        order => order,
    })
}

/// Where an extended attribute keeps its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XattrValue {
    /// The value is in the record itself.
    Embedded(Vec<u8>),
    /// The value is a data stream, addressed like file data. `obj_id` is the
    /// id its file extents are keyed by. Resource forks always take this form.
    DataStream { obj_id: u64, size: u64 },
}

/// Look up an extended attribute value for an inode.
///
/// Xattr catalog keys are `[obj_id_and_type u64][name_len u16][name\0]`, so
/// the lookup compares the OID/type first, then the NUL-terminated name.
///
/// Returns `None` when the inode carries no attribute of that name. Attributes
/// stored as a data stream are rejected with [`ApfsError::Unsupported`]; use
/// [`lookup_xattr_value`] to handle both forms.
pub fn lookup_xattr<R: Read + Seek>(
    reader: &mut R,
    catalog_root: u64,
    omap_root: impl Into<CatalogContext>,
    block_size: u32,
    oid: u64,
    name: &str,
) -> Result<Option<Vec<u8>>> {
    let omap_root = omap_root.into();
    match lookup_xattr_value(reader, catalog_root, omap_root, block_size, oid, name)? {
        Some(XattrValue::Embedded(data)) => Ok(Some(data)),
        Some(XattrValue::DataStream { .. }) => Err(ApfsError::Unsupported(
            "xattr is stored as a data stream, not embedded in the record".into(),
        )),
        None => Ok(None),
    }
}

/// Look up an extended attribute, reporting whether its value is embedded or
/// held in a data stream.
pub fn lookup_xattr_value<R: Read + Seek>(
    reader: &mut R,
    catalog_root: u64,
    omap_root: impl Into<CatalogContext>,
    block_size: u32,
    oid: u64,
    name: &str,
) -> Result<Option<XattrValue>> {
    let omap_root = omap_root.into();
    let mut search_name = Vec::with_capacity(name.len() + 1);
    search_name.extend_from_slice(name.as_bytes());
    search_name.push(0);
    let compare_fn = |key: &[u8]| compare_xattr_key(key, oid, &search_name);

    let value = btree::btree_lookup(
        reader,
        catalog_root,
        block_size,
        0,
        0,
        &compare_fn,
        Some(omap_root.omap),
    )?;

    match value {
        Some(value) => Ok(Some(parse_xattr_value(&value)?)),
        None => Ok(None),
    }
}

/// Names of every extended attribute on an inode, in on-disk order.
pub fn list_xattr_names<R: Read + Seek>(
    reader: &mut R,
    catalog_root: u64,
    omap_root: impl Into<CatalogContext>,
    block_size: u32,
    oid: u64,
) -> Result<Vec<String>> {
    let omap_root = omap_root.into();
    let compare_fn = catalog_key(oid, J_TYPE_XATTR);
    let records = btree::btree_scan(
        reader,
        catalog_root,
        block_size,
        0,
        0,
        &compare_fn,
        Some(omap_root.omap),
    )?;

    let mut names = Vec::with_capacity(records.len());
    for (key, _) in records {
        let bytes = xattr_name_bytes(&key)?;
        let raw = &bytes[..bytes.len() - 1];
        names.push(String::from_utf8(raw.to_vec()).map_err(|e| {
            ApfsError::CorruptedData(format!("xattr name for inode {oid} is not UTF-8: {e}"))
        })?);
    }
    Ok(names)
}

/// Parse an xattr record value: `flags u16 | data_len u16 | data`.
///
/// A record flagged `XATTR_DATA_STREAM` carries a `j_xattr_dstream_t` in place
/// of the value — `xattr_obj_id u64` then a `j_dstream_t` opening with
/// `size u64`, both little-endian (Apple File System Reference, 2020-06-22,
/// "j_xattr_dstream_t" and "j_dstream_t"; same layout in apfs-fuse
/// `DiskStruct.h`).
fn parse_xattr_value(value: &[u8]) -> Result<XattrValue> {
    if value.len() < 4 {
        return Err(ApfsError::CorruptedData(format!(
            "xattr value too short: {} bytes",
            value.len()
        )));
    }
    let flags = u16::from_le_bytes([value[0], value[1]]);
    let data_len = u16::from_le_bytes([value[2], value[3]]) as usize;
    if !matches!(flags & 3, 1 | 2) || flags & !7 != 0 {
        return Err(ApfsError::Unsupported(format!("xattr flags {flags:#x}")));
    }

    if flags & XATTR_DATA_STREAM != 0 {
        if data_len != XATTR_DSTREAM_SIZE {
            return Err(ApfsError::CorruptedData(
                "invalid xattr data-stream reference length".into(),
            ));
        }
        let xdata = value.get(4..4 + XATTR_DSTREAM_SIZE).ok_or_else(|| {
            ApfsError::CorruptedData(format!(
                "xattr data-stream reference truncated: {} bytes after the header",
                value.len().saturating_sub(4)
            ))
        })?;
        let field = |offset: usize| {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&xdata[offset..offset + 8]);
            u64::from_le_bytes(bytes)
        };
        return Ok(XattrValue::DataStream {
            obj_id: field(0),
            size: field(8),
        });
    }

    let data = value
        .get(4..4 + data_len)
        .ok_or_else(|| ApfsError::CorruptedData("truncated embedded xattr".into()))?;
    Ok(XattrValue::Embedded(data.to_vec()))
}

/// Order an on-disk catalog key against the `(oid, type)` being searched for.
///
/// An undecodable key fails the operation. Any ordering guessed for it would
/// steer the descent or end a scan on the damaged key, reporting records that
/// exist as absent. PROVISIONAL(anomaly-channel): fail now, degrade to a
/// reported miss once there is somewhere to report to.
fn compare_key_to(key: &[u8], oid: u64, j_type: u8) -> Result<std::cmp::Ordering> {
    let (key_oid, key_type) = decode_catalog_key(key)?;
    Ok(compare_catalog_keys(key_oid, key_type, oid, j_type))
}

/// Comparator selecting the record with this `(oid, type)`.
///
/// Used for both `btree_lookup`, which wants the single matching record, and
/// `btree_scan`, which reads the same ordering as a range and collects the run
/// of `Equal` keys.
fn catalog_key(oid: u64, j_type: u8) -> impl Fn(&[u8]) -> Result<std::cmp::Ordering> {
    move |key| compare_key_to(key, oid, j_type)
}

/// Resolve a path like "/Applications/Upscayl.app/Contents/Info.plist" to its (OID, InodeVal).
pub fn resolve_path<R: Read + Seek>(
    reader: &mut R,
    catalog_root: u64,
    omap_root: impl Into<CatalogContext>,
    block_size: u32,
    path: &str,
) -> Result<(u64, InodeVal)> {
    let omap_root = omap_root.into();
    let path = path.trim_matches('/');

    if path.is_empty() {
        // Root directory
        let inode = lookup_inode(reader, catalog_root, omap_root, block_size, ROOT_DIR_RECORD)?;
        return Ok((ROOT_DIR_RECORD, inode));
    }

    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut current_parent = ROOT_DIR_RECORD;

    for (i, component) in components.iter().enumerate() {
        // Look up the directory record for this component under current_parent
        let drec = lookup_drec(
            reader,
            omap_root,
            catalog_root,
            block_size,
            current_parent,
            component,
        )?;

        if i == components.len() - 1 {
            // Final component — look up its inode
            let inode = lookup_inode(reader, catalog_root, omap_root, block_size, drec.file_id)?;
            return Ok((drec.file_id, inode));
        }

        // Not the final component — it must be a directory
        if drec.file_type() != DT_DIR {
            return Err(ApfsError::NotADirectory(components[..=i].join("/")));
        }

        current_parent = drec.file_id;
    }

    unreachable!()
}

/// Look up a specific directory record by name under a parent OID.
fn lookup_drec<R: Read + Seek>(
    reader: &mut R,
    omap_root: impl Into<CatalogContext>,
    catalog_root: u64,
    block_size: u32,
    parent_oid: u64,
    name: &str,
) -> Result<DrecVal> {
    let omap_root = omap_root.into();
    // Scan all DRECs for this parent and find the one with matching name
    let compare_fn = catalog_key(parent_oid, J_TYPE_DIR_REC);

    let entries = btree::btree_scan(
        reader,
        catalog_root,
        block_size,
        0,
        0,
        &compare_fn,
        Some(omap_root.omap),
    )?;

    let wanted = omap_root.name_key(name);
    let mut found = None;
    for (key, val) in &entries {
        let entry_name = decode_drec_name(key, omap_root.incompatible_features & 9 != 0)?;
        if entry_name == name {
            return DrecVal::parse(val);
        }
        if omap_root.name_key(&entry_name) == wanted {
            if found.is_some() {
                return Err(ApfsError::CorruptedData(
                    "ambiguous normalized directory name".into(),
                ));
            }
            found = Some(DrecVal::parse(val)?);
        }
    }
    found.ok_or_else(|| ApfsError::FileNotFound(name.to_string()))
}

/// Compare two catalog keys in APFS sort order: OID first, then type.
/// Returns the ordering of (oid_a, type_a) vs (oid_b, type_b).
///
/// Catalog records are deliberately *not* ordered by the packed
/// `obj_id_and_type` word they are stored in. The record type occupies the high
/// nibble, so ordering that word as an integer would sort by type before OID,
/// and comparing its bytes in place is wrong again because it is stored
/// little-endian. Decode with `decode_catalog_key` and order through this
/// function rather than touching the raw key bytes.
pub(crate) fn compare_catalog_keys(
    oid_a: u64,
    type_a: u8,
    oid_b: u64,
    type_b: u8,
) -> std::cmp::Ordering {
    match oid_a.cmp(&oid_b) {
        std::cmp::Ordering::Equal => type_a.cmp(&type_b),
        ord => ord,
    }
}

#[cfg(test)]
mod tests;
