//! Seekable logical files share a validated extent stream or decoded compression bytes.
use crate::extents::ForkReader;
use std::io::{self, Read, Seek, SeekFrom};

/// A logical file stream; compressed content has the same seek and read contract.
pub enum FileReader<'a, R: Read + Seek> {
    Fork(ForkReader<'a, R>),
    Decoded(io::Cursor<Vec<u8>>),
}
impl<R: Read + Seek> Read for FileReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Fork(reader) => reader.read(output),
            Self::Decoded(reader) => reader.read(output),
        }
    }
}
impl<R: Read + Seek> Seek for FileReader<'_, R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Fork(reader) => reader.seek(position),
            Self::Decoded(reader) => reader.seek(position),
        }
    }
}

/// Raw entry identity, parent and name. The record retains BSD/Finder fields and both forks.
#[derive(Debug, Clone)]
pub struct EntryMetadata {
    pub identifier: u32,
    pub parent_identifier: u32,
    pub name: String,
    pub link_identifier: Option<u32>,
    pub record: crate::catalog::CatalogRecord,
}
impl<R: Read + Seek> crate::HfsVolume<R> {
    /// Access a raw catalog entry by its CNID, independently of path normalization.
    pub fn entry_by_id(&mut self, identifier: u32) -> crate::Result<EntryMetadata> {
        let (record, name, parent_identifier) = crate::catalog::access::entry_by_id(
            &mut self.reader,
            &self.header,
            &self.catalog_btree_header,
            identifier,
        )?;
        let link_identifier = match &record {
            crate::catalog::CatalogRecord::File(file) => {
                crate::catalog::access::link_reference(file)
            }
            crate::catalog::CatalogRecord::Folder(_)
            | crate::catalog::CatalogRecord::FileThread(_)
            | crate::catalog::CatalogRecord::FolderThread(_) => None,
        };
        Ok(EntryMetadata {
            identifier,
            parent_identifier,
            name,
            link_identifier,
            record,
        })
    }
    /// Volume name comes from the root folder catalog record.
    pub fn volume_name(&mut self) -> crate::Result<String> {
        Ok(self.entry_by_id(2)?.name)
    }
    /// Open the logical bytes of a CNID, resolving backing hardlinks as paths do.
    pub fn open_file_by_id(&mut self, identifier: u32) -> crate::Result<FileReader<'_, R>> {
        let metadata = self.entry_by_id(identifier)?;
        let record = crate::catalog::access::resolve_link(
            &mut self.reader,
            &self.header,
            &self.catalog_btree_header,
            metadata.record,
        )?;
        let crate::catalog::CatalogRecord::File(file) = record else {
            return Err(crate::HfsPlusError::FileNotFound(format!(
                "CNID {identifier} is not a file"
            )));
        };
        if let Some(header) = self.file_compression(&file)? {
            return Ok(FileReader::Decoded(io::Cursor::new(
                self.read_compressed(&file, &header)?,
            )));
        }
        let map = crate::extents::collect_extents(
            &mut self.reader,
            &self.header,
            &self.extents_btree_header,
            &file.data_fork,
            file.file_id,
            crate::extents::FORK_TYPE_DATA,
        )?;
        Ok(FileReader::Fork(ForkReader::from_extents(
            &mut self.reader,
            &file.data_fork,
            self.header.block_size,
            &map,
        )?))
    }
    /// Extended attributes expose the same seek contract as files.
    pub fn open_xattr(
        &mut self,
        path: &str,
        name: &str,
    ) -> crate::Result<Option<io::Cursor<Vec<u8>>>> {
        Ok(self.get_xattr(path, name)?.map(io::Cursor::new))
    }
}

/// Attribute value length and complete physical allocation map; inline values have no extents.
#[derive(Debug, Clone)]
pub struct XattrMetadata {
    pub name: String,
    pub size: u64,
    pub extents: Vec<crate::volume::ExtentDescriptor>,
}
impl<R: Read + Seek> crate::HfsVolume<R> {
    /// Inspect attribute storage without reading its value bytes.
    pub fn xattr_metadata(
        &mut self,
        path: &str,
        name: &str,
    ) -> crate::Result<Option<XattrMetadata>> {
        let identifier = self.resolve_path_to_cnid(path)?;
        let Some(tree) = self.attributes_btree()? else {
            return Ok(None);
        };
        match crate::attributes::lookup(&mut self.reader, &tree, identifier, name)? {
            None => Ok(None),
            Some(crate::attributes::AttrValue::Inline(value)) => Ok(Some(XattrMetadata {
                name: name.into(),
                size: value.len() as u64,
                extents: Vec::new(),
            })),
            Some(crate::attributes::AttrValue::Fork(fork)) => {
                let extents = crate::attributes::fork_extents(
                    &mut self.reader,
                    &tree,
                    &self.header,
                    identifier,
                    name,
                    &fork,
                )?;
                Ok(Some(XattrMetadata {
                    name: name.into(),
                    size: fork.logical_size,
                    extents,
                }))
            }
        }
    }
    /// Inspect the complete physical allocation map of a resource fork.
    pub fn resource_extents(
        &mut self,
        path: &str,
    ) -> crate::Result<Vec<crate::volume::ExtentDescriptor>> {
        let file = self.resolve_path_to_file(path)?;
        crate::extents::collect_extents(
            &mut self.reader,
            &self.header,
            &self.extents_btree_header,
            &file.resource_fork,
            file.file_id,
            crate::extents::FORK_TYPE_RESOURCE,
        )
    }
}
