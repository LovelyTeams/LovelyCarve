// Not public: its comparator contract fails silently when broken, so callers
// are served by the readers below instead. See the module docs.
mod access;
mod backing;
pub(crate) mod btree;
pub mod catalog;
pub use access::{ContainerInfo, XattrInfo};
mod snapshots;
pub use snapshots::SnapshotInfo;
pub mod error;
pub mod extents;
pub mod fletcher;
pub mod object;
pub mod omap;
pub mod superblock;

pub use error::{ApfsError, Result};

/// Failure while decoding a transparently compressed file.
pub use cmpfs::CmpfsError as CompressionError;
/// `com.apple.decmpfs` header of a transparently compressed file.
pub use cmpfs::Header as CompressionHeader;
/// Where a compressed file's payload lives.
pub use cmpfs::Storage as CompressionStorage;
/// Whether an extended attribute is user data or compression machinery.
pub use cmpfs::XattrKind;

use std::io::{Read, Seek, Write};

/// A seekable logical file stream. Implementers return the same logical bytes
/// through Read and Seek, preserving EOF and errors rather than shortening files.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

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
    pub date_added: i64,
    pub link_target: Option<String>,
    pub name: String,
    pub oid: u64,
    pub kind: EntryKind,
    pub size: u64,
    pub create_time: i64,
    pub modify_time: i64,
    pub change_time: i64,
    pub access_time: i64,
}

/// Detailed file/directory metadata
#[derive(Debug, Clone)]
pub struct FileStat {
    pub parent_oid: u64,
    pub private_id: u64,
    pub bsd_flags: u32,
    pub device_number: Option<u32>,
    pub oid: u64,
    pub kind: EntryKind,
    pub size: u64,
    pub create_time: i64,
    pub modify_time: i64,
    pub change_time: i64,
    pub access_time: i64,
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub nlink: u32,
    /// Present when the file is transparently compressed. `size` above is then
    /// the decompressed size, which the inode does not record.
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

/// Volume information
#[derive(Debug, Clone)]
pub struct VolumeInfo {
    pub uuid: [u8; 16],
    pub volume_group_id: [u8; 16],
    pub transaction_id: u64,
    pub features: u64,
    pub readonly_compatible_features: u64,
    pub incompatible_features: u64,
    pub flags: u64,
    pub role: u16,
    pub allocated_size: u64,
    pub num_snapshots: u64,
    pub next_object_id: u64,
    pub encrypted: bool,
    pub index: usize,
    pub oid: u64,
    pub name: String,
    pub block_size: u32,
    pub num_files: u64,
    pub num_directories: u64,
    pub num_symlinks: u64,
}

/// One record surfaced by [`ApfsVolume::walk_catalog`]. Catalog leaves are
/// sorted by (oid, type), so for a given directory all of its `DirEntry`
/// records arrive consecutively — but a child's `Inode` record lives under
/// the *child's* OID elsewhere in the stream, which is why callers join the
/// two by OID after the walk instead of expecting them adjacent.
#[derive(Debug, Clone)]
pub enum CatalogRecord {
    /// A `J_TYPE_INODE` record: `oid` is the file/directory's catalog OID.
    Inode { oid: u64, inode: catalog::InodeVal },
    /// A `J_TYPE_DIR_REC` record: one name inside directory `parent_oid`,
    /// pointing at the child's OID via `drec.file_id`.
    DirEntry {
        parent_oid: u64,
        name: String,
        drec: catalog::DrecVal,
    },
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

/// High-level read-only APFS volume reader
pub struct ApfsVolume<R: Read + Seek> {
    reader: backing::ContainerReader<R>,
    block_size: u32,
    vol_omap_root_block: catalog::CatalogContext,
    catalog_root_block: u64,
    snapshot_tree_oid: u64,
    snapshot_tree_type: u32,
    info: VolumeInfo,
}

impl<R: Read + Seek> ApfsVolume<R> {
    /// Open an APFS container and mount the first volume.
    ///
    /// 1. Read block 0 → parse NX superblock, validate NXSB magic + Fletcher-64
    /// 2. Scan checkpoint descriptor area for latest valid NX superblock
    /// 3. Read container OMAP at omap_oid physical block
    /// 4. Find first non-zero OID in fs_oids array
    /// 5. Resolve volume OID → physical block via container OMAP
    /// 6. Parse volume superblock (APSB magic)
    /// 7. Read volume OMAP at vol.omap_oid physical block
    /// 8. Resolve vol.root_tree_oid → physical block via volume OMAP → catalog B-tree root
    /// 9. Store all state
    pub fn open(mut reader: R) -> Result<Self> {
        let container = read_container(&mut reader)?;
        let (volume_index, vol_oid) = container
            .nxsb
            .fs_oids
            .iter()
            .enumerate()
            .find_map(|(index, &oid)| (oid != 0).then_some((index, oid)))
            .ok_or(ApfsError::NoVolume)?;
        Self::open_volume(reader, container, volume_index, vol_oid)
    }

    /// Open a specific APFS volume by its slot index in the container
    /// `fs_oids` array.
    pub fn open_index(mut reader: R, volume_index: usize) -> Result<Self> {
        let container = read_container(&mut reader)?;
        let vol_oid = container
            .nxsb
            .fs_oids
            .get(volume_index)
            .copied()
            .filter(|&oid| oid != 0)
            .ok_or(ApfsError::NoVolume)?;
        Self::open_volume(reader, container, volume_index, vol_oid)
    }

    /// List every APFS volume advertised by the container `fs_oids` array.
    pub fn list_volumes(mut reader: R) -> Result<Vec<VolumeInfo>> {
        let container = read_container(&mut reader)?;
        let mut reader = backing::ContainerReader::new(
            reader,
            container.nxsb.block_count * u64::from(container.nxsb.block_size),
        );
        let mut volumes = Vec::new();
        for (volume_index, vol_oid) in container
            .nxsb
            .fs_oids
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, oid)| *oid != 0)
        {
            let vol_sb = read_volume_superblock(
                &mut reader,
                container.container_omap_root,
                container.nxsb.block_size,
                vol_oid,
            )?;
            volumes.push(volume_info_from_superblock(
                volume_index,
                vol_oid,
                container.nxsb.block_size,
                &vol_sb,
            ));
        }
        Ok(volumes)
    }

    fn open_volume(
        reader: R,
        container: ContainerState,
        volume_index: usize,
        vol_oid: u64,
    ) -> Result<Self> {
        let block_size = container.nxsb.block_size;
        let mut reader = backing::ContainerReader::new(
            reader,
            container.nxsb.block_count * u64::from(block_size),
        );
        let vol_sb = read_volume_superblock(
            &mut reader,
            container.container_omap_root,
            block_size,
            vol_oid,
        )?;

        if vol_sb.fs_flags & 1 == 0 {
            return Err(ApfsError::Unsupported(
                "encrypted APFS volume requires FileVault keybag unlock".into(),
            ));
        }
        if vol_sb.incompatible_features & !0x0b != 0 {
            return Err(ApfsError::Unsupported(format!(
                "volume incompatible features {:#x} (sealed, encryption rolling or incomplete restore)",
                vol_sb.incompatible_features
            )));
        }

        let vol_omap_root_block = omap::OmapTree {
            root: omap::read_omap_tree_root(&mut reader, vol_sb.omap_oid, block_size)?,
            xid: vol_sb.header.xid,
        };

        let catalog_root_block = omap::omap_lookup(
            &mut reader,
            vol_omap_root_block,
            block_size,
            vol_sb.root_tree_oid,
        )?;

        Ok(ApfsVolume {
            reader,
            block_size,
            vol_omap_root_block: catalog::CatalogContext {
                omap: vol_omap_root_block,
                incompatible_features: vol_sb.incompatible_features,
            },
            catalog_root_block,
            snapshot_tree_oid: vol_sb.snap_meta_tree_oid,
            snapshot_tree_type: vol_sb.snap_meta_tree_type,
            info: volume_info_from_superblock(volume_index, vol_oid, block_size, &vol_sb),
        })
    }

    /// Get volume metadata
    pub fn volume_info(&self) -> &VolumeInfo {
        &self.info
    }

    /// Stream every inode and directory record of the catalog B-tree, in one
    /// pass, to `visit`. This is the bulk-enumeration primitive behind
    /// whole-volume snapshots: where [`list_directory`](Self::list_directory)
    /// pays a path resolution plus one B-tree descent *per child* for the
    /// inode join, this walks each catalog tree node exactly once and lets
    /// the caller join `DirEntry.drec.file_id` → `Inode.oid` in memory.
    ///
    /// The volume OMAP is preloaded into a map first so child-node resolution
    /// is a hash lookup, not a per-node OMAP descent. Records that fail to
    /// parse are skipped, matching the lenient per-record handling in
    /// `catalog::list_directory`. The visitor returns `false` to stop early
    /// (e.g. cancellation); the walk then returns `Ok(false)`.
    pub fn walk_catalog(&mut self, visit: &mut dyn FnMut(CatalogRecord) -> bool) -> Result<bool> {
        let omap_cache = omap::load_omap(
            &mut self.reader,
            self.vol_omap_root_block.omap,
            self.block_size,
        )?;
        btree::btree_walk_leaves(
            &mut self.reader,
            self.catalog_root_block,
            self.block_size,
            Some(&omap_cache),
            &mut |key: &[u8], val: &[u8]| {
                let Ok((oid, j_type)) = catalog::decode_catalog_key(key) else {
                    return true;
                };
                match j_type {
                    catalog::J_TYPE_INODE => match catalog::InodeVal::parse(val) {
                        Ok(inode) => visit(CatalogRecord::Inode { oid, inode }),
                        Err(_) => true,
                    },
                    catalog::J_TYPE_DIR_REC => {
                        match (
                            catalog::decode_drec_name(
                                key,
                                self.vol_omap_root_block.incompatible_features & 9 != 0,
                            ),
                            catalog::DrecVal::parse(val),
                        ) {
                            (Ok(name), Ok(drec)) => visit(CatalogRecord::DirEntry {
                                parent_oid: oid,
                                name,
                                drec,
                            }),
                            _ => true,
                        }
                    }
                    _ => true,
                }
            },
        )
    }

    /// List entries in a directory by path
    pub fn list_directory(&mut self, path: &str) -> Result<Vec<DirEntry>> {
        let (oid, _inode) = if path == "/" || path.is_empty() {
            // Root directory has a well-known OID
            (catalog::ROOT_DIR_PARENT, catalog::ROOT_DIR_RECORD)
        } else {
            let (oid, inode) = catalog::resolve_path(
                &mut self.reader,
                self.catalog_root_block,
                self.vol_omap_root_block,
                self.block_size,
                path,
            )?;
            if inode.kind() != catalog::INODE_DIR_TYPE {
                return Err(ApfsError::NotADirectory(path.to_string()));
            }
            (oid, oid)
        };

        let parent = if path == "/" || path.is_empty() {
            catalog::ROOT_DIR_RECORD
        } else {
            oid
        };

        let mut entries = catalog::list_directory(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            parent,
        )?;
        self.resolve_entry_sizes(&mut entries)?;
        Ok(entries)
    }

    /// Make listed sizes agree with `stat`. A compressed file's inode records
    /// no size of its own — the real length is in the decmpfs header — and a
    /// symlink inode reports 0 where `stat` reports the target length.
    fn resolve_entry_sizes(&mut self, entries: &mut [DirEntry]) -> Result<()> {
        for entry in entries.iter_mut() {
            match entry.kind {
                EntryKind::File => {
                    if let Some(header) = self.compression(entry.oid)? {
                        entry.size = header.uncompressed_size;
                    }
                }
                EntryKind::Symlink => {
                    if let Some(target) = self.symlink_target(entry.oid)? {
                        entry.size = target.len() as u64;
                        entry.link_target = Some(String::from_utf8(target).map_err(|error| {
                            ApfsError::CorruptedData(format!("invalid symlink UTF-8: {error}"))
                        })?);
                    }
                }
                EntryKind::Directory => {}
            }
        }
        Ok(())
    }

    /// Read an entire file into memory
    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.read_file_to(path, &mut buf)?;
        Ok(buf)
    }

    /// Symlink targets on APFS are stored in a "com.apple.fs.symlink"
    /// extended attribute (NUL-terminated), not as file data. Returns the
    /// target bytes for symlink inodes, or None when no xattr is present.
    fn symlink_target(&mut self, oid: u64) -> Result<Option<Vec<u8>>> {
        let Some(xattr) = self.read_xattr(oid, catalog::SYMLINK_XATTR_NAME)? else {
            return Ok(None);
        };
        let end = xattr.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        Ok(Some(xattr[..end].to_vec()))
    }

    /// Read an extended attribute, or `None` when the inode has no attribute
    /// of that name.
    ///
    /// Resolves both storage forms: values held in the catalog record and
    /// values held in a data stream, which is how resource forks are stored.
    pub fn get_xattr(&mut self, path: &str, name: &str) -> Result<Option<Vec<u8>>> {
        let (oid, _inode) = catalog::resolve_path(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            path,
        )?;
        self.read_xattr(oid, name)
    }

    /// Every extended attribute on a file or directory, classified.
    ///
    /// Nothing is filtered out: a caller inspecting the volume sees what the
    /// volume holds. [`XattrKind::Compression`] marks the attributes macOS
    /// hides, which [`Self::read_file`] has already resolved and which must
    /// not be copied onto an extracted file.
    pub fn list_xattrs(&mut self, path: &str) -> Result<Vec<XattrEntry>> {
        let (oid, _inode) = catalog::resolve_path(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            path,
        )?;
        let names = catalog::list_xattr_names(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            oid,
        )?;
        Ok(classify(names))
    }

    fn read_xattr(&mut self, oid: u64, name: &str) -> Result<Option<Vec<u8>>> {
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
            Some(catalog::XattrValue::Embedded(data)) => Ok(Some(data)),
            Some(catalog::XattrValue::DataStream { obj_id, size }) => {
                let extents = catalog::lookup_extents(
                    &mut self.reader,
                    self.catalog_root_block,
                    self.vol_omap_root_block,
                    self.block_size,
                    obj_id,
                )?;
                let mut data = Vec::new();
                extents::read_file_data(
                    &mut self.reader,
                    self.block_size,
                    &extents,
                    size,
                    &mut data,
                )?;
                Ok(Some(data))
            }
        }
    }

    /// The `com.apple.decmpfs` header of a transparently compressed inode, or
    /// `None` when the file stores its bytes in the data fork as usual.
    fn compression(&mut self, oid: u64) -> Result<Option<CompressionHeader>> {
        let Some(attr) = self.read_xattr(oid, cmpfs::XATTR_NAME)? else {
            return Ok(None);
        };
        Ok(Some(CompressionHeader::parse(&attr)?))
    }

    /// Decompress a transparently compressed file.
    ///
    /// Whole-file, unlike the extent path: compression blocks are addressed
    /// relative to the decompressed output, so nothing can be emitted before
    /// the block covering it has been decoded.
    fn read_compressed(&mut self, oid: u64, header: &CompressionHeader) -> Result<Vec<u8>> {
        let attr = self.read_xattr(oid, cmpfs::XATTR_NAME)?.ok_or_else(|| {
            ApfsError::CorruptedData(format!("inode {oid} lost its decmpfs attribute"))
        })?;

        let resource_fork = match header.storage() {
            CompressionStorage::ResourceFork => Some(
                self.read_xattr(oid, cmpfs::RESOURCE_FORK_XATTR_NAME)?
                    .ok_or_else(|| {
                        ApfsError::CorruptedData(format!(
                            "inode {oid} is compressed into its resource fork, but has none"
                        ))
                    })?,
            ),
            _ => None,
        };

        Ok(cmpfs::decompress(&attr, resource_fork.as_deref())?)
    }

    /// Stream a file to a writer
    ///
    /// Transparently compressed files are decompressed; see [`Self::stat`] to
    /// detect one first.
    pub fn read_file_to<W: Write>(&mut self, path: &str, writer: &mut W) -> Result<u64> {
        let (oid, inode) = catalog::resolve_path(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            path,
        )?;

        let mut file = self.open_inode(oid, inode)?;
        Ok(std::io::copy(&mut file, writer)?)
    }

    /// Open a seekable logical stream. Compressed files currently decode in full
    /// into memory; block-wise resource-fork seeking is not implemented here.
    /// Uncompressed streams seek directly through the validated extent map.
    pub fn open_file(&mut self, path: &str) -> Result<Box<dyn ReadSeek + '_>> {
        let (oid, inode) = catalog::resolve_path(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            path,
        )?;

        self.open_inode(oid, inode)
    }

    /// Get metadata for a file or directory
    pub fn stat(&mut self, path: &str) -> Result<FileStat> {
        let (oid, inode) = catalog::resolve_path(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            path,
        )?;

        self.stat_inode(oid, inode)
    }

    /// Recursive walk of all entries
    pub fn walk(&mut self) -> Result<Vec<WalkEntry>> {
        let mut entries = Vec::new();
        self.walk_recursive(catalog::ROOT_DIR_RECORD, "", &mut entries)?;
        Ok(entries)
    }

    /// Check if a path exists
    pub fn exists(&mut self, path: &str) -> Result<bool> {
        match catalog::resolve_path(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            path,
        ) {
            Ok(_) => Ok(true),
            Err(ApfsError::FileNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn walk_recursive(
        &mut self,
        parent_oid: u64,
        parent_path: &str,
        entries: &mut Vec<WalkEntry>,
    ) -> Result<()> {
        let mut dir_entries = catalog::list_directory(
            &mut self.reader,
            self.catalog_root_block,
            self.vol_omap_root_block,
            self.block_size,
            parent_oid,
        )?;
        self.resolve_entry_sizes(&mut dir_entries)?;

        for entry in dir_entries {
            let full_path = if parent_path.is_empty() {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", parent_path, entry.name)
            };

            let is_dir = entry.kind == EntryKind::Directory;
            let oid = entry.oid;

            entries.push(WalkEntry {
                path: full_path.clone(),
                entry,
            });

            if is_dir {
                self.walk_recursive(oid, &full_path, entries)?;
            }
        }

        Ok(())
    }
}

struct ContainerState {
    nxsb: superblock::NxSuperblock,
    container_omap_root: omap::OmapTree,
}

fn read_container<R: Read + Seek>(reader: &mut R) -> Result<ContainerState> {
    let nxsb = superblock::read_nxsb(reader)?;
    let nxsb = superblock::find_latest_nxsb(reader, &nxsb)?;
    let container_omap_root = omap::OmapTree {
        root: omap::read_omap_tree_root(
            &mut backing::ContainerReader::new(
                reader,
                nxsb.block_count * u64::from(nxsb.block_size),
            ),
            nxsb.omap_oid,
            nxsb.block_size,
        )?,
        xid: nxsb.header.xid,
    };
    Ok(ContainerState {
        nxsb,
        container_omap_root,
    })
}

fn read_volume_superblock<R: Read + Seek>(
    reader: &mut R,
    container_omap_root: omap::OmapTree,
    block_size: u32,
    vol_oid: u64,
) -> Result<superblock::ApfsSuperblock> {
    let vol_block = omap::omap_lookup(reader, container_omap_root, block_size, vol_oid)?;
    let (_, vol_data) = object::read_object(reader, vol_block, block_size)?;
    let block = superblock::ApfsSuperblock::parse(&vol_data)?;
    if block.header.object_type() != object::OBJECT_TYPE_FS
        || block.header.oid != vol_oid
        || block.header.xid > container_omap_root.xid
    {
        return Err(ApfsError::CorruptedData(
            "APSB identity or transaction does not match the container".into(),
        ));
    }
    if block
        .fs_alloc_count
        .checked_mul(u64::from(block_size))
        .is_none()
    {
        return Err(ApfsError::CorruptedData(
            "volume allocated size overflow".into(),
        ));
    }
    Ok(block)
}

fn volume_info_from_superblock(
    index: usize,
    oid: u64,
    block_size: u32,
    vol_sb: &superblock::ApfsSuperblock,
) -> VolumeInfo {
    VolumeInfo {
        uuid: vol_sb.uuid,
        volume_group_id: vol_sb.volume_group_id,
        transaction_id: vol_sb.header.xid,
        features: vol_sb.features,
        readonly_compatible_features: vol_sb.readonly_compatible_features,
        incompatible_features: vol_sb.incompatible_features,
        flags: vol_sb.fs_flags,
        role: vol_sb.role,
        allocated_size: vol_sb.fs_alloc_count * u64::from(block_size),
        num_snapshots: vol_sb.num_snapshots,
        next_object_id: vol_sb.next_obj_id,
        encrypted: vol_sb.fs_flags & 1 == 0,
        index,
        oid,
        name: vol_sb.volume_name.clone(),
        block_size,
        num_files: vol_sb.num_files,
        num_directories: vol_sb.num_directories,
        num_symlinks: vol_sb.num_symlinks,
    }
}

#[cfg(test)]
mod tests;
