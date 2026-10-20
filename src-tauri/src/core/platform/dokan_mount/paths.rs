//! Mount-point validation and filesystem path projection.
use super::*;
use crate::core::filesystem::evidence_session::EvidenceSession;

/// Normalize a user-supplied mount point into the form Dokan expects.
///
/// Drive-letter mounts (`C`, `C:`, `C:\`, `C:/`) canonicalize to `C:\` — the
/// trailing-backslash form the mount manager requires. Anything longer is
/// treated as a directory mount point and canonicalized to `C:\mount\foo`
/// (forward slashes flipped to back, trailing separator stripped) — Dokan's
/// folder mount points must NOT carry a trailing backslash.
pub(super) fn normalize_mount_point(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("mount point is empty");
    }
    let bytes = trimmed.as_bytes();
    // `C`
    if trimmed.len() == 1 && bytes[0].is_ascii_alphabetic() {
        return Ok(format!("{trimmed}:\\"));
    }
    // `C:`
    if trimmed.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Ok(format!("{trimmed}\\"));
    }
    // `C:\` or `C:/`
    if trimmed.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        return Ok(format!("{}:\\", &trimmed[..1]));
    }
    // Directory mount point: flip separators, strip any trailing slash.
    let flipped = trimmed.replace('/', "\\");
    Ok(flipped.trim_end_matches('\\').to_string())
}

/// True when `normalized` is a bare drive-letter root like `C:\` — the only
/// shape that should register with the Windows Mount Manager. Directory mount
/// points must skip MOUNT_MANAGER (its reparse-point registration is rejected
/// for folder targets on many systems).
pub(super) fn is_drive_letter_mount(normalized: &str) -> bool {
    let bytes = normalized.as_bytes();
    normalized.len() == 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// Validate a directory mount point before handing it to Dokan: it must be an
/// existing, empty directory on an NTFS volume. Dokan otherwise fails with an
/// opaque "Start" error; these checks turn the common mistakes into clear,
/// actionable messages. The NTFS probe is best-effort — if the volume query
/// itself fails we let Dokan be the final arbiter rather than blocking a mount
/// that might be fine.
pub(super) fn validate_folder_mount(path: &str) -> Result<()> {
    let dir = std::path::Path::new(path);
    if !dir.exists() {
        anyhow::bail!("挂载文件夹不存在：{path}");
    }
    if !dir.is_dir() {
        anyhow::bail!("挂载点不是文件夹：{path}");
    }
    if std::fs::read_dir(dir)
        .with_context(|| format!("无法读取挂载文件夹：{path}"))?
        .next()
        .is_some()
    {
        anyhow::bail!("挂载文件夹必须为空：{path}");
    }
    if let Some(fs_name) = volume_filesystem_name(dir) {
        if !fs_name.eq_ignore_ascii_case("NTFS") {
            anyhow::bail!("挂载文件夹须位于 NTFS 卷（当前为 {fs_name}）：{path}");
        }
    }
    Ok(())
}

/// Best-effort read of the filesystem name (`NTFS`, `FAT32`, ...) for the
/// volume hosting `path`, via `GetVolumeInformationW` on the path's drive root
/// (`C:\`). Returns `None` when the root can't be derived or the query fails.
pub(super) fn volume_filesystem_name(path: &std::path::Path) -> Option<String> {
    let root = match path.components().next()? {
        std::path::Component::Prefix(prefix) => {
            // `C:\mount\foo` → prefix os_str is `C:` → root `C:\`.
            format!("{}\\", prefix.as_os_str().to_string_lossy())
        }
        _ => return None,
    };
    let root_u16 = U16CString::from_str(&root).ok()?;
    let mut fs_name_buf = [0u16; 32];
    let ok = unsafe {
        GetVolumeInformationW(
            root_u16.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_name_buf.as_mut_ptr(),
            fs_name_buf.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    let len = fs_name_buf
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(fs_name_buf.len());
    Some(String::from_utf16_lossy(&fs_name_buf[..len]))
}

pub(super) fn root_node(image: &Arc<EvidenceSession>, node_id: &str) -> Result<FsNode> {
    if let Some(id) = host_docker::parse_host_docker_id(node_id) {
        return host_docker::mount_root_node(image, &id)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Docker mount root must be the container group or a container root: {node_id}"
            )
        });
    }
    let node = fs_tree::children_for_mount(image, "")?
        .into_iter()
        .find(|node| node.id == node_id)
        .or_else(|| find_child_by_id(image, node_id).ok().flatten())
        .ok_or_else(|| anyhow::anyhow!("mount root node not found: {node_id}"))?;
    Ok(node)
}

pub(super) fn find_child_by_id(
    image: &Arc<EvidenceSession>,
    node_id: &str,
) -> Result<Option<FsNode>> {
    for parent in fs_tree::walk_image(image, 20_000)? {
        if parent.node.id == node_id {
            return Ok(Some(parent.node));
        }
    }
    Ok(None)
}

pub(super) fn normalize_dokan_path(path: &str) -> String {
    let replaced = path.replace('/', "\\");
    if replaced.is_empty() || replaced == "\\" {
        return "\\".to_string();
    }
    let trimmed = replaced.trim_end_matches('\\');
    if trimmed.starts_with('\\') {
        trimmed.to_string()
    } else {
        format!("\\{trimmed}")
    }
}

pub(super) fn path_components(path: &str) -> Vec<String> {
    path.trim_matches('\\')
        .split('\\')
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

pub(super) fn join_mount_path(parent: &str, name: &str) -> String {
    if parent == "\\" {
        format!("\\{name}")
    } else {
        format!("{parent}\\{name}")
    }
}

pub(super) fn volume_name_for_mount(image: &Arc<EvidenceSession>, root_node_id: &str) -> String {
    let stem = image
        .path
        .file_stem()
        .or_else(|| image.path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let suffix = mount_suffix_for_root(image, root_node_id);
    let raw = if let Some(suffix) = suffix {
        if stem.is_empty() {
            suffix
        } else {
            format!("{stem}_{suffix}")
        }
    } else {
        stem.to_string()
    };
    sanitize_volume_label(&raw)
}

pub(super) fn mount_suffix_for_root(
    image: &Arc<EvidenceSession>,
    root_node_id: &str,
) -> Option<String> {
    let trimmed = root_node_id
        .strip_prefix("bde-locked:")
        .unwrap_or(root_node_id);
    let image = image.filesystem_session(trimmed);
    if trimmed == "docker:" {
        let tag = image
            .docker
            .as_ref()
            .and_then(|docker| docker.repo_tags().first().cloned())
            .filter(|tag| !tag.trim().is_empty())
            .unwrap_or_else(|| "image".to_string());
        return Some(format!("docker_{tag}"));
    }
    if let Some(id) = host_docker::parse_host_docker_id(trimmed) {
        return Some(match &id {
            host_docker::HostDockerNodeId::Group => "docker_containers".to_string(),
            host_docker::HostDockerNodeId::Container { container_id, .. } => {
                let label = host_docker::mount_root_node(image, &id)
                    .ok()
                    .flatten()
                    .map(|node| node.name)
                    .unwrap_or_else(|| container_id.chars().take(12).collect());
                format!("docker_{label}")
            }
        });
    }
    if trimmed == "tar:" {
        // The image file stem already carries the archive name (the label is
        // built as `{stem}_{suffix}`), so the suffix is just the type tag.
        return Some("tar".to_string());
    }
    if trimmed == "zip:" {
        // Same as tar: the stem carries the archive name, so the suffix is just
        // the type tag.
        return Some("zip".to_string());
    }
    if trimmed == "ad1:" {
        return Some("ad1".to_string());
    }
    if let Some(part_id) = trimmed.strip_prefix("part:") {
        let id: u8 = part_id.parse().ok()?;
        let label = image
            .partitions
            .iter()
            .find(|partition| partition.id == id)
            .and_then(|partition| partition.label.clone());
        return Some(match label {
            Some(label) if !label.trim().is_empty() => format!("p{id}_{label}"),
            _ => format!("p{id}"),
        });
    }
    if let Some((part_id, lv_index)) = parse_lvm_id(trimmed) {
        let id: u8 = part_id.parse().ok()?;
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.id == id)?;
        let volume = LvmVolume::open(image, partition).ok()?;
        let lv = volume.logical_volumes.get(lv_index)?;
        return Some(format!("p{id}_lv_{}", lv.name));
    }
    Some(trimmed.to_string())
}

pub(super) fn is_docker_mount_root(root_node_id: &str) -> bool {
    root_node_id == "docker:"
        || root_node_id.starts_with("docker:")
        || host_docker::parse_host_docker_id(root_node_id).is_some()
}

pub(super) fn fs_kind_for_root(
    image: &EvidenceSession,
    root_node_id: &str,
) -> Option<PartitionKind> {
    let trimmed = root_node_id
        .strip_prefix("bde-locked:")
        .unwrap_or(root_node_id);
    let image = image
        .decrypted_session_for_node(trimmed)
        .map(Arc::as_ref)
        .unwrap_or(image);
    if let Some(part_id) = trimmed.strip_prefix("part:") {
        let part_id = part_id.parse::<u8>().ok()?;
        return image
            .partitions
            .iter()
            .find(|partition| partition.id == part_id)
            .map(|partition| partition.kind);
    }
    if let Some((part_id, lv_index)) = parse_lvm_id(trimmed) {
        let part_id = part_id.parse::<u8>().ok()?;
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.id == part_id)?;
        let volume = LvmVolume::open(image, partition).ok()?;
        return volume.logical_volumes.get(lv_index).map(|lv| lv.kind);
    }
    None
}

pub(super) fn fs_label_for_kind(kind: PartitionKind) -> &'static str {
    match kind {
        PartitionKind::Ntfs => "NTFS",
        PartitionKind::Refs => "REFS",
        PartitionKind::Ext4 => "EXT4",
        PartitionKind::Btrfs => "BTRFS",
        PartitionKind::Fat32 => "FAT32",
        PartitionKind::Fat12 => "FAT12",
        PartitionKind::Fat16 => "FAT16",
        PartitionKind::ExFat => "EXFAT",
        PartitionKind::Cdfs => "CDFS",
        PartitionKind::Squashfs => "SQUASHFS",
        PartitionKind::ApfsContainer => "APFS",
        PartitionKind::HfsPlus => "HFS+",
        PartitionKind::HfsX => "HFSX",
        PartitionKind::Ufs => "UFS",
        PartitionKind::Xfs => "XFS",
        PartitionKind::Zfs => "ZFS",
        PartitionKind::Lvm => "LVM2",
        PartitionKind::Bitlocker => "BITLOCKER",
        PartitionKind::Luks => "LUKS",
        PartitionKind::Filevault => "FILEVAULT",
        PartitionKind::Mdadm => "MDADM",
        PartitionKind::Apm
        | PartitionKind::Bsd
        | PartitionKind::Swap
        | PartitionKind::Extended
        | PartitionKind::BiosBoot
        | PartitionKind::Unknown => "LCARVE",
    }
}

/// Filesystems that distinguish `Foo` from `foo` at rest. Returning `true`
/// switches Dokan into case-sensitive lookup so the original forensic path
/// is preserved when, e.g., ext4 has both `README` and `readme` in the same
/// directory. NTFS / FAT / exFAT / HFS+ stay case-insensitive (their
/// canonical lookup is too).
pub(super) fn is_case_sensitive_fs(kind: PartitionKind) -> bool {
    matches!(
        kind,
        PartitionKind::Ext4
            | PartitionKind::Btrfs
            | PartitionKind::Xfs
            | PartitionKind::Zfs
            | PartitionKind::Ufs
            | PartitionKind::Squashfs
            | PartitionKind::ApfsContainer
            | PartitionKind::HfsX
    )
}

pub(super) fn sanitize_volume_label(label: &str) -> String {
    crate::core::common::navigation_name::volume_name(label)
        .trim_matches('.')
        .chars()
        .take(MAX_VOLUME_LABEL_CHARS)
        .collect()
}

/// Replace characters Win32 rejects (`\\ / : * ? " < > |` and ASCII control
/// codes) with `_` so Linux / Unix names like `report:2024.txt` round-trip
/// to a valid Windows filename. Returns `Cow::Borrowed` when the original
/// name is already legal so we don't pay an allocation per directory entry.
pub(super) fn sanitize_for_windows(name: &str) -> std::borrow::Cow<'_, str> {
    let needs_fix = name.chars().any(|ch| {
        ch.is_control() || matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    });
    if !needs_fix {
        return std::borrow::Cow::Borrowed(name);
    }
    let replaced: String = name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    std::borrow::Cow::Owned(replaced)
}
