//! session implementation.
use super::*;
use crate::core::common::error::Failure;
use crate::core::filesystem::evidence_session::EvidenceSession;

/// Ensure `session` holds a VFS import covering `(image_id, node_id)`.
/// Reuses an existing import when it matches and still exists; otherwise
/// reloads it. An in-flight reader keeps its old VFS files alive through Arc
/// ownership; clearing/replacing the cache never waits for its SQL query.
/// `parent_id` is the database's directory node, used to find its sidecars.
pub fn cached_session(
    session: &mut Option<Arc<SqliteSession>>,
    image: &Arc<EvidenceSession>,
    image_id: &str,
    node_id: &str,
    parent_id: Option<&str>,
) -> anyhow::Result<Arc<SqliteSession>> {
    let hit = matches!(
        session.as_ref(),
        Some(s) if s.image_id == image_id
            && s.node_id == node_id
            && s.parent_id.as_deref() == parent_id
            && sqlite_vfs::exists(&s.main_name)
    );
    if !hit {
        *session = Some(Arc::new(load_session(image, image_id, node_id, parent_id)?));
    }
    session
        .as_ref()
        .cloned()
        .context("sqlite session missing after VFS import")
}

pub(super) fn load_session(
    image: &Arc<EvidenceSession>,
    image_id: &str,
    node_id: &str,
    parent_id: Option<&str>,
) -> anyhow::Result<SqliteSession> {
    let main_name = format!(
        "lovelycarve-sqlite-{}.sqlite",
        uuid::Uuid::new_v4().simple()
    );

    let (main_bytes, truncated) = read_node_prefix(image, node_id)?;
    let bytes_copied = main_bytes.len() as u64;
    let head16 = head16_from_bytes(&main_bytes);
    sqlite_vfs::import_file(&main_name, &main_bytes)?;
    drop(main_bytes);

    let mut sidecars: Vec<String> = Vec::new();
    let imported_wal = find_sidecar_nodes(image, node_id, parent_id).and_then(|siblings| {
        if let Some(wal) = siblings
            .into_iter()
            .find(|sidecar| sidecar.suffix == "-wal")
        {
            let (bytes, truncated) = read_node_prefix(image, &wal.node_id)
                .with_context(|| format!("read {}", wal.name))?;
            anyhow::ensure!(
                !truncated,
                Failure::resource_limit(format!("{} exceeds the preview import limit", wal.name))
            );
            if !bytes.is_empty() {
                sqlite_vfs::import_file(&wal_vfs_name(&main_name), &bytes)
                    .with_context(|| format!("import {}", wal.name))?;
                sidecars.push(wal.name);
            }
        }
        Ok(())
    });
    let wal_issue = imported_wal.err().map(|error| format!("{error:#}"));
    if let Some(issue) = &wal_issue {
        tracing::warn!("SQLite preview without verified WAL coverage: {issue}");
    }

    Ok(SqliteSession {
        image_id: image_id.to_string(),
        node_id: node_id.to_string(),
        parent_id: parent_id.map(str::to_owned),
        main_name,
        sidecars,
        wal_issue,
        bytes_copied,
        truncated,
        head16,
    })
}

/// The sidecar files sharing the database's directory and name. `parent_id`
/// is that directory's node; without it the parent is guessed from the node
/// id, which only works for path-shaped ids (not `ntfs:` / `ext4:` /
/// `folder:` ones).
pub(crate) fn sidecar_nodes(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    parent_id: Option<&str>,
) -> Vec<SidecarNode> {
    find_sidecar_nodes(image, node_id, parent_id).unwrap_or_default()
}

fn find_sidecar_nodes(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    parent_id: Option<&str>,
) -> anyhow::Result<Vec<SidecarNode>> {
    let parent = match parent_id {
        Some(parent) => parent.to_string(),
        None => parent_id_of(image, node_id).unwrap_or_default(),
    };
    let siblings =
        fs_tree::children_for_image(image, &parent).context("read SQLite parent directory")?;
    let target_name = siblings
        .iter()
        .find(|sibling| sibling.id == node_id)
        .map(|sibling| &sibling.name)
        .context(Failure::not_found(
            "SQLite node not found in its parent directory",
        ))?;
    let mut found = Vec::new();
    for suffix in ["-wal", "-shm", "-journal"] {
        let wanted = format!("{target_name}{suffix}");
        if let Some(sibling) = siblings
            .iter()
            .find(|sibling| sibling.kind == NodeKind::File && sibling.name == wanted)
        {
            found.push(SidecarNode {
                suffix,
                node_id: sibling.id.clone(),
                name: sibling.name.clone(),
            });
        }
    }
    Ok(found)
}

/// Read a file node up to [`MAX_PREVIEW_BYTES`], returning whether it was cut.
pub(crate) fn read_node_prefix(
    image: &Arc<EvidenceSession>,
    node_id: &str,
) -> anyhow::Result<(Vec<u8>, bool)> {
    let stat = fs_tree::file_stat(image, node_id).ok();
    let cap = usize::try_from(MAX_PREVIEW_BYTES).unwrap_or(usize::MAX);
    let read_limit = stat
        .as_ref()
        .map(|stat| stat.size.min(MAX_PREVIEW_BYTES) as usize)
        .unwrap_or(cap);
    let bytes = fs_tree::read_file_limited(image, node_id, read_limit)?;
    anyhow::ensure!(
        stat.is_none() || bytes.len() == read_limit,
        Failure::corrupt(format!(
            "SQLite file was read short ({} of {read_limit} bytes)",
            bytes.len()
        ))
    );
    let truncated = stat.is_some_and(|stat| stat.size > MAX_PREVIEW_BYTES);
    Ok((bytes, truncated))
}

pub(super) fn head16_from_bytes(bytes: &[u8]) -> Option<[u8; 16]> {
    bytes.get(..16)?.try_into().ok()
}

#[cfg(test)]
pub(super) fn sanitise_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (ch as u32) < 0x20
        {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let trimmed = out.trim().to_string();
    if trimmed.is_empty() {
        "preview.sqlite".to_string()
    } else {
        trimmed
    }
}

pub(super) fn parent_id_of(image: &Arc<EvidenceSession>, node_id: &str) -> Option<String> {
    lookup_node(image, node_id).and_then(|node| node.parent_id)
}

pub(super) fn lookup_node(image: &Arc<EvidenceSession>, node_id: &str) -> Option<FsNode> {
    let parent = guess_parent(image, node_id)?;
    fs_tree::children_for_image(image, &parent)
        .ok()?
        .into_iter()
        .find(|node| node.id == node_id)
}

/// Best-effort: walk back through the node id by stripping the last
/// `/` segment until we find a parent that resolves. Mirrors what the
/// frontend does — most filesystem ids are `imageId/backendPath`, so
/// the parent is just one segment back. Falls back to `""` (root) if
/// nothing else matches.
pub(super) fn guess_parent(_image: &Arc<EvidenceSession>, node_id: &str) -> Option<String> {
    let trimmed = node_id.trim_end_matches('/');
    if let Some(idx) = trimmed.rfind('/') {
        Some(trimmed[..idx].to_string())
    } else {
        Some(String::new())
    }
}
