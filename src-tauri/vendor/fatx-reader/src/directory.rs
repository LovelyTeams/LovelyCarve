use std::collections::HashSet;

use chrono::NaiveDate;
use lamfold::{BlockSource, DirEntry, FileKind, FoldError, LocalFileTimes, NodeId, Result};

use crate::{Fatx, Format, Node, require};

const DIRENT_BYTES: usize = 64;
const DIRECTORY_SCAN_LIMIT: u64 = 32 * 1024 * 1024;

impl<S: BlockSource> Fatx<S> {
    pub(crate) fn entries(&mut self, dir: NodeId) -> Result<&[DirEntry]> {
        if !self.directories.contains_key(&dir) {
            let node = self.node(dir)?;
            if node.kind != FileKind::Directory {
                return Err(FoldError::NotDirectory);
            }
            let records = self.read_directory(node.cluster)?;
            let mut child_roots = HashSet::new();
            for (dirent, node) in &records {
                require(
                    !self.nodes.contains_key(&dirent.node),
                    "FATX directories share a record",
                )?;
                if node.kind == FileKind::Directory {
                    require(
                        !self.directory_roots.contains(&node.cluster)
                            && child_roots.insert(node.cluster),
                        "FATX directory has multiple references",
                    )?;
                }
            }
            self.directory_roots.extend(child_roots);
            self.nodes
                .extend(records.iter().map(|(entry, node)| (entry.node, *node)));
            self.directories
                .insert(dir, records.into_iter().map(|(entry, _)| entry).collect());
        }
        Ok(&self.directories[&dir])
    }

    fn read_directory(&mut self, start: u32) -> Result<Vec<(DirEntry, Node)>> {
        let geometry = self.geometry;
        let mut entries = Vec::new();
        let mut visited = HashSet::new();
        let mut names = HashSet::new();
        let mut bytes = vec![0; geometry.cluster_size as usize];
        let mut cluster = start;
        loop {
            require(
                (visited.len() as u64 + 1) * geometry.cluster_size <= DIRECTORY_SCAN_LIMIT,
                "FATX directory scan limit exceeded",
            )?;
            require(visited.insert(cluster), "FATX circular directory chain")?;
            let offset = geometry.cluster_offset(cluster)?;
            self.source.read_at(offset, &mut bytes)?;
            for (index, record) in bytes.as_chunks::<DIRENT_BYTES>().0.iter().enumerate() {
                let name_len = usize::from(record[0]);
                match name_len {
                    0 | 0xff => return Ok(entries),
                    0xe5 => continue,
                    1..=42 => (),
                    _ => return Err(FoldError::Corrupt("FATX invalid filename length")),
                }
                let name: String = record[2..2 + name_len]
                    .iter()
                    .copied()
                    .map(char::from)
                    .collect();
                require(
                    !name.contains(['/', '\\', '\0'])
                        && !matches!(name.as_str(), "." | "..")
                        && names.insert(name.to_ascii_uppercase()),
                    "FATX invalid or duplicate filename",
                )?;
                let kind = if record[1] & 0x10 != 0 {
                    FileKind::Directory
                } else {
                    FileKind::Regular
                };
                let cluster = geometry.format.dword(&record[44..]);
                let size = geometry.format.dword(&record[48..]);
                if kind == FileKind::Directory || size != 0 || cluster != 0 {
                    geometry.cluster_offset(cluster)?;
                }
                let node = Node {
                    cluster,
                    size,
                    kind,
                    times: timestamps(geometry.format, record),
                };
                entries.push((
                    DirEntry {
                        name,
                        node: offset + (index * DIRENT_BYTES) as u64,
                        kind,
                    },
                    node,
                ));
            }
            match self.next_cluster(cluster)? {
                Some(next) => cluster = next,
                None => return Ok(entries),
            }
        }
    }
}

fn timestamps(format: Format, record: &[u8; DIRENT_BYTES]) -> LocalFileTimes {
    let decode = |offset: usize| {
        let value = format.dword(&record[offset..]);
        let date = value >> 16;
        let time = value & 0xffff;
        let year = if format == Format::Fatx { 2000 } else { 1980 } + (date >> 9) as i32;
        let day = NaiveDate::from_ymd_opt(year, (date >> 5) & 15, date & 31)?;
        let wall = day.and_hms_opt(time >> 11, (time >> 5) & 63, (time & 31) * 2)?;
        Some((wall.and_utc().timestamp(), 0))
    };
    let (created, modified) = if format == Format::Fatx {
        (56, 52)
    } else {
        (52, 56)
    };
    LocalFileTimes {
        created: decode(created),
        modified: decode(modified),
        accessed: decode(60),
        changed: None,
    }
}
