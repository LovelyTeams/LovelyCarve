pub mod access;
pub mod attributes;
pub mod btree;
pub mod catalog;
pub mod error;
pub mod extents;
pub mod unicode;
pub mod volume;

#[cfg(any(test, feature = "testutil"))]
pub mod testutil;

pub use error::{HfsPlusError, Result};
pub use volume::VolumeHeader;

/// Failure while decoding a transparently compressed file.
pub use cmpfs::CmpfsError as CompressionError;
/// `com.apple.decmpfs` header of a transparently compressed file.
pub use cmpfs::Header as CompressionHeader;
/// Where a compressed file's payload lives.
pub use cmpfs::Storage as CompressionStorage;
/// Whether an extended attribute is user data or compression machinery.
pub use cmpfs::XattrKind;

use std::io::{Read, Seek, Write};

/// Which on-disk header was accepted during volume open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderCopy {
    Primary,
    Alternate,
}

/// Entry kind in the filesystem
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
}

/// A directory entry returned by list_directory
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// File or folder name
    pub name: String,
    /// Catalog Node ID
    pub cnid: u32,
    /// Backing hardlink reference, retaining the directory entry CNID separately.
    pub link_reference: Option<u32>,
    /// Entry type
    pub kind: EntryKind,
    /// Data fork logical size (0 for directories)
    pub size: u64,
    /// HFS+ creation date (seconds since 1904-01-01)
    pub create_date: u32,
    /// HFS+ modification date
    pub modify_date: u32,
    /// HFS+ attribute modification date
    pub attribute_mod_date: u32,
    /// HFS+ access date
    pub access_date: u32,
}

/// HFS+ permissions (BSD-style)
#[derive(Debug, Clone)]
pub struct HfsPermissions {
    pub owner_id: u32,
    pub group_id: u32,
    pub mode: u16,
    /// BSD `st_flags` admin half. Bit 0x20 is `UF_COMPRESSED`, set on files
    /// whose real content lives in a `com.apple.decmpfs` extended attribute
    /// (transparent compression) rather than the data fork.
    pub admin_flags: u8,
    pub owner_flags: u8,
    /// Union of device number, hardlink reference or link count; interpret with mode/flags.
    pub special: u32,
}

/// Detailed file/directory metadata
#[derive(Debug, Clone)]
pub struct FileStat {
    pub cnid: u32,
    pub kind: EntryKind,
    pub size: u64,
    pub create_date: u32,
    pub modify_date: u32,
    pub attribute_mod_date: u32,
    pub access_date: u32,
    pub backup_date: u32,
    /// Finder added time is signed POSIX seconds, unlike the HFS epoch fields.
    pub added_time: Option<i32>,
    pub catalog_flags: u16,
    pub link_count: Option<u32>,
    pub device_number: Option<u32>,
    pub finder_info: [u8; 32],
    pub permissions: HfsPermissions,
    pub data_fork_extents: u32,
    pub resource_fork_size: u64,
    /// Present when the file is transparently compressed. `size` above is then
    /// the decompressed size; the data fork is empty.
    ///
    /// Also the signal that the file's compression is already resolved: the
    /// attributes listed as [`XattrKind::Compression`] must not be replicated
    /// onto an extracted copy.
    pub compression: Option<CompressionHeader>,
}

/// An extended attribute name, classified so a caller replicating metadata can
/// skip compression machinery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XattrEntry {
    /// Attribute name.
    pub name: String,
    /// Whether this is user data or transparent-compression machinery.
    pub kind: XattrKind,
}

/// Entry from walk() — includes full path
#[derive(Debug, Clone)]
pub struct WalkEntry {
    pub path: String,
    pub entry: DirEntry,
}

/// Pair each attribute name with its kind.
///
/// The file counts as compressed when the header attribute is present, which is
/// the same condition `read_file` decompresses on. Deriving it from the listing
/// keeps the two in step: if `read_file` treated the resource fork as a
/// payload, this reports it as machinery.
fn classify(names: Vec<String>) -> Vec<XattrEntry> {
    let compressed = names.iter().any(|name| name == cmpfs::XATTR_NAME);
    names
        .into_iter()
        .map(|name| XattrEntry {
            kind: cmpfs::classify_xattr(&name, compressed),
            name,
        })
        .collect()
}

/// High-level HFS+/HFSX volume reader
pub struct HfsVolume<R: Read + Seek> {
    reader: R,
    pub(crate) header: VolumeHeader,
    /// Selected on-disk header copy.
    pub header_copy: HeaderCopy,
    pub(crate) catalog_btree_header: btree::BTreeHeaderRecord,
    pub(crate) extents_btree_header: btree::BTreeHeaderRecord,
    /// Attributes B-tree header, read on first use. `Some(None)` records a
    /// volume with no attributes file. Loading it lazily keeps a damaged
    /// attributes tree from failing `open` for callers that never ask for an
    /// extended attribute.
    attributes_btree_header: Option<Option<btree::BTreeHeaderRecord>>,
}

impl<R: Read + Seek> HfsVolume<R> {
    /// Open and validate an HFS+/HFSX volume
    pub fn open(mut reader: R) -> Result<Self> {
        let (header, header_copy) = match VolumeHeader::parse(&mut reader) {
            Ok(header) => (header, HeaderCopy::Primary),
            Err(primary) => match VolumeHeader::parse_alternate(&mut reader) {
                Ok(header) => (header, HeaderCopy::Alternate),
                Err(_) => return Err(primary),
            },
        };

        // Read catalog B-tree header
        let mut catalog_btree_header =
            btree::read_btree_header(&mut reader, &header.catalog_file, header.block_size)?;

        if header.is_hfsx && !matches!(catalog_btree_header.key_compare_type, 0xbc | 0xcf) {
            return Err(HfsPlusError::InvalidBTree(
                "unsupported HFSX catalog comparison type".into(),
            ));
        }

        // Read extents overflow B-tree header
        let mut extents_btree_header =
            btree::read_btree_header(&mut reader, &header.extents_file, header.block_size)?;

        extents_btree_header.extents = extents::collect_extents(
            &mut reader,
            &header,
            &extents_btree_header,
            &header.extents_file,
            3,
            extents::FORK_TYPE_DATA,
        )?;
        catalog_btree_header.extents = extents::collect_extents(
            &mut reader,
            &header,
            &extents_btree_header,
            &header.catalog_file,
            4,
            extents::FORK_TYPE_DATA,
        )?;
        Ok(HfsVolume {
            reader,
            header,
            header_copy,
            catalog_btree_header,
            extents_btree_header,
            attributes_btree_header: None,
        })
    }

    /// Access the parsed volume header
    pub fn volume_header(&self) -> &VolumeHeader {
        &self.header
    }

    /// List entries in a directory by path
    pub fn list_directory(&mut self, path: &str) -> Result<Vec<DirEntry>> {
        let cnid = self.resolve_path_to_cnid(path)?;
        let mut entries = catalog::list_directory(
            &mut self.reader,
            &self.header,
            &self.catalog_btree_header,
            cnid,
        )?;
        self.resolve_entry_sizes(&mut entries)?;
        Ok(entries)
    }

    /// Make listed sizes agree with `stat`: a compressed file's data fork is
    /// empty, so its listed size is the decmpfs header's uncompressed size,
    /// exactly as `stat` reports it.
    fn resolve_entry_sizes(&mut self, entries: &mut [DirEntry]) -> Result<()> {
        for entry in entries.iter_mut() {
            if entry.link_reference.is_some() {
                let (record, _, _) = catalog::access::entry_by_id(
                    &mut self.reader,
                    &self.header,
                    &self.catalog_btree_header,
                    entry.cnid,
                )?;
                let record = catalog::access::resolve_link(
                    &mut self.reader,
                    &self.header,
                    &self.catalog_btree_header,
                    record,
                )?;
                match record {
                    catalog::CatalogRecord::File(file) => {
                        entry.size = self
                            .file_compression(&file)?
                            .map_or(file.data_fork.logical_size, |header| {
                                header.uncompressed_size
                            });
                    }
                    catalog::CatalogRecord::Folder(_) => {
                        entry.kind = EntryKind::Directory;
                        entry.size = 0;
                    }
                    catalog::CatalogRecord::FolderThread(_)
                    | catalog::CatalogRecord::FileThread(_) => {
                        return Err(HfsPlusError::CorruptedData(
                            "hardlink targets a thread".into(),
                        ));
                    }
                }
                continue;
            }
            if entry.kind == EntryKind::File
                && let Some(header) = self.compression(entry.cnid)?
            {
                entry.size = header.uncompressed_size;
            }
        }
        Ok(())
    }

    /// The Attributes B-tree header, or `None` on a volume with no attributes
    /// file. Read once and cached.
    fn attributes_btree(&mut self) -> Result<Option<btree::BTreeHeaderRecord>> {
        if self.attributes_btree_header.is_none() {
            let header = if self.header.attributes_file.logical_size == 0 {
                None
            } else {
                let mut tree = btree::read_btree_header(
                    &mut self.reader,
                    &self.header.attributes_file,
                    self.header.block_size,
                )?;
                tree.extents = extents::collect_extents(
                    &mut self.reader,
                    &self.header,
                    &self.extents_btree_header,
                    &self.header.attributes_file,
                    8,
                    extents::FORK_TYPE_DATA,
                )?;
                Some(tree)
            };
            self.attributes_btree_header = Some(header);
        }
        Ok(self.attributes_btree_header.clone().flatten())
    }

    /// Read an extended attribute, or `None` when the file has no attribute of
    /// that name.
    pub fn get_xattr(&mut self, path: &str, name: &str) -> Result<Option<Vec<u8>>> {
        let file_id = self.resolve_path_to_cnid(path)?;
        self.read_xattr(file_id, name)
    }

    /// Every extended attribute on a file or directory, classified.
    ///
    /// Nothing is filtered out: a caller inspecting the volume sees what the
    /// volume holds. [`XattrKind::Compression`] marks the attributes macOS
    /// hides, which [`Self::read_file`] has already resolved and which must
    /// not be copied onto an extracted file.
    ///
    /// Scans the Attributes B-tree, so it costs more than a single
    /// [`Self::get_xattr`]; prefer that when the name is known.
    ///
    /// HFS+ keeps the resource fork in the catalog record rather than as an
    /// attribute, so unlike APFS this never reports
    /// `com.apple.ResourceFork`; see [`FileStat::resource_fork_size`].
    pub fn list_xattrs(&mut self, path: &str) -> Result<Vec<XattrEntry>> {
        let file_id = self.resolve_path_to_cnid(path)?;
        let Some(attributes) = self.attributes_btree()? else {
            return Ok(Vec::new());
        };
        let names = attributes::list_names(&mut self.reader, &attributes, file_id)?;
        Ok(classify(names))
    }

    fn read_xattr(&mut self, file_id: u32, name: &str) -> Result<Option<Vec<u8>>> {
        let Some(attributes) = self.attributes_btree()? else {
            return Ok(None);
        };
        match attributes::lookup(&mut self.reader, &attributes, file_id, name)? {
            None => Ok(None),
            Some(attributes::AttrValue::Inline(data)) => Ok(Some(data)),
            Some(attributes::AttrValue::Fork(fork)) => {
                if fork.logical_size > 16 * 1024 * 1024 {
                    return Err(HfsPlusError::CorruptedData(
                        "attribute value exceeds 16 MiB memory budget".into(),
                    ));
                }
                let mut data = Vec::new();
                let map = attributes::fork_extents(
                    &mut self.reader,
                    &attributes,
                    &self.header,
                    file_id,
                    name,
                    &fork,
                )?;
                let mut reader = extents::ForkReader::from_extents(
                    &mut self.reader,
                    &fork,
                    self.header.block_size,
                    &map,
                )?;
                reader.read_to_end(&mut data)?;
                Ok(Some(data))
            }
        }
    }

    /// The `com.apple.decmpfs` header of a transparently compressed file, or
    /// `None` when the file stores its bytes in the data fork as usual.
    fn compression(&mut self, file_id: u32) -> Result<Option<CompressionHeader>> {
        let Some(attr) = self.read_xattr(file_id, cmpfs::XATTR_NAME)? else {
            return Ok(None);
        };
        Ok(Some(CompressionHeader::parse(&attr)?))
    }

    fn file_compression(
        &mut self,
        file: &catalog::CatalogFile,
    ) -> Result<Option<CompressionHeader>> {
        let compression = self.compression(file.file_id)?;
        if file.permissions.owner_flags & 0x20 != 0 && compression.is_none() {
            return Err(HfsPlusError::CorruptedData(
                "UF_COMPRESSED file lacks decmpfs attribute".into(),
            ));
        }
        Ok(compression)
    }

    /// Decompress a transparently compressed file.
    ///
    /// Whole-file, unlike the extent path: compression blocks are addressed
    /// relative to the decompressed output, so nothing can be emitted before
    /// the block covering it has been decoded.
    ///
    /// The resource fork here is the file's real fork from the catalog record,
    /// not an extended attribute — HFS+ has one, so it does not need the
    /// `com.apple.ResourceFork` attribute APFS uses.
    fn read_compressed(
        &mut self,
        file_record: &catalog::CatalogFile,
        header: &CompressionHeader,
    ) -> Result<Vec<u8>> {
        if header.uncompressed_size > 512 * 1024 * 1024
            || file_record.resource_fork.logical_size > 512 * 1024 * 1024
        {
            return Err(HfsPlusError::CorruptedData(
                "compressed file exceeds 512 MiB memory budget".into(),
            ));
        }
        let attr = self
            .read_xattr(file_record.file_id, cmpfs::XATTR_NAME)?
            .ok_or_else(|| {
                HfsPlusError::CorruptedData(format!(
                    "file {} lost its decmpfs attribute",
                    file_record.file_id
                ))
            })?;

        let resource_fork = if header.storage() == CompressionStorage::ResourceFork {
            if file_record.resource_fork.logical_size == 0 {
                return Err(HfsPlusError::CorruptedData(format!(
                    "file {} is compressed into its resource fork, but the fork is empty",
                    file_record.file_id
                )));
            }
            let mut data = Vec::new();
            extents::read_fork_data_typed(
                &mut self.reader,
                &self.header,
                &self.extents_btree_header,
                &file_record.resource_fork,
                file_record.file_id,
                extents::FORK_TYPE_RESOURCE,
                &mut data,
            )?;
            Some(data)
        } else {
            None
        };

        Ok(cmpfs::decompress(&attr, resource_fork.as_deref())?)
    }

    /// Read a complete logical file. Compression and overflow use the same path as streaming.
    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        let mut reader = self.open_file(path)?;
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        Ok(output)
    }

    /// Open a logical Read + Seek stream, including compressed files and overflow extents.
    /// Compressed content is currently decoded into memory before seeking.
    pub fn open_file(&mut self, path: &str) -> Result<access::FileReader<'_, R>> {
        let file = self.resolve_path_to_file(path)?;
        if let Some(header) = self.file_compression(&file)? {
            return Ok(access::FileReader::Decoded(std::io::Cursor::new(
                self.read_compressed(&file, &header)?,
            )));
        }
        let map = extents::collect_extents(
            &mut self.reader,
            &self.header,
            &self.extents_btree_header,
            &file.data_fork,
            file.file_id,
            extents::FORK_TYPE_DATA,
        )?;
        Ok(access::FileReader::Fork(extents::ForkReader::from_extents(
            &mut self.reader,
            &file.data_fork,
            self.header.block_size,
            &map,
        )?))
    }

    /// Stream complete logical file bytes, failing on missing mappings or truncated evidence.
    pub fn read_file_to<W: Write>(&mut self, path: &str, mut writer: W) -> Result<u64> {
        Ok(std::io::copy(&mut self.open_file(path)?, &mut writer)?)
    }

    /// Open the original resource fork, independently of transparent decompression.
    pub fn open_resource_fork(&mut self, path: &str) -> Result<extents::ForkReader<'_, R>> {
        let file = self.resolve_path_to_file(path)?;
        let map = extents::collect_extents(
            &mut self.reader,
            &self.header,
            &self.extents_btree_header,
            &file.resource_fork,
            file.file_id,
            extents::FORK_TYPE_RESOURCE,
        )?;
        extents::ForkReader::from_extents(
            &mut self.reader,
            &file.resource_fork,
            self.header.block_size,
            &map,
        )
    }

    /// Return the validated physical allocation map of the original data fork.
    pub fn data_extents(&mut self, path: &str) -> Result<Vec<volume::ExtentDescriptor>> {
        let file = self.resolve_path_to_file(path)?;
        extents::collect_extents(
            &mut self.reader,
            &self.header,
            &self.extents_btree_header,
            &file.data_fork,
            file.file_id,
            extents::FORK_TYPE_DATA,
        )
    }

    /// Get metadata for a file or directory
    pub fn stat(&mut self, path: &str) -> Result<FileStat> {
        let (record, _name) = self.resolve_path_to_record(path)?;
        match record {
            catalog::CatalogRecord::File(f) => {
                // A compressed file's data fork is empty; the real length is
                // in the decmpfs header.
                let compression = self.file_compression(&f)?;
                Ok(FileStat {
                    cnid: f.file_id,
                    // Same mode test as list_directory: a symlink is a file
                    // record whose mode says S_IFLNK.
                    kind: if f.permissions.file_mode & 0o170000 == 0o120000 {
                        EntryKind::Symlink
                    } else {
                        EntryKind::File
                    },
                    size: compression.map_or(f.data_fork.logical_size, |h| h.uncompressed_size),
                    create_date: f.create_date,
                    modify_date: f.content_mod_date,
                    attribute_mod_date: f.attribute_mod_date,
                    access_date: f.access_date,
                    backup_date: f.backup_date,
                    added_time: f.added_time,
                    catalog_flags: f.flags,
                    link_count: matches!(f.permissions.file_mode & 0o170000, 0o100000 | 0o120000)
                        .then_some(f.permissions.special.max(1)),
                    device_number: matches!(
                        f.permissions.file_mode & 0o170000,
                        0o020000 | 0o060000
                    )
                    .then_some(f.permissions.special),
                    finder_info: f.finder_info,
                    permissions: HfsPermissions {
                        owner_id: f.permissions.owner_id,
                        group_id: f.permissions.group_id,
                        mode: f.permissions.file_mode,
                        admin_flags: f.permissions.admin_flags,
                        owner_flags: f.permissions.owner_flags,
                        special: f.permissions.special,
                    },
                    data_fork_extents: f
                        .data_fork
                        .extents
                        .iter()
                        .filter(|e| e.block_count > 0)
                        .count() as u32,
                    resource_fork_size: f.resource_fork.logical_size,
                    compression,
                })
            }
            catalog::CatalogRecord::Folder(f) => Ok(FileStat {
                cnid: f.folder_id,
                kind: EntryKind::Directory,
                size: 0,
                create_date: f.create_date,
                modify_date: f.content_mod_date,
                attribute_mod_date: f.attribute_mod_date,
                access_date: f.access_date,
                backup_date: f.backup_date,
                added_time: f.added_time,
                catalog_flags: f.flags,
                link_count: None,
                device_number: None,
                finder_info: f.finder_info,
                permissions: HfsPermissions {
                    owner_id: f.permissions.owner_id,
                    group_id: f.permissions.group_id,
                    mode: f.permissions.file_mode,
                    admin_flags: f.permissions.admin_flags,
                    owner_flags: f.permissions.owner_flags,
                    special: f.permissions.special,
                },
                data_fork_extents: 0,
                resource_fork_size: 0,
                compression: None,
            }),
            _ => Err(HfsPlusError::CorruptedData(
                "unexpected thread record".into(),
            )),
        }
    }

    /// Recursive walk of all entries
    pub fn walk(&mut self) -> Result<Vec<WalkEntry>> {
        let mut entries = Vec::new();
        self.walk_recursive(
            catalog::CNID_ROOT_FOLDER,
            "",
            &mut entries,
            &mut std::collections::HashSet::new(),
        )?;
        Ok(entries)
    }

    /// Check if a path exists
    pub fn exists(&mut self, path: &str) -> Result<bool> {
        match self.resolve_path_to_record(path) {
            Ok(_) => Ok(true),
            Err(HfsPlusError::FileNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    // --- Internal helpers ---

    fn resolve_path_to_cnid(&mut self, path: &str) -> Result<u32> {
        let (record, _name) = self.resolve_path_to_record(path)?;
        match record {
            catalog::CatalogRecord::Folder(f) => Ok(f.folder_id),
            catalog::CatalogRecord::File(f) => Ok(f.file_id),
            _ => Err(HfsPlusError::CorruptedData(
                "unexpected thread record".into(),
            )),
        }
    }

    fn resolve_path_to_file(&mut self, path: &str) -> Result<catalog::CatalogFile> {
        let (record, _name) = self.resolve_path_to_record(path)?;
        match record {
            catalog::CatalogRecord::File(f) => Ok(f),
            catalog::CatalogRecord::Folder(_) => Err(HfsPlusError::NotADirectory(path.to_string())),
            _ => Err(HfsPlusError::CorruptedData(
                "unexpected thread record".into(),
            )),
        }
    }

    fn resolve_path_to_record(&mut self, path: &str) -> Result<(catalog::CatalogRecord, String)> {
        catalog::resolve_path(
            &mut self.reader,
            &self.header,
            &self.catalog_btree_header,
            path,
        )
    }

    fn walk_recursive(
        &mut self,
        parent_cnid: u32,
        parent_path: &str,
        entries: &mut Vec<WalkEntry>,
        visited: &mut std::collections::HashSet<u32>,
    ) -> Result<()> {
        if !visited.insert(parent_cnid) || visited.len() > 512 {
            return Err(HfsPlusError::CorruptedData(
                "cyclic directory tree or walk limit".into(),
            ));
        }
        let mut dir_entries = catalog::list_directory(
            &mut self.reader,
            &self.header,
            &self.catalog_btree_header,
            parent_cnid,
        )?;
        self.resolve_entry_sizes(&mut dir_entries)?;

        for entry in dir_entries {
            let full_path = if parent_path.is_empty() {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", parent_path, entry.name)
            };

            let is_dir = entry.kind == EntryKind::Directory;
            let cnid = if is_dir {
                self.resolve_path_to_cnid(&full_path)?
            } else {
                entry.cnid
            };

            entries.push(WalkEntry {
                path: full_path.clone(),
                entry,
            });

            if is_dir {
                self.walk_recursive(cnid, &full_path, entries, visited)?;
            }
        }

        visited.remove(&parent_cnid);
        Ok(())
    }
}
