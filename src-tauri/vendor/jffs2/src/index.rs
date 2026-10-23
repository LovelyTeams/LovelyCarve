//! Versioned namespace and non-overlapping fragment reconstruction.
use crate::{DataNode, DirectoryEntry, File, FileType, Fragment, Inode};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

type Namespace = BTreeMap<u32, Vec<DirectoryEntry>>;
pub(crate) fn build(
    nodes: &[DataNode],
    entries: Vec<DirectoryEntry>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(BTreeMap<u32, File>, Namespace)> {
    let mut versions: BTreeMap<u32, BTreeMap<u32, usize>> = BTreeMap::new();
    for (number, node) in nodes.iter().enumerate() {
        ensure!(!cancelled(), "JFFS2 index cancelled");
        let versions = versions.entry(node.inode.number).or_default();
        if let Some(previous) = versions.get(&node.inode.version) {
            let mut previous = nodes[*previous].clone();
            previous.source = node.source;
            ensure!(previous == *node, "JFFS2 conflicting inode version");
        } else {
            versions.insert(node.inode.version, number);
        }
    }
    let mut files = BTreeMap::new();
    let mut total_fragments = 0usize;
    for (inode, versions) in versions {
        ensure!(!cancelled(), "JFFS2 index cancelled");
        let latest = *versions
            .last_key_value()
            .context("JFFS2 missing inode version")?
            .1;
        let mut file = File {
            inode: nodes[latest].inode.clone(),
            fragments: BTreeMap::new(),
        };
        for number in versions.values() {
            ensure!(!cancelled(), "JFFS2 fragment index cancelled");
            let node = &nodes[*number];
            ensure!(
                node.inode.kind == file.inode.kind,
                "JFFS2 inode type changes across versions"
            );
            let start = u64::from(node.offset);
            let end = (start + u64::from(node.size)).min(file.inode.size);
            if start >= end {
                continue;
            }
            let first = file
                .fragments
                .range(..=start)
                .next_back()
                .map(|(start, _)| *start)
                .unwrap_or(start);
            let overlapping: Vec<_> = file
                .fragments
                .range(first..end)
                .filter(|(_, fragment)| fragment.end > start)
                .map(|(start, fragment)| (*start, fragment.clone()))
                .collect();
            for (previous_start, previous) in overlapping {
                file.fragments.remove(&previous_start);
                if previous_start < start {
                    file.fragments.insert(
                        previous_start,
                        Fragment {
                            end: start,
                            ..previous.clone()
                        },
                    );
                }
                if previous.end > end {
                    file.fragments.insert(
                        end,
                        Fragment {
                            within: previous.within + (end - previous_start) as usize,
                            ..previous
                        },
                    );
                }
            }
            file.fragments.insert(
                start,
                Fragment {
                    end,
                    node: *number,
                    within: 0,
                },
            );
        }
        total_fragments += file.fragments.len();
        ensure!(
            total_fragments <= 2_000_000,
            "JFFS2 fragment count exceeds limit"
        );
        files.insert(inode, file);
    }
    let mut names: BTreeMap<(u32, String), DirectoryEntry> = BTreeMap::new();
    for entry in entries {
        ensure!(!cancelled(), "JFFS2 namespace cancelled");
        let key = (entry.parent, entry.name.clone());
        if let Some(previous) = names.get(&key) {
            if previous.version > entry.version {
                continue;
            }
            if previous.version == entry.version {
                ensure!(*previous == entry, "JFFS2 conflicting directory version");
                continue;
            }
        }
        names.insert(key, entry);
    }
    let mut children: Namespace = BTreeMap::new();
    let mut directory_times: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
    for entry in names.into_values() {
        let latest = directory_times
            .entry(entry.parent)
            .or_insert((entry.version, entry.modified));
        if entry.version > latest.0 {
            *latest = (entry.version, entry.modified);
        }
        if entry.inode != 0 {
            children.entry(entry.parent).or_default().push(entry);
        }
    }
    if let std::collections::btree_map::Entry::Vacant(root) = files.entry(1) {
        ensure!(
            directory_times.contains_key(&1),
            "JFFS2 missing root inode and root dirents"
        );
        root.insert(File {
            inode: Inode {
                number: 1,
                version: 0,
                kind: FileType::Directory,
                mode: 0o40755,
                uid: 0,
                gid: 0,
                size: 0,
                accessed: 0,
                modified: 0,
                changed: 0,
                implicit_root: true,
            },
            fragments: BTreeMap::new(),
        });
    }
    ensure!(
        files[&1].inode.kind == FileType::Directory,
        "JFFS2 root is not a directory"
    );
    for (parent, (version, modified)) in directory_times {
        if let Some(file) = files.get_mut(&parent)
            && file.inode.kind == FileType::Directory
            && version > file.inode.version
        {
            file.inode.modified = modified;
            file.inode.changed = modified;
        }
    }
    let mut pending = vec![(1, 0)];
    let mut directories = BTreeSet::new();
    while let Some((parent, depth)) = pending.pop() {
        ensure!(!cancelled(), "JFFS2 namespace cancelled");
        ensure!(
            depth <= 256 && directories.insert(parent),
            "JFFS2 directory cycle/hardlink or depth limit"
        );
        for entry in children.get(&parent).map(Vec::as_slice).unwrap_or_default() {
            let inode = &files
                .get(&entry.inode)
                .context("JFFS2 live dirent refers to missing inode")?
                .inode;
            ensure!(
                entry.kind.is_none_or(|kind| kind == inode.kind),
                "JFFS2 dirent and inode type mismatch"
            );
            if inode.kind == FileType::Directory {
                pending.push((entry.inode, depth + 1));
            }
        }
    }
    // Unlinked historical subtrees are not projected into the live namespace.
    children.retain(|parent, _| directories.contains(parent));
    Ok((files, children))
}
