use std::collections::HashSet;

use lamfold::{BlockSource, DirEntry, FileKind, FoldError, NodeId, Result};
use xdvdfs::layout::{DirectoryEntryDiskNode, DirectoryEntryNode, DiskRegion};

use crate::{Entry, SECTOR_SIZE, Xdvdfs, region_offset, require};

const RECORD_SIZE: usize = 14;
const MAX_CHILD_OFFSET: usize = 4 * (u16::MAX as usize - 1);
const MAX_ENTRY_END: usize = MAX_CHILD_OFFSET + SECTOR_SIZE;

impl<S: BlockSource> Xdvdfs<S> {
    pub(crate) fn entries(&mut self, dir: NodeId) -> Result<&[DirEntry]> {
        if !self.directories.contains_key(&dir) {
            let entry = self.node(dir)?;
            if entry.kind != FileKind::Directory {
                return Err(FoldError::NotDirectory);
            }
            let records = self.read_table(entry.region)?;
            let mut sectors = HashSet::new();
            for (dirent, entry) in &records {
                require(
                    !self.nodes.contains_key(&dirent.node),
                    "XDVDFS directory tables share a record",
                )?;
                if entry.kind == FileKind::Directory && entry.region.size != 0 {
                    let sector = entry.region.sector;
                    require(
                        !self.directory_sectors.contains(&sector) && sectors.insert(sector),
                        "XDVDFS directory table has multiple references",
                    )?;
                }
            }
            self.directory_sectors.extend(sectors);
            self.nodes
                .extend(records.iter().map(|(dirent, entry)| (dirent.node, *entry)));
            self.directories
                .insert(dir, records.into_iter().map(|(dirent, _)| dirent).collect());
        }
        Ok(&self.directories[&dir])
    }

    fn read_table(&mut self, region: DiskRegion) -> Result<Vec<(DirEntry, Entry)>> {
        if region.size == 0 {
            return Ok(Vec::new());
        }
        let base = region_offset(region, self.source.len())?;
        let mut bytes = vec![0; (region.size as usize).min(MAX_ENTRY_END)];
        self.source.read_at(base, &mut bytes)?;

        let mut pending = vec![0];
        let mut visited = HashSet::new();
        let mut names = HashSet::new();
        let mut ranges = Vec::new();
        let mut entries = Vec::new();
        while let Some(offset) = pending.pop() {
            require(
                visited.insert(offset),
                "XDVDFS repeated directory tree offset",
            )?;
            let record: &[u8; RECORD_SIZE] = bytes
                .get(offset..offset + RECORD_SIZE)
                .and_then(|record| record.try_into().ok())
                .ok_or(FoldError::Corrupt("XDVDFS directory record outside table"))?;
            if *record == [0xff; RECORD_SIZE] || *record == [0; RECORD_SIZE] {
                require(offset == 0, "XDVDFS directory tree points to padding")?;
                break;
            }
            let node = DirectoryEntryDiskNode::deserialize::<()>(record)
                .map_err(|_| FoldError::Corrupt("XDVDFS directory record encoding"))?;
            let name_len = usize::from(node.dirent.filename_length);
            let end = offset + RECORD_SIZE + name_len;
            require(
                name_len != 0
                    && end <= bytes.len()
                    && offset / SECTOR_SIZE == (end - 1) / SECTOR_SIZE,
                "XDVDFS directory name outside table or sector",
            )?;
            let mut dirent = DirectoryEntryNode {
                node,
                name: [0; 256],
                offset: base + offset as u64,
            };
            dirent.name[..name_len].copy_from_slice(&bytes[offset + RECORD_SIZE..end]);
            let name = dirent
                .name_str::<()>()
                .map_err(|_| FoldError::Unsupported("XDVDFS filename encoding"))?
                .into_owned();
            require(
                !name.contains(['/', '\\', '\0'])
                    && !matches!(name.as_str(), "." | "..")
                    && names.insert(name.to_ascii_uppercase()),
                "XDVDFS invalid or duplicate filename",
            )?;
            let kind = if node.dirent.is_directory() {
                FileKind::Directory
            } else {
                FileKind::Regular
            };
            region_offset(node.dirent.data, self.source.len())?;
            let entry = Entry {
                region: node.dirent.data,
                kind,
            };
            entries.push((
                DirEntry {
                    name,
                    node: dirent.offset,
                    kind,
                },
                entry,
            ));
            ranges.push(offset..end);
            for child in [node.right_entry_offset, node.left_entry_offset] {
                if child != 0 && child != u16::MAX {
                    pending.push(usize::from(child) * 4);
                }
            }
        }
        ranges.sort_unstable_by_key(|range| range.start);
        require(
            ranges.windows(2).all(|pair| pair[0].end <= pair[1].start),
            "XDVDFS directory records overlap",
        )?;
        Ok(entries)
    }
}
