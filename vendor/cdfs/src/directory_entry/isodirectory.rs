// SPDX-License-Identifier: (MIT OR Apache-2.0)

#[allow(unused)]
use log::{debug, error, info, trace, warn};

use std::{
    collections::HashSet,
    convert::TryFrom,
    fmt,
    path::{Component as PathComponent, PathBuf},
    str,
};

use itertools::Itertools;

use super::{DirectoryEntry, ExtraAttributes, ExtraMeta};
use crate::{
    parse::{
        directory_entry::{DirectoryEntryHeader, FileFlags},
        susp::{
            system_use_entries, AlternateNameFlags, ChildLink, PosixAttributes, PosixTimestamp,
            SuspExtension, SymbolicLinkRecordFlags, SystemUseEntry,
        },
    },
    BlockBuffer, BlockBufferCtor, FileRef, ISO9660Reader, ISOError, Result, BLOCK_SIZE,
};

/// [`DirectoryEntry`](crate::DirectoryEntry) for directories.
///
/// # See Also
///
/// ISO-9660 / ECMA-119 § 9
pub struct ISODirectory<T: ISO9660Reader> {
    pub(crate) header: DirectoryEntryHeader,

    /// The name encoded with UTF-8.
    pub identifier: String,

    pub(super) ext: ExtraMeta,

    file: FileRef<T>,
}

impl<T: ISO9660Reader> ExtraAttributes for ISODirectory<T> {
    fn ext(&self) -> &ExtraMeta {
        &self.ext
    }

    fn header(&self) -> &DirectoryEntryHeader {
        &self.header
    }
}

impl<T: ISO9660Reader> Clone for ISODirectory<T> {
    fn clone(&self) -> ISODirectory<T> {
        ISODirectory {
            header: self.header.clone(),
            identifier: self.identifier.clone(),
            file: self.file.clone(),
            ext: self.ext.clone(),
        }
    }
}

impl<T: ISO9660Reader> fmt::Debug for ISODirectory<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("ISODirectory")
            .field("header", &self.header)
            .field("identifier", &self.identifier)
            .field("ext", &self.ext)
            .finish()
    }
}

impl<T: ISO9660Reader> ISODirectory<T> {
    pub(crate) fn new(
        header: DirectoryEntryHeader,
        ext: ExtraMeta,
        identifier: String,
        file: FileRef<T>,
    ) -> Self {
        let mut identifier = match ext.alt_name.as_ref() {
            Some(alt_name) => alt_name.clone(),
            None => identifier,
        };

        if &identifier == "\u{0}" {
            identifier = ".".to_string();
        } else if &identifier == "\u{1}" {
            identifier = "..".to_string();
        }

        ISODirectory {
            header,
            identifier,
            file,
            ext,
        }
    }

    /// Returns the number of [`BLOCK_SIZE`](crate::BLOCK_SIZE) byte blocks required to contain the directory entry.
    pub fn block_count(&self) -> u32 {
        let len = self.header.extent_length;
        let block_size = u32::from(BLOCK_SIZE);
        (len + block_size - 1) / block_size // ceil(len / block_size)
    }

    /// I'm pretty sure this doesn't need to be public and IsoFuse should just use `contents()` instead.
    pub fn read_entry_at(
        &self,
        block: &mut BlockBuffer,
        buf_block_num: &mut Option<u64>,
        offset: u64,
    ) -> Result<(DirectoryEntry<T>, Option<u64>)> {
        let blksize = u64::from(BLOCK_SIZE);
        let mut block_num = offset / blksize;
        let mut block_pos = (offset % blksize) as usize;

        if buf_block_num != &Some(block_num) {
            let lba = self.header.extent_loc as u64 + block_num;
            let count = self.file.read_at(block, lba)?;

            if count != 2048 {
                *buf_block_num = None;
                return Err(ISOError::ReadSize(count));
            }

            *buf_block_num = Some(block_num);
        }

        let (header, identifier, susp) =
            DirectoryEntryHeader::parse(&block[block_pos..], self.header.character_encoding)?;

        let (susp, cont) = match susp {
            Some(susp) => {
                susp.into_iter()
                    .fold((Vec::new(), None), |(mut susp, continuation), entry| {
                        if let SystemUseEntry::ContinuationArea(cont) = entry {
                            (susp, Some(cont))
                        } else {
                            susp.push(entry);
                            (susp, continuation)
                        }
                    })
            }
            None => (vec![], None),
        };

        // Pull in all the continuations
        let mut cont = cont;
        let mut susp = susp;
        while let Some(current_cont) = cont {
            let lba = current_cont.block_location as u64;
            let block = &mut BlockBuffer::new();
            let count = self.file.read_at(block, lba)?;

            #[cfg(feature = "assertions")]
            assert!(count == 2048 || count == usize::try_from(current_cont.length)?);

            let data = &block[0..usize::try_from(current_cont.length)?];
            let (_, mut cont_susp) = system_use_entries(data)?;

            susp.append(&mut cont_susp);

            let continuation_index = susp
                .iter()
                .position(|entry| matches!(entry, SystemUseEntry::ContinuationArea(_)));

            cont = match continuation_index {
                Some(index) => {
                    // Order is important because we might have e.g. multiple NM entries
                    if let SystemUseEntry::ContinuationArea(new_cont) = susp.remove(index) {
                        Some(new_cont)
                    } else {
                        unreachable!()
                    }
                }
                None => None,
            }
        }

        trace!("id={identifier:?}");
        for entry in susp.iter() {
            trace!("{entry:?}");
        }

        let extensions: HashSet<SuspExtension> = susp
            .iter()
            .filter_map(|entry| match entry {
                SystemUseEntry::ExtensionsReference(er) => Some(er.extensions.clone()),
                _ => None,
            })
            .next()
            .unwrap_or_else(Vec::new)
            .into_iter()
            .collect();

        // BEGIN:ROCKRIDGE
        let relocated: bool = susp
            .iter()
            .filter_map(|entry| match entry {
                SystemUseEntry::RelocatedDirectory(child_link) => Some(child_link.clone()),
                _ => None,
            })
            .next()
            .map(|_| true)
            .unwrap_or(false);

        let child_link: Option<ChildLink> = susp
            .iter()
            .filter_map(|entry| match entry {
                SystemUseEntry::ChildLink(child_link) => Some(*child_link),
                _ => None,
            })
            .next();

        let alt_name: Option<String> = susp
            .iter()
            .filter_map(|entry| match entry {
                SystemUseEntry::AlternateName(name) => Some(name),
                _ => None,
            })
            .take_while_inclusive(|name_meta| {
                name_meta.flags.contains(AlternateNameFlags::CONTINUE)
            })
            .fold(None, |acc, name| {
                let mut acc = acc.unwrap_or_default();
                acc += &name.name;
                Some(acc)
            });

        let timestamps: PosixTimestamp = susp
            .iter()
            .filter_map(|entry| match entry {
                SystemUseEntry::PosixTimestamp(timestamp) => Some(timestamp),
                _ => None,
            })
            .fold(PosixTimestamp::default(), |mut acc, new_timestamp| {
                if let Some(timestamp) = new_timestamp.creation {
                    if acc.creation.is_some() {
                        error!("duplicate ctime");
                    }
                    acc.creation = Some(timestamp);
                }

                if let Some(timestamp) = new_timestamp.modify {
                    if acc.modify.is_some() {
                        error!("duplicate mtime");
                    }
                    acc.modify = Some(timestamp);
                }

                if let Some(timestamp) = new_timestamp.access {
                    if acc.access.is_some() {
                        error!("duplicate atime");
                    }
                    acc.access = Some(timestamp);
                }

                if let Some(timestamp) = new_timestamp.attributes {
                    if acc.attributes.is_some() {
                        error!("duplicate attribute mod time");
                    }
                    acc.attributes = Some(timestamp);
                }

                acc
            });

        let symlink_target: Option<String> = susp
            .iter()
            .filter_map(|entry| match entry {
                SystemUseEntry::SymbolicLink(symlink) => Some(symlink.clone()),
                _ => None,
            })
            .take_while_inclusive(|meta| meta.should_continue)
            .map(|symlink| {
                symlink
                    .records
                    .into_iter()
                    .map(|component| {
                        if component.flags.contains(SymbolicLinkRecordFlags::ROOT) {
                            #[cfg(feature = "assertions")]
                            assert!(component.component.is_empty());

                            (&PathComponent::RootDir).into()
                        } else if component.flags.contains(SymbolicLinkRecordFlags::CURRENT) {
                            #[cfg(feature = "assertions")]
                            assert!(component.component.is_empty());

                            (&PathComponent::CurDir).into()
                        } else if component.flags.contains(SymbolicLinkRecordFlags::PARENT) {
                            #[cfg(feature = "assertions")]
                            assert!(component.component.is_empty());

                            (&PathComponent::ParentDir).into()
                        } else {
                            PathBuf::from(component.component)
                        }
                    })
                    .fold(PathBuf::new(), |acc: PathBuf, component: PathBuf| {
                        acc.join(component)
                    })
            })
            .fold(None, |acc: Option<PathBuf>, component: PathBuf| {
                Some(acc.unwrap_or_default().join(component))
            })
            .map(|target| target.to_str().unwrap().into());

        let attributes: Option<PosixAttributes> = susp
            .iter()
            .filter_map(|entry| match entry {
                SystemUseEntry::PosixAttributes(attributes) => Some(attributes.clone()),
                _ => None,
            })
            .next();

        if !extensions.is_empty() {
            trace!("Found the following extensions for {identifier:?}:");
            for extension in extensions.iter() {
                trace!("\t{extension:?}");
            }
        }
        // END:ROCKRIDGE

        // Theoretically this should/could contain e.g. Amiga or Apple specific extensions munged into a more generic representation
        let extra_meta = ExtraMeta {
            alt_name,
            symlink_target,
            attributes,
            extensions,
            timestamps,
            relocated,
        };

        block_pos += header.length as usize;

        let entry = DirectoryEntry::new(header, extra_meta, identifier, self.file.clone())?;

        // All bytes after the last directory entry are zero.
        if block_pos >= (2048 - 33) || block[block_pos] == 0 {
            block_num += 1;
            block_pos = 0;
        }

        let next_offset = if block_num < self.block_count() as u64 {
            Some(2048 * block_num + u64::try_from(block_pos)?)
        } else {
            None
        };

        // If we get a CL record we assume that we've a (dummy) regular file entry and
        // the LBA of the directory.
        if let Some(child_link) = child_link {
            match entry {
                DirectoryEntry::File(file_entry) => {
                    let mut header = file_entry.header;
                    let ext = file_entry.ext;
                    let identifier = file_entry.identifier;

                    header.file_flags = header.file_flags.union(FileFlags::DIRECTORY);
                    header.extent_loc = child_link.0;
                    // Lazy…
                    header.extent_length = 2048;

                    let new_entry = DirectoryEntry::Directory(ISODirectory::new(
                        header,
                        ext,
                        identifier,
                        self.file.clone(),
                    ));

                    Ok((new_entry, next_offset))
                }
                _ => unimplemented!("We shouldn't have a child link for a not-regular-file entry"),
            }
        } else {
            Ok((entry, next_offset))
        }
    }

    /// Returns a [`ISODirectoryIterator`], akin to POSIX.1's `readdir`.
    pub fn contents(&self) -> ISODirectoryIterator<T> {
        ISODirectoryIterator {
            directory: self,
            block: BlockBuffer::new(),
            block_num: None,
            next_offset: Some(0),
        }
    }

    /// Returns the [`DirectoryEntry`] of the matching child
    ///
    /// # Arguments
    ///
    /// * `identifier` - A valid path segment
    ///
    /// # Errors
    ///
    /// Returns an error variant if there is an I/O error reading a directory entry.  Returns Ok(None)
    /// if the path specified by `identifer` cannot be found.
    pub fn find(&self, identifier: &str) -> Result<Option<DirectoryEntry<T>>> {
        for entry in self.contents() {
            let entry = entry?;
            if entry
                .header()
                .file_flags
                .contains(FileFlags::ASSOCIATED_FILE)
            {
                continue;
            }
            if entry.identifier().eq_ignore_ascii_case(identifier) {
                return Ok(Some(entry));
            }
        }

        Ok(None)
    }

    /// Returns the [`DirectoryEntry`] matching the specified path.  Similar to `find` but takes a
    /// full path and recurses through the descendants instead of a single path segment.
    pub fn find_recursive(&self, path: &str) -> Result<Option<DirectoryEntry<T>>> {
        // TODO: avoid clone()
        let mut entry = DirectoryEntry::Directory(self.clone());
        for segment in path.split('/').filter(|x| !x.is_empty()) {
            let parent = match entry {
                DirectoryEntry::Directory(dir) => dir,
                _ => return Ok(None),
            };

            entry = match parent.find(segment)? {
                Some(entry) => entry,
                None => return Ok(None),
            };
        }

        Ok(Some(entry))
    }

    /// Returns true if Rock Ridge extensions have been detected.
    pub fn is_rock_ridge(&self) -> bool {
        // Should consider also looking for an `RR` entry.
        self.ext.extensions.contains(&SuspExtension::RockRidge1_09)
            || self.ext.extensions.contains(&SuspExtension::RockRidge1_12)
    }
}

/// Iterator for the contents of [`ISODirectory`] constructed by [`contents()`](ISODirectory::contents()).  Similar to POSIX.1's `readdir`.
pub struct ISODirectoryIterator<'a, T: ISO9660Reader> {
    directory: &'a ISODirectory<T>,
    next_offset: Option<u64>,
    block: BlockBuffer,
    block_num: Option<u64>,
}

impl<'a, T: ISO9660Reader> Iterator for ISODirectoryIterator<'a, T> {
    type Item = Result<DirectoryEntry<T>>;

    fn next(&mut self) -> Option<Result<DirectoryEntry<T>>> {
        let offset = self.next_offset?;
        match self
            .directory
            .read_entry_at(&mut self.block, &mut self.block_num, offset)
        {
            Ok((entry, next_offset)) => {
                self.next_offset = next_offset;
                Some(Ok(entry))
            }
            Err(err) => Some(Err(err)),
        }
    }
}
