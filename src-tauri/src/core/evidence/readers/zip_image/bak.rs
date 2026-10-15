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
    progress: &dyn Fn(&str, u64, u64),
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Vec<TarImage>> {
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
    let total = candidates
        .iter()
        .fold(0u64, |total, entry| total.saturating_add(entry.size));
    let mut done = 0u64;
    let mut backups = Vec::new();
    for entry in candidates {
        anyhow::ensure!(!cancel.is_cancelled(), "image load cancelled");
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let Some(member_index) = entry.archive_index else {
            continue;
        };
        progress(&entry.rel_path, done, total);
        let result = (|| {
            let limit = entry.size.checked_add(1).context("backup size overflow")?;
            let mut reader = archive.by_index(member_index)?.take(limit);
            let backup = {
                let mut observed = BackupProgressReader {
                    input: &mut reader,
                    cancel,
                    progress,
                    label: &entry.rel_path,
                    base: done,
                    consumed: 0,
                    size: entry.size,
                    total,
                };
                open_bak_archive_from_reader(&mut observed, source_path, entry.rel_path.clone())?
            };
            if limit - reader.limit() != entry.size {
                bail!("backup ZIP member logical length mismatch");
            }
            Ok::<_, anyhow::Error>(backup)
        })();
        anyhow::ensure!(!cancel.is_cancelled(), "image load cancelled");
        done = done.saturating_add(entry.size);
        progress(&entry.rel_path, done, total);
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
        root.bak_source = Some((backup_index, String::new()));
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
    Ok(backups)
}

// Observe input consumption rather than TAR output: backup headers and
// compressed streams have different sizes, but ZIP member lengths are known.
struct BackupProgressReader<'a, R> {
    input: R,
    cancel: &'a tokio_util::sync::CancellationToken,
    progress: &'a dyn Fn(&str, u64, u64),
    label: &'a str,
    base: u64,
    consumed: u64,
    size: u64,
    total: u64,
}

impl<R: Read> Read for BackupProgressReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::other("image load cancelled"));
        }
        let read = self.input.read(buffer)?;
        self.consumed = self.consumed.saturating_add(read as u64);
        if read > 0 {
            (self.progress)(
                self.label,
                self.base.saturating_add(self.consumed.min(self.size)),
                self.total,
            );
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests;
