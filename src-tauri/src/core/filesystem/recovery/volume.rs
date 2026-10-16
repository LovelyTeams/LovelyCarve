//! Per-volume enumeration and logical streaming, including lost volumes.
use super::*;
use crate::core::filesystem::volumes::{
    exfat::{parse_exfat_id, ExfatVolume},
    fat::{parse_fat_id, FatVolume},
    ntfs::{parse_ntfs_id, NtfsVolume},
};

pub(super) enum Reader<'a> {
    Fat(FatVolume),
    Exfat(ExfatVolume),
    Ntfs(NtfsVolume<'a>),
}
impl<'a> Reader<'a> {
    pub(super) fn open(image: &'a Arc<DiskImage>, recovered: &RecoveryVolume) -> Result<Self> {
        let partition = &recovered.partition;
        let sector = image.sector_size as u64;
        let end = partition
            .start_lba
            .checked_add(partition.length_lba)
            .and_then(|end| end.checked_mul(sector))
            .context("Recovery volume bounds overflow")?;
        if partition.length_lba == 0 || end > image.len() {
            bail!("Recovery volume is outside the evidence");
        }
        match partition.kind {
            PartitionKind::Fat12 | PartitionKind::Fat16 | PartitionKind::Fat32 => {
                Ok(Self::Fat(FatVolume::open(Arc::clone(image), partition)?))
            }
            PartitionKind::ExFat => Ok(Self::Exfat(ExfatVolume::open(
                Arc::clone(image),
                partition,
            )?)),
            PartitionKind::Ntfs => Ok(Self::Ntfs(match &recovered.reconstruction {
                Some(geometry) => NtfsVolume::open_recovered(image, partition, geometry)?,
                None => NtfsVolume::open(image, partition)?,
            })),
            _ => bail!("Unsupported recovery filesystem"),
        }
    }
    pub(super) fn stream(
        &self,
        node: &FsNode,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        match self {
            Self::Fat(volume) => volume.stream_file(
                &parse_fat_id(&node.id).context("Invalid FAT candidate")?.1,
                1024 * 1024,
                visitor,
            ),
            Self::Exfat(volume) => volume.stream_file(
                &parse_exfat_id(&node.id)
                    .context("Invalid exFAT candidate")?
                    .1,
                1024 * 1024,
                visitor,
            ),
            Self::Ntfs(volume) => volume.stream_file(
                parse_ntfs_id(&node.id).context("Invalid NTFS candidate")?.1,
                1024 * 1024,
                visitor,
            ),
        }
    }
}

pub(super) fn scan_volume(
    image: &Arc<DiskImage>,
    recovered: &RecoveryVolume,
    options: &RecoveryOptions,
    volume_index: usize,
    output: &mut RecoveryScan,
    cancel: &CancellationToken,
    progress: &mut dyn FnMut(u64, Option<u64>, &str),
) -> Result<bool> {
    let candidates = &mut output.candidates;
    let directories = &mut output.directories;
    let reader = Reader::open(image, recovered)?;
    if let Reader::Ntfs(volume) = &reader {
        let (edges, _, truncated) = volume.scan_tree_edges(200_000, cancel, &mut |done| {
            progress(done, None, "Scanning NTFS MFT")
        })?;
        // Keep record identities even when the parent chain has been lost.
        let parents = edges
            .iter()
            .filter(|edge| edge.node.kind == NodeKind::Dir)
            .map(|edge| {
                (
                    edge.record_number,
                    (edge.parent_record_number, edge.node.name.clone()),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        for edge in edges {
            check_cancel(cancel)?;
            let mut names = vec![edge.node.name.clone()];
            let mut parent = edge.parent_record_number;
            let mut seen = HashSet::new();
            while parent != 5 && seen.insert(parent) && names.len() < 64 {
                let Some((next, name)) = parents.get(&parent) else {
                    names.push(format!("$Parent-{parent}"));
                    break;
                };
                names.push(name.clone());
                parent = *next;
            }
            // Zero-length system default streams have no body to restore;
            // their named streams are not ordinary file recovery candidates.
            if recovered.reconstruction.is_some()
                && edge.node.kind == NodeKind::File
                && edge.node.size == 0
                && (edge.record_number < 16 || seen.contains(&11))
            {
                continue;
            }
            names.reverse();
            if edge.node.kind == NodeKind::Dir && directories.len() < MAX_CANDIDATES {
                directories.push(RecoveryDirectory {
                    source_path: None,
                    source_node: None,
                    volume_index,
                    path: format!("/{}", names.join("/")),
                    node: edge.node.clone(),
                });
            }
            add_candidate(
                edge.node,
                None,
                format!("/{}", names.join("/")),
                volume_index,
                options,
                candidates,
            );
            if candidates.len() >= MAX_CANDIDATES {
                return Ok(true);
            }
        }
        return Ok(truncated);
    }
    let mut pending = vec![(String::new(), 0usize)];
    let mut visited = HashSet::new();
    let mut count = 0u64;
    let mut truncated = false;
    while let Some((path, depth)) = pending.pop() {
        check_cancel(cancel)?;
        if !visited.insert(path.clone()) {
            continue;
        }
        let children = match &reader {
            Reader::Fat(volume) => volume
                .children_with_deleted_strict(&path)
                .map(|nodes| nodes.into_iter().map(|node| (node, None)).collect()),
            Reader::Exfat(volume) => volume.recovery_children(&path),
            Reader::Ntfs(_) => unreachable!(),
        }
        .with_context(|| format!("Cannot enumerate directory /{path}"))?;
        for (node, initialized_size) in children {
            check_cancel(cancel)?;
            count += 1;
            if count > 200_000 {
                return Ok(true);
            }
            let child_path = if path.is_empty() {
                node.name.clone()
            } else {
                format!("{path}/{}", node.name)
            };
            if node.kind == NodeKind::Dir && !node.deleted {
                if depth >= 64 {
                    truncated = true;
                } else {
                    pending.push((child_path.clone(), depth + 1));
                }
            }
            if node.kind == NodeKind::Dir && directories.len() < MAX_CANDIDATES {
                directories.push(RecoveryDirectory {
                    source_path: None,
                    source_node: None,
                    volume_index,
                    path: format!("/{child_path}"),
                    node: node.clone(),
                });
            }
            add_candidate(
                node,
                initialized_size,
                format!("/{child_path}"),
                volume_index,
                options,
                candidates,
            );
            if candidates.len() >= MAX_CANDIDATES {
                return Ok(true);
            }
        }
        progress(count, None, "Scanning filesystem directory records");
    }
    Ok(truncated)
}

fn add_candidate(
    node: FsNode,
    initialized_size: Option<u64>,
    path: String,
    volume_index: usize,
    options: &RecoveryOptions,
    candidates: &mut Vec<RecoveryCandidate>,
) {
    if node.kind != NodeKind::File || (!node.deleted && !options.include_existing) {
        return;
    }
    let readable =
        node.size == 0 || !node.id.starts_with("deleted:") || node.id.starts_with("deleted:run:");
    candidates.push(RecoveryCandidate {
        id: candidates.len().to_string(),
        volume_index,
        path,
        node,
        readable,
        initialized_size,
        source_path: None,
        source_node: None,
    });
}
