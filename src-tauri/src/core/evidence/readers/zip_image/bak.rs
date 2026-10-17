//! Project recognised Xiaomi/Android backups beneath their ZIP member paths.
//! Failed or encrypted backups retain their original readable file node.
//!
//! With a case cache, the projection one open decodes is saved as a
//! [`listing`] bound to the archive and to every candidate member. Reopening
//! the unchanged archive restores the backup directories from it without
//! decoding; each backup then decodes on its first content read and must
//! reproduce exactly the members the listing projected.

use std::sync::OnceLock;
use std::time::Instant;

use super::*;
use crate::core::evidence::readers::mi_bak::open_bak_archive_from_reader;
use crate::core::evidence::readers::tar_image::{TarEntry, TarEntryKind};

mod listing;

use listing::{Expansion, ListedEntry, MemberIdentity};

/// The evidence archive whose backups are projected.
pub(super) struct BackupSource<'a> {
    pub(super) path: &'a Path,
    pub(super) archive_len: u64,
    /// Listing file in the case cache; `None` when no case owns the open.
    pub(super) listing: Option<&'a Path>,
}

/// The listing file kept in an evidence cache directory.
pub(super) fn listing_path(cache_directory: &Path) -> PathBuf {
    cache_directory.join(listing::LISTING_FILE)
}

/// A backup member projected as a directory. Its TAR decodes at open, or on
/// the first content read when the projection came from a saved listing.
pub(crate) struct ZipBackup {
    label: String,
    member_index: usize,
    /// Logical length of the ZIP member.
    size: u64,
    entry_count: usize,
    total_size: u64,
    tar: OnceLock<TarImage>,
    decoding: Mutex<()>,
}

impl ZipBackup {
    /// ZIP member path of the backup, also the label of its TAR.
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.entry_count
    }

    pub(crate) fn total_size(&self) -> u64 {
        self.total_size
    }

    pub(super) fn decoded(&self) -> Option<&TarImage> {
        self.tar.get()
    }

    /// Decodes the backup once. `verify` must accept the TAR before any
    /// reader sees it; a failed decode is retried by the next read.
    pub(super) fn decode(
        &self,
        archive: &mut ZipArchive<Cursor<ArchiveBytes>>,
        source_path: &Path,
        verify: impl FnOnce(&TarImage) -> Result<()>,
    ) -> Result<&TarImage> {
        let _decoding = self.decoding.lock();
        if let Some(tar) = self.tar.get() {
            return Ok(tar);
        }
        let started = Instant::now();
        let tar = decode_member(archive, self.member_index, self.size, |reader| {
            open_bak_archive_from_reader(reader, source_path, self.label.clone())
        })
        .with_context(|| format!("decode backup {:?}", self.label))?;
        verify(&tar)
            .with_context(|| format!("backup {:?} no longer matches its listing", self.label))?;
        tracing::info!(
            "mi-bak: decoded ZIP member {:?} on first read (entries={}) in {:.3?}",
            self.label,
            self.entry_count,
            started.elapsed()
        );
        Ok(self.tar.get_or_init(|| tar))
    }
}

pub(super) fn index_backups(
    archive: &mut ZipArchive<Cursor<ArchiveBytes>>,
    entries: &mut BTreeMap<String, ZipEntry>,
    total_size: &mut u64,
    source: BackupSource<'_>,
    progress: &dyn Fn(&str, u64, u64),
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Vec<ZipBackup>> {
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
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let identities = source
        .listing
        .and_then(|_| match member_identities(archive, &candidates) {
            Ok(identities) => Some(identities),
            Err(error) => {
                tracing::warn!("mi-bak: backup listing unavailable: {error:#}");
                None
            }
        });
    if let (Some(path), Some(identities)) = (source.listing, identities.as_deref()) {
        if let Some(backups) =
            listing::load(path, source.archive_len, identities).and_then(|expansions| {
                restore(entries, total_size, &candidates, identities, expansions)
            })
        {
            tracing::info!(
                "mi-bak: restored {} backup directories of {} from {}",
                backups.len(),
                source.path.display(),
                path.display()
            );
            return Ok(backups);
        }
    }
    let (backups, expansions) = decode_all(
        archive,
        entries,
        total_size,
        &candidates,
        source.path,
        progress,
        cancel,
    )?;
    if let (Some(path), Some(identities)) = (source.listing, identities.as_deref()) {
        if let Err(error) = listing::save(path, source.archive_len, identities, &expansions) {
            tracing::warn!(
                "mi-bak: saving backup listing {} failed: {error:#}",
                path.display()
            );
        }
    }
    Ok(backups)
}

fn member_identities(
    archive: &mut ZipArchive<Cursor<ArchiveBytes>>,
    candidates: &[ZipEntry],
) -> Result<Vec<MemberIdentity>> {
    candidates
        .iter()
        .map(|entry| {
            let index = entry
                .archive_index
                .context("backup candidate has no archive member")?;
            let member = archive.by_index_raw(index)?;
            Ok(MemberIdentity {
                path: entry.rel_path.clone(),
                index,
                header_start: member.header_start(),
                crc32: member.crc32(),
                compressed_size: member.compressed_size(),
                size: member.size(),
            })
        })
        .collect()
}

/// Projects the saved expansions (aligned with `candidates` and their
/// `identities`) without decoding. `None` when they cannot be projected as an
/// open of the archive would have done.
fn restore(
    entries: &mut BTreeMap<String, ZipEntry>,
    total_size: &mut u64,
    candidates: &[ZipEntry],
    identities: &[MemberIdentity],
    expansions: Vec<Option<Expansion>>,
) -> Option<Vec<ZipBackup>> {
    let listed = expansions
        .iter()
        .flatten()
        .fold(0usize, |count, expansion| {
            count.saturating_add(expansion.entries.len())
        });
    if expansions.len() != candidates.len()
        || identities.len() != candidates.len()
        || listed > MAX_ENTRIES.saturating_sub(entries.len())
    {
        return None;
    }
    let mut backups = Vec::new();
    for ((member, identity), expansion) in candidates.iter().zip(identities).zip(expansions) {
        let Some(expansion) = expansion else {
            continue;
        };
        let backup_index = backups.len();
        project(entries, total_size, member, backup_index, &expansion);
        backups.push(ZipBackup {
            label: member.rel_path.clone(),
            member_index: identity.index,
            size: member.size,
            entry_count: expansion.entries.len(),
            total_size: expansion.total_size,
            tar: OnceLock::new(),
            decoding: Mutex::new(()),
        });
    }
    Some(backups)
}

/// Decodes every candidate, returning the published backups and, per
/// candidate, what it projected (`None` when it stayed a file).
fn decode_all(
    archive: &mut ZipArchive<Cursor<ArchiveBytes>>,
    entries: &mut BTreeMap<String, ZipEntry>,
    total_size: &mut u64,
    candidates: &[ZipEntry],
    source_path: &Path,
    progress: &dyn Fn(&str, u64, u64),
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(Vec<ZipBackup>, Vec<Option<Expansion>>)> {
    let total = candidates
        .iter()
        .fold(0u64, |total, entry| total.saturating_add(entry.size));
    let mut done = 0u64;
    let mut backups = Vec::new();
    let mut expansions = Vec::with_capacity(candidates.len());
    for entry in candidates {
        anyhow::ensure!(!cancel.is_cancelled(), "image load cancelled");
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let Some(member_index) = entry.archive_index else {
            expansions.push(None);
            continue;
        };
        progress(&entry.rel_path, done, total);
        let result = decode_member(archive, member_index, entry.size, |reader| {
            let mut observed = BackupProgressReader {
                input: reader,
                cancel,
                progress,
                label: &entry.rel_path,
                base: done,
                consumed: 0,
                size: entry.size,
                total,
            };
            open_bak_archive_from_reader(&mut observed, source_path, entry.rel_path.clone())
        });
        anyhow::ensure!(!cancel.is_cancelled(), "image load cancelled");
        done = done.saturating_add(entry.size);
        progress(&entry.rel_path, done, total);
        let backup = match result {
            Ok(backup) => backup,
            Err(error) => {
                tracing::warn!("mi-bak: keeping ZIP member {:?}: {error:#}", entry.rel_path);
                expansions.push(None);
                continue;
            }
        };
        if backup.entry_count() > MAX_ENTRIES.saturating_sub(entries.len()) {
            tracing::warn!(
                "mi-bak: keeping ZIP member {:?}: entry cap reached",
                entry.rel_path
            );
            expansions.push(None);
            continue;
        }

        let expansion = Expansion {
            total_size: backup.total_size(),
            entries: backup
                .entries()
                .filter(|member| !member.rel_path.is_empty())
                .map(listed_entry)
                .collect(),
        };
        let backup_index = backups.len();
        project(entries, total_size, entry, backup_index, &expansion);
        tracing::info!(
            "mi-bak: indexed ZIP member {:?} (entries={})",
            entry.rel_path,
            backup.entry_count()
        );
        backups.push(ZipBackup {
            label: entry.rel_path.clone(),
            member_index,
            size: entry.size,
            entry_count: backup.entry_count(),
            total_size: backup.total_size(),
            tar: OnceLock::from(backup),
            decoding: Mutex::new(()),
        });
        expansions.push(Some(expansion));
    }
    // Candidates left after the entry cap stay files.
    expansions.resize_with(candidates.len(), || None);
    Ok((backups, expansions))
}

/// Streams ZIP member `member_index` through `decode`, then requires that the
/// member was consumed to exactly its logical length `size`. The member reader
/// verifies the CRC at its end, so a short or corrupt member never publishes.
fn decode_member(
    archive: &mut ZipArchive<Cursor<ArchiveBytes>>,
    member_index: usize,
    size: u64,
    decode: impl FnOnce(&mut dyn Read) -> Result<TarImage>,
) -> Result<TarImage> {
    let limit = size.checked_add(1).context("backup size overflow")?;
    let mut reader = archive.by_index(member_index)?.take(limit);
    let backup = decode(&mut reader)?;
    if limit - reader.limit() != size {
        bail!("backup ZIP member logical length mismatch");
    }
    Ok(backup)
}

/// Inserts the expansion's members beneath the backup's member path and turns
/// the member itself into the backup directory.
fn project(
    entries: &mut BTreeMap<String, ZipEntry>,
    total_size: &mut u64,
    member: &ZipEntry,
    backup_index: usize,
    expansion: &Expansion,
) {
    for listed in &expansion.entries {
        let rel_path = format!("{}/{}", member.rel_path, listed.path);
        entries.insert(
            rel_path.clone(),
            ZipEntry {
                rel_path,
                kind: listed.kind,
                size: listed.size,
                mode: listed.mode,
                mtime: listed.mtime,
                link_target: listed.link_target.clone(),
                archive_index: None,
                // Decoded offsets never refer to the original ZIP bytes.
                stored_location: None,
                bak_source: Some((backup_index, listed.path.clone())),
            },
        );
    }
    *total_size = total_size
        .saturating_sub(member.size)
        .saturating_add(expansion.total_size);
    let mut root = dir_entry(member.rel_path.clone());
    root.bak_source = Some((backup_index, String::new()));
    root.mtime = member.mtime;
    root.mode = member.mode;
    entries.insert(member.rel_path.clone(), root);
}

fn listed_entry(member: &TarEntry) -> ListedEntry {
    ListedEntry {
        path: member.rel_path.clone(),
        kind: match member.kind {
            TarEntryKind::File | TarEntryKind::Hardlink => ZipEntryKind::File,
            TarEntryKind::Dir => ZipEntryKind::Dir,
            TarEntryKind::Symlink => ZipEntryKind::Symlink,
            TarEntryKind::Other => ZipEntryKind::Other,
        },
        size: member.size,
        mode: member.mode,
        mtime: member.mtime,
        link_target: member.link_target.clone(),
    }
}

/// A backup restored from a listing serves content only once its decoded TAR
/// holds exactly the members the listing projected beneath it.
pub(super) fn verify_projection(
    entries: &BTreeMap<String, ZipEntry>,
    backup_index: usize,
    backup: &ZipBackup,
    tar: &TarImage,
) -> Result<()> {
    anyhow::ensure!(
        tar.entry_count() == backup.entry_count && tar.total_size() == backup.total_size,
        "decoded {} entries totalling {} bytes, listing has {} totalling {}",
        tar.entry_count(),
        tar.total_size(),
        backup.entry_count,
        backup.total_size
    );
    for member in tar.entries().filter(|member| !member.rel_path.is_empty()) {
        let listed = listed_entry(member);
        let matches = entries
            .get(&format!("{}/{}", backup.label, member.rel_path))
            .is_some_and(|entry| {
                entry
                    .bak_source
                    .as_ref()
                    .is_some_and(|(index, path)| *index == backup_index && *path == listed.path)
                    && entry.kind == listed.kind
                    && entry.size == listed.size
                    && entry.mode == listed.mode
                    && entry.mtime == listed.mtime
                    && entry.link_target == listed.link_target
            });
        anyhow::ensure!(
            matches,
            "member {:?} differs from the listing",
            member.rel_path
        );
    }
    Ok(())
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
