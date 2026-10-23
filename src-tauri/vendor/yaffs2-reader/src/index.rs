use crate::{DirectoryEntry, FileType, Object, cancelled};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};
use tokio_util::sync::CancellationToken;

pub(crate) fn build(
    objects: &BTreeMap<u32, Object>,
    token: &CancellationToken,
) -> Result<BTreeMap<u32, Vec<DirectoryEntry>>> {
    let mut directories: BTreeMap<u32, Vec<DirectoryEntry>> = BTreeMap::new();
    let mut names = BTreeSet::new();
    let mut visible = BTreeSet::from([1]);
    for (&number, object) in objects {
        cancelled(token)?;
        if number == 1 || object.node.hidden() {
            continue;
        }
        let mut chain = BTreeSet::new();
        let mut current = number;
        let mut hidden = false;
        while current != 1 {
            ensure!(
                chain.len() < 256 && chain.insert(current),
                "YAFFS2 parent cycle/depth limit"
            );
            let node = &objects
                .get(&current)
                .context("YAFFS2 missing parent/object header")?
                .node;
            if node.hidden() {
                hidden = true;
                break;
            }
            let parent = &objects
                .get(&node.parent_id)
                .context("YAFFS2 missing parent directory")?
                .node;
            ensure!(
                parent.kind == FileType::Directory,
                "YAFFS2 parent is not a directory"
            );
            current = node.parent_id;
        }
        if !hidden {
            visible.insert(number);
        }
    }
    for &number in &visible {
        cancelled(token)?;
        let node = &objects[&number].node;
        if number != 1 {
            ensure!(
                names.insert((node.parent_id, node.name.clone())),
                "YAFFS2 duplicate live directory name"
            );
            directories
                .entry(node.parent_id)
                .or_default()
                .push(DirectoryEntry {
                    name: node.name.clone(),
                    object_id: number,
                });
        }
        if node.kind == FileType::HardLink {
            let mut target = number;
            let mut visited = BTreeSet::new();
            loop {
                ensure!(
                    visited.len() < 256 && visited.insert(target),
                    "YAFFS2 hard-link cycle/depth limit"
                );
                let node = &objects
                    .get(&target)
                    .context("YAFFS2 dangling hard link")?
                    .node;
                ensure!(
                    visible.contains(&target),
                    "YAFFS2 hard link targets a hidden object"
                );
                if node.kind != FileType::HardLink {
                    ensure!(
                        node.kind != FileType::Directory,
                        "YAFFS2 directory hard link is invalid"
                    );
                    break;
                }
                target = node.equivalent;
            }
        }
    }
    for entries in directories.values_mut() {
        entries.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Ok(directories)
}
