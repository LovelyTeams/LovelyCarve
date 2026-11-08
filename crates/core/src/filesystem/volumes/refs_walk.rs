//! Single traversal shared by live ReFS metadata and volume snapshots.
use crate::filesystem::tree::fs_tree::Mactb;
use anyhow::Result;
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

pub const REFS_ROOT_OBJECT_ID: u64 = 0x600;

#[derive(Debug, Clone)]
pub struct WalkEntry {
    pub parent_object_id: u64,
    pub object_id: u64,
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
    pub inode: Option<u64>,
    pub timestamps: Mactb,
    pub link_target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WalkStream {
    pub owner_parent_object_id: u64,
    pub owner_name: String,
    pub stream_name: String,
    pub size: u64,
}

#[derive(Debug, Default)]
pub struct WalkResult {
    pub entries: Vec<WalkEntry>,
    pub streams: Vec<WalkStream>,
}

pub(super) fn collect(
    volume: &refs_reader::Volume,
    cancel: &CancellationToken,
) -> Result<WalkResult> {
    let mut pending = vec![REFS_ROOT_OBJECT_ID];
    let mut visited = HashSet::new();
    let mut output = WalkResult::default();
    let check = || {
        if cancel.is_cancelled() {
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "snapshot build cancelled",
            ))
        } else {
            Ok(())
        }
    };
    while let Some(object_id) = pending.pop() {
        anyhow::ensure!(!cancel.is_cancelled(), "snapshot build cancelled");
        if !visited.insert(object_id) {
            continue;
        }
        for entry in volume.list_directory_checked(object_id, &check)? {
            anyhow::ensure!(!cancel.is_cancelled(), "snapshot build cancelled");
            let directory_id = entry.directory_id;
            if let Some(id) = directory_id {
                pending.push(id);
            } else {
                for stream in volume.list_streams_checked(&entry, &check)? {
                    if stream.is_snapshot {
                        output.streams.push(WalkStream {
                            owner_parent_object_id: object_id,
                            owner_name: entry.name.clone(),
                            stream_name: stream.name,
                            size: stream.size,
                        });
                    }
                }
            }
            output.entries.push(WalkEntry {
                parent_object_id: object_id,
                object_id: directory_id.unwrap_or(0),
                name: entry.name.clone(),
                is_directory: directory_id.is_some(),
                size: entry.size,
                inode: entry.inode,
                timestamps: super::refs::entry_timestamps(&entry),
                link_target: entry.symlink_target,
            });
        }
    }
    Ok(output)
}
