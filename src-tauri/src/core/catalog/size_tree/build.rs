//! Reading one volume index into a [`SizeTree`].

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::Connection;

use super::{
    child_slice, DirectoryTally, EntryKind, SizeNode, SizeTree, Tally, NO_ORDINAL, NO_TALLY,
    NTFS_NODE_PREFIX, NTFS_STREAM_PREFIX, UNKNOWN_TIME,
};
use crate::core::catalog::filedb::filedb_file;
use crate::core::filesystem::meta_addr::IS_DELETE_DELETED;

/// MFT record of `$BadClus`. Its `$Bad` stream is sparse and as long as the
/// volume: counted, it would double every total.
const BAD_CLUSTERS_RECORD: i64 = 8;
/// A modification this far past the build time still counts as plausible;
/// anything later is a bad clock or a corrupt record, and is not the
/// volume's newest write.
const FUTURE_SLACK_SECONDS: i64 = 86_400;
/// Longest extension interned. Longer "extensions" are parts of names.
const MAX_EXTENSION_LEN: usize = 16;
/// A node index meaning "none".
const NO_NODE: u32 = u32::MAX;

/// Read the index at `index` into a [`SizeTree`]. `now` (Unix seconds) bounds
/// which modification times are plausible.
pub fn build_size_tree(index: &Path, now: i64) -> anyhow::Result<SizeTree> {
    let conn = filedb_file::open_reader(index)?;
    let meta = filedb_file::read_meta(&conn)?;
    let directory_numbers = number_directories(&conn, &meta.volume_key)?;
    let plausible_until = now.saturating_add(FUTURE_SLACK_SECONDS);

    let mut nodes = vec![SizeNode {
        ordinal: NO_ORDINAL,
        size: 0,
        modified: UNKNOWN_TIME,
        tally: NO_TALLY,
        extension: 0,
        kind: EntryKind::Directory,
        deleted: false,
    }];
    // The directory number each node's row names as its parent.
    let mut parent_numbers: Vec<u32> = vec![NO_NODE];
    // The node standing for each directory number. A directory listed twice
    // keeps its first row: that is the one the walk descended into.
    let mut directory_nodes: Vec<u32> = vec![NO_NODE; directory_numbers.len()];
    if let Some(root) = directory_nodes.first_mut() {
        *root = 0;
    }
    let mut extensions = ExtensionTable::new();
    let mut charged = ChargedFiles::default();
    let mut newest_write = UNKNOWN_TIME;
    let mut unattached = 0u64;

    let mut statement = conn.prepare(
        "SELECT ordinal, nid, pid, kind, size, mtime, is_delete, extension, meta_addr,
                meta_addr_high
         FROM filedb
         ORDER BY ordinal",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let Some(&parent_number) = directory_numbers.get(row.get_ref(2)?.as_str()?) else {
            unattached += 1;
            continue;
        };
        let nid = row.get_ref(1)?.as_str()?;
        let kind = match row.get_ref(3)?.as_str()? {
            "dir" => EntryKind::Directory,
            "file" => EntryKind::File,
            "symlink" => EntryKind::Symlink,
            "unallocated" => EntryKind::Unallocated,
            other => anyhow::bail!("unknown entry kind {other:?} in {}", index.display()),
        };
        let stream = nid.starts_with(NTFS_STREAM_PREFIX);
        let meta_addr: i64 = row.get(8)?;
        if stream && meta_addr == BAD_CLUSTERS_RECORD {
            continue;
        }
        let deleted = row.get::<_, i64>(6)? == IS_DELETE_DELETED;
        let logical_size = u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0);
        let size = match kind {
            EntryKind::File => {
                // One file named in several directories is charged once.
                let first_name = match ntfs_volume_of(nid) {
                    Some(volume) => charged.first_name(volume, meta_addr, row.get(9)?),
                    None => true,
                };
                if first_name {
                    logical_size
                } else {
                    0
                }
            }
            EntryKind::Unallocated => logical_size,
            EntryKind::Directory | EntryKind::Symlink => 0,
        };
        let modified = row
            .get_ref(5)?
            .as_str_or_null()?
            .and_then(unix_seconds)
            .unwrap_or(UNKNOWN_TIME);
        if kind == EntryKind::File && !deleted && modified <= plausible_until {
            newest_write = newest_write.max(modified);
        }
        // A stream's "extension" is the tail of `name.ext:stream (数据流)`.
        let extension = match row.get_ref(7)?.as_str_or_null()? {
            Some(extension) if !stream => extensions.intern(extension),
            _ => 0,
        };
        let node_index = u32::try_from(nodes.len())
            .ok()
            .filter(|index| *index != NO_NODE)
            .ok_or_else(|| anyhow::anyhow!("{} holds too many rows", index.display()))?;
        if kind == EntryKind::Directory {
            if let Some(slot) = directory_numbers
                .get(nid)
                .and_then(|&number| directory_nodes.get_mut(number as usize))
            {
                if *slot == NO_NODE {
                    *slot = node_index;
                }
            }
        }
        nodes.push(SizeNode {
            ordinal: row.get(0)?,
            size,
            modified,
            tally: NO_TALLY,
            extension,
            kind,
            deleted,
        });
        parent_numbers.push(parent_number);
    }
    drop(rows);
    drop(statement);

    // Compressed child lists, in walk order. The root has no parent, and a row
    // whose parent directory never got a node of its own is left out.
    let parents: Vec<u32> = parent_numbers
        .iter()
        .map(|&number| {
            directory_nodes
                .get(number as usize)
                .copied()
                .unwrap_or(NO_NODE)
        })
        .collect();
    drop(parent_numbers);
    // Each parent's child count lands one slot to its right, so the running
    // sum turns the counts into start offsets.
    let mut child_start = vec![0u32; nodes.len() + 1];
    for &parent in parents.iter().filter(|&&parent| parent != NO_NODE) {
        if let Some(count) = child_start.get_mut(parent as usize + 1) {
            *count += 1;
        }
    }
    let mut running = 0u32;
    for start in &mut child_start {
        running += *start;
        *start = running;
    }
    let mut fill = child_start.clone();
    let mut children = vec![0u32; running as usize];
    for (node, &parent) in parents.iter().enumerate() {
        if parent == NO_NODE {
            continue;
        }
        let Some(next) = fill.get_mut(parent as usize) else {
            continue;
        };
        if let Some(slot) = children.get_mut(*next as usize) {
            *slot = node as u32;
            *next += 1;
        }
    }
    drop(parents);
    drop(fill);

    let tallies = aggregate(&mut nodes, &child_start, &children);
    let directories: HashMap<Box<str>, u32> = directory_numbers
        .into_iter()
        .filter_map(|(nid, number)| {
            let node = *directory_nodes.get(number as usize)?;
            let tallied = nodes.get(node as usize)?.tally != NO_TALLY;
            tallied.then_some((nid, node))
        })
        .collect();
    let unreachable = nodes
        .iter()
        .filter(|node| node.kind == EntryKind::Directory && node.tally == NO_TALLY)
        .count();
    if unattached > 0 || unreachable > 0 {
        tracing::warn!(
            "size tree of {}: {unattached} rows name no indexed parent, \
             {unreachable} directories are not reachable from the volume root",
            index.display()
        );
    }
    Ok(SizeTree {
        nodes,
        child_start,
        children,
        tallies,
        directories,
        extensions: extensions.into_names(),
        newest_write,
        truncated: meta.truncated,
        built_at: meta.built_at,
    })
}

/// Number every directory row, the volume root (`volume_key`) first.
fn number_directories(
    conn: &Connection,
    volume_key: &str,
) -> anyhow::Result<HashMap<Box<str>, u32>> {
    let mut numbers: HashMap<Box<str>, u32> = HashMap::new();
    numbers.insert(volume_key.into(), 0);
    let mut statement = conn.prepare("SELECT nid FROM filedb WHERE kind = 'dir'")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let nid = row.get_ref(0)?.as_str()?;
        if !numbers.contains_key(nid) {
            let number = u32::try_from(numbers.len())?;
            numbers.insert(nid.into(), number);
        }
    }
    Ok(numbers)
}

/// Sum every directory's subtree, children before parents, and give each
/// directory reachable from the root its tally. A row whose parent chain
/// never reaches the root (a corrupt, self-referencing chain) is never
/// visited and keeps [`NO_TALLY`].
fn aggregate(nodes: &mut [SizeNode], child_start: &[u32], children: &[u32]) -> Vec<DirectoryTally> {
    let mut tallies: Vec<DirectoryTally> = Vec::new();
    let mut stack: Vec<(u32, bool)> = vec![(0, false)];
    while let Some((node, summed_children)) = stack.pop() {
        if !summed_children {
            stack.push((node, true));
            for &child in child_slice(child_start, children, node) {
                if nodes[child as usize].kind == EntryKind::Directory {
                    stack.push((child, false));
                }
            }
            continue;
        }
        let mut tally = DirectoryTally::default();
        for &child in child_slice(child_start, children, node) {
            let entry = nodes[child as usize];
            let (live, all) = match entry.kind {
                EntryKind::Directory => {
                    let below = tallies
                        .get(entry.tally as usize)
                        .copied()
                        .unwrap_or_default();
                    let counted = |tally: Tally| Tally {
                        dirs: tally.dirs.saturating_add(1),
                        ..tally
                    };
                    (counted(below.live), counted(below.all))
                }
                EntryKind::File => {
                    let own = Tally {
                        bytes: entry.size,
                        files: 1,
                        dirs: 0,
                        newest: entry.modified,
                    };
                    (own, own)
                }
                EntryKind::Unallocated => {
                    let own = Tally {
                        bytes: entry.size,
                        ..Tally::default()
                    };
                    (own, own)
                }
                EntryKind::Symlink => (Tally::default(), Tally::default()),
            };
            tally.all.add(all);
            if !entry.deleted {
                tally.live.add(live);
            }
        }
        nodes[node as usize].tally = tallies.len() as u32;
        tallies.push(tally);
    }
    tallies
}

/// The volume part of an NTFS node id — `part:2` of `ntfs:part:2:1234` and of
/// `ntfs-ads:part:2:1234:<stream>` — so records of two volumes in one index
/// never meet.
pub(super) fn ntfs_volume_of(nid: &str) -> Option<&str> {
    if let Some(rest) = nid.strip_prefix(NTFS_NODE_PREFIX) {
        return rest.rsplit_once(':').map(|(volume, _record)| volume);
    }
    let rest = nid.strip_prefix(NTFS_STREAM_PREFIX)?;
    let (owner, _stream) = rest.rsplit_once(':')?;
    owner.rsplit_once(':').map(|(volume, _record)| volume)
}

/// Unix seconds of an RFC 3339 timestamp, as the index stores them.
fn unix_seconds(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.timestamp())
}

/// Lowercase extensions, each stored once.
struct ExtensionTable {
    ids: HashMap<Box<str>, u16>,
    names: Vec<Box<str>>,
}

impl ExtensionTable {
    fn new() -> Self {
        Self {
            ids: HashMap::new(),
            names: vec!["".into()],
        }
    }

    /// The id of `extension`; `0` (none) for noise and once the table is full.
    fn intern(&mut self, extension: &str) -> u16 {
        if extension.is_empty() || extension.len() > MAX_EXTENSION_LEN {
            return 0;
        }
        if let Some(&id) = self.ids.get(extension) {
            return id;
        }
        let Ok(id) = u16::try_from(self.names.len()) else {
            return 0;
        };
        self.ids.insert(extension.into(), id);
        self.names.push(extension.into());
        id
    }

    fn into_names(self) -> Vec<Box<str>> {
        self.names
    }
}

/// Files already charged, by volume, MFT record and stream. Records are small
/// numbers, so a main stream takes one bit; a named stream, or a record past
/// the bitmap, goes into a set.
#[derive(Default)]
struct ChargedFiles {
    volumes: HashMap<Box<str>, usize>,
    bitmaps: Vec<Vec<u64>>,
    others: HashSet<(usize, i64, i64)>,
}

impl ChargedFiles {
    /// Records the bitmap covers: 128 million, 16 MiB at most.
    const BITMAP_RECORDS: i64 = 1 << 27;

    /// Whether this is the first name met for the file.
    fn first_name(&mut self, volume: &str, record: i64, stream: i64) -> bool {
        let volume = match self.volumes.get(volume) {
            Some(&known) => known,
            None => {
                let next = self.bitmaps.len();
                self.volumes.insert(volume.into(), next);
                self.bitmaps.push(Vec::new());
                next
            }
        };
        if stream == 0 && (0..Self::BITMAP_RECORDS).contains(&record) {
            if let Some(bitmap) = self.bitmaps.get_mut(volume) {
                let word = (record / 64) as usize;
                let bit = 1u64 << (record % 64);
                if bitmap.len() <= word {
                    bitmap.resize(word + 1, 0);
                }
                if let Some(bits) = bitmap.get_mut(word) {
                    let first = *bits & bit == 0;
                    *bits |= bit;
                    return first;
                }
            }
        }
        self.others.insert((volume, record, stream))
    }
}
