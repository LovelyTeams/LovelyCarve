//! Project recognised Xiaomi/Android backups beneath their ZIP member paths.
//! Failed or encrypted backups retain their original readable file node.

use super::*;
use crate::core::evidence::readers::mi_bak::open_bak_archive_from_reader;
use crate::core::evidence::readers::tar_image::TarEntryKind;

pub(super) fn index_backups(
    archive: &mut ZipArchive<Cursor<Mmap>>,
    entries: &mut BTreeMap<String, ZipEntry>,
    total_size: &mut u64,
    source_path: &Path,
) -> Vec<TarImage> {
    let candidates: Vec<_> = entries
        .values()
        .filter(|entry| {
            entry.kind == ZipEntryKind::File
                && Path::new(&entry.rel_path)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("bak")
                            || extension.eq_ignore_ascii_case("ab")
                    })
        })
        .cloned()
        .collect();
    let mut backups = Vec::new();
    for entry in candidates {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let Some(member_index) = entry.archive_index else {
            continue;
        };
        let result = (|| {
            let limit = entry.size.checked_add(1).context("backup size overflow")?;
            let mut reader = archive.by_index(member_index)?.take(limit);
            let backup =
                open_bak_archive_from_reader(&mut reader, source_path, entry.rel_path.clone())?;
            if limit - reader.limit() != entry.size {
                bail!("backup ZIP member logical length mismatch");
            }
            Ok::<_, anyhow::Error>(backup)
        })();
        let backup = match result {
            Ok(backup) => backup,
            Err(error) => {
                tracing::warn!("mi-bak: keeping ZIP member {:?}: {error:#}", entry.rel_path);
                continue;
            }
        };
        if backup.entry_count() > MAX_ENTRIES.saturating_sub(entries.len()) {
            tracing::warn!(
                "mi-bak: keeping ZIP member {:?}: entry cap reached",
                entry.rel_path
            );
            continue;
        }

        let backup_index = backups.len();
        for member in backup
            .entries()
            .filter(|member| !member.rel_path.is_empty())
        {
            let rel_path = format!("{}/{}", entry.rel_path, member.rel_path);
            let kind = match member.kind {
                TarEntryKind::File | TarEntryKind::Hardlink => ZipEntryKind::File,
                TarEntryKind::Dir => ZipEntryKind::Dir,
                TarEntryKind::Symlink => ZipEntryKind::Symlink,
                TarEntryKind::Other => ZipEntryKind::Other,
            };
            entries.insert(
                rel_path.clone(),
                ZipEntry {
                    rel_path,
                    kind,
                    size: member.size,
                    mode: member.mode,
                    mtime: member.mtime,
                    link_target: member.link_target.clone(),
                    archive_index: None,
                    // Decoded offsets never refer to the original ZIP bytes.
                    stored_location: None,
                    bak_source: Some((backup_index, member.rel_path.clone())),
                },
            );
        }
        *total_size = total_size
            .saturating_sub(entry.size)
            .saturating_add(backup.total_size());
        let mut root = dir_entry(entry.rel_path.clone());
        root.mtime = entry.mtime;
        root.mode = entry.mode;
        entries.insert(entry.rel_path.clone(), root);
        tracing::info!(
            "mi-bak: indexed ZIP member {:?} (entries={})",
            entry.rel_path,
            backup.entry_count()
        );
        backups.push(backup);
    }
    backups
}

#[cfg(test)]
mod tests;
