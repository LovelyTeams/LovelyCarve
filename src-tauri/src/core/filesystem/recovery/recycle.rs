//! Restore recycled names from validated $I companions while retaining the
//! physical node and its original metadata for read/export provenance.
use super::*;
use crate::core::filesystem::volumes::ntfs::{parse_ntfs_id, NtfsVolume};

pub(super) fn restore_recycle_names(
    image: &Arc<DiskImage>,
    recovered: &RecoveryVolume,
    index: usize,
    candidates: &mut [RecoveryCandidate],
    directories: &mut Vec<RecoveryDirectory>,
    cancel: &CancellationToken,
) -> Result<()> {
    let Some(geometry) = &recovered.reconstruction else {
        return Ok(());
    };
    let volume = NtfsVolume::open_recovered(image, &recovered.partition, geometry)?;
    let mut mappings = Vec::new();
    for metadata in candidates.iter().filter(|entry| {
        entry.volume_index == index
            && entry.node.name.starts_with("$I")
            && entry.path.starts_with("/$RECYCLE.BIN/")
            && entry.node.size <= 32768
    }) {
        check_cancel(cancel)?;
        let record = parse_ntfs_id(&metadata.node.id)
            .context("Invalid recycle metadata node")?
            .1;
        let bytes = match volume.read_file(record, 32768) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::debug!("Recycle metadata {}: {error:#}", metadata.node.id);
                continue;
            }
        };
        let Some((destination, size)) = original_path(&bytes) else {
            continue;
        };
        let target_name = format!("$R{}", &metadata.node.name[2..]);
        if let Some(target) = candidates.iter().find(|entry| {
            entry.volume_index == index
                && entry.node.name == target_name
                && entry.node.parent_id == metadata.node.parent_id
                && entry.node.size == size
        }) {
            mappings.push((
                target.node.id.clone(),
                target.path.clone(),
                destination,
                false,
            ));
        } else if let Some(target) = directories.iter().find(|entry| {
            entry.volume_index == index
                && entry.node.name == target_name
                && entry.node.parent_id == metadata.node.parent_id
        }) {
            mappings.push((
                target.node.id.clone(),
                target.path.clone(),
                destination,
                true,
            ));
        }
    }
    // Establish original parent folders before rewriting directory paths. A
    // missing parent gets an explicit virtual identity; a deleted directory at
    // the same path is not assumed to be the same generation of that folder.
    for (id, source, destination, is_directory) in &mappings {
        let parent_path = destination
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        let parent_id = original_parent(directories, index, parent_path)?;
        let name = destination
            .rsplit('/')
            .next()
            .unwrap_or(destination)
            .to_string();
        if *is_directory {
            if let Some(target) = directories
                .iter_mut()
                .find(|entry| entry.volume_index == index && entry.node.id == *id)
            {
                target.source_path = Some(source.clone());
                target.source_node = Some(target.node.clone());
                target.path = destination.clone();
                target.node.name = name;
                target.node.parent_id = Some(parent_id);
                target.node.deleted = true;
            }
        } else if let Some(target) = candidates
            .iter_mut()
            .find(|entry| entry.volume_index == index && entry.node.id == *id)
        {
            target.source_path = Some(source.clone());
            target.source_node = Some(target.node.clone());
            target.path = destination.clone();
            target.node.name = name;
            target.node.parent_id = Some(parent_id);
            target.node.deleted = true;
        }
    }
    // Children retain record identities, so moving a recovered directory does
    // not merge it with a same-name folder or disturb its data mappings.
    for entry in candidates
        .iter_mut()
        .filter(|entry| entry.volume_index == index && entry.source_path.is_none())
    {
        for (_, source, destination, is_directory) in &mappings {
            if *is_directory && entry.path.starts_with(&format!("{source}/")) {
                entry.source_path = Some(entry.path.clone());
                entry.source_node = Some(entry.node.clone());
                entry.path = format!("{destination}{}", &entry.path[source.len()..]);
                entry.node.deleted = true;
                break;
            }
        }
    }
    for entry in directories
        .iter_mut()
        .filter(|entry| entry.volume_index == index && entry.source_path.is_none())
    {
        for (_, source, destination, is_directory) in &mappings {
            if *is_directory && entry.path.starts_with(&format!("{source}/")) {
                entry.source_path = Some(entry.path.clone());
                entry.source_node = Some(entry.node.clone());
                entry.path = format!("{destination}{}", &entry.path[source.len()..]);
                entry.node.deleted = true;
                break;
            }
        }
    }
    Ok(())
}

fn original_parent(
    directories: &mut Vec<RecoveryDirectory>,
    index: usize,
    path: &str,
) -> Result<String> {
    if path.is_empty() || path == "/" {
        return Ok("part:0".to_string());
    }
    if let Some(parent) = directories
        .iter()
        .find(|entry| entry.volume_index == index && entry.path == path && !entry.node.deleted)
    {
        return Ok(parent.node.id.clone());
    }
    let mut parent = "part:0".to_string();
    let mut current = String::new();
    for component in path
        .split('/')
        .filter(|component| !component.is_empty())
        .take(64)
    {
        current.push('/');
        current.push_str(component);
        if let Some(entry) = directories.iter().find(|entry| {
            entry.volume_index == index && entry.path == current && !entry.node.deleted
        }) {
            parent = entry.node.id.clone();
            continue;
        }
        if directories.len() >= MAX_CANDIDATES {
            bail!("Recovered directory limit reached while restoring original paths");
        }
        let id = format!(
            "recovery:original-parent:{}",
            hex::encode(current.as_bytes())
        );
        directories.push(RecoveryDirectory {
            volume_index: index,
            path: current.clone(),
            source_path: None,
            source_node: None,
            node: FsNode {
                id: id.clone(),
                name: component.to_string(),
                parent_id: Some(parent),
                kind: NodeKind::Dir,
                size: 0,
                deleted: false,
                inode: None,
                timestamps: super::super::tree::fs_tree::Mactb::default(),
                link_target: None,
                has_snapshot: false,
            },
        });
        parent = id;
    }
    Ok(parent)
}
fn original_path(bytes: &[u8]) -> Option<(String, u64)> {
    let version = u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?);
    let size = u64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?);
    let value = match version {
        1 => bytes.get(24..544)?,
        2 => {
            let length = u32::from_le_bytes(bytes.get(24..28)?.try_into().ok()?) as usize;
            if length == 0 || length > 16000 {
                return None;
            }
            bytes.get(28..28usize.checked_add(length.checked_mul(2)?)?)?
        }
        _ => return None,
    };
    let words = value
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    let name = String::from_utf16(&words).ok()?.replace('\\', "/");
    let name = if name.as_bytes().get(1) == Some(&b':') && name.as_bytes()[0].is_ascii_alphabetic()
    {
        &name[2..]
    } else if name.starts_with("//") {
        &name[1..]
    } else {
        return None;
    };
    let components = name
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components.is_empty()
        || components.len() > 64
        || components
            .iter()
            .any(|component| matches!(*component, "." | ".."))
    {
        return None;
    }
    Some((format!("/{}", components.join("/")), size))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_recycle_v2_paths_are_bounded_and_do_not_escape() {
        fn data(path: &str) -> Vec<u8> {
            let name = path.encode_utf16().chain([0]).collect::<Vec<_>>();
            let mut bytes = vec![0; 28];
            bytes[..8].copy_from_slice(&2u64.to_le_bytes());
            bytes[8..16].copy_from_slice(&100u64.to_le_bytes());
            bytes[24..28].copy_from_slice(&(name.len() as u32).to_le_bytes());
            for unit in name {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes
        }
        assert_eq!(
            original_path(&data("E:\\temp\\file.bin")),
            Some(("/temp/file.bin".to_string(), 100))
        );
        assert!(original_path(&data("E:\\..\\file.bin")).is_none());
        assert!(original_path(&data("relative\\file.bin")).is_none());
        let mut invalid = data("E:\\file.bin");
        invalid[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(original_path(&invalid).is_none());
    }
}
