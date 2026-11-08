//! Projection of a backup as a **logical file container**.
//!
//! An iOS backup holds a captured file tree, not a block device: there is no
//! partition table and no filesystem to walk, so it belongs with AD1 and
//! AFF4-Logical rather than with EWF or VMDK.
//!
//! This module flattens the manifest into an entry list plus read-by-index — the
//! shape any browser or mounting front-end wants — so a consumer needs an
//! adapter rather than a new code path, which is the format special-casing
//! ADR-0011 exists to prevent.
//!
//! # What this is not
//!
//! An earlier version of these docs said the fleet reaches logical containers
//! through `disk_forensic::logical::open` and that this module was shaped to
//! match it. That was never true of the consumer it named: `4n6mount` does not
//! depend on `disk-forensic`. Mounting goes through `forensic_vfs::TreeOpen`,
//! implemented behind this crate's `vfs` feature — see
//! [ADR-0010](https://github.com/SecurityRonin/ios-backup-forensic/blob/main/docs/decisions/0010-mounting-goes-through-the-vfs-tree-seam.md),
//! which supersedes ADR-0005.
//!
//! [`LogicalView::open`] takes [`Credentials`] because an encrypted backup
//! cannot be listed without them — the entry list itself lives in an encrypted
//! `Manifest.db`.

use std::path::Path;

use crate::backup::Backup;
use crate::credentials::Credentials;
use crate::error::Error;
use crate::manifest::BackupFile;

/// One entry in the projected tree.
///
/// Field-for-field what `disk_forensic::logical::LogicalEntry` carries, so the
/// adapter is a `map` and nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LogicalEntry {
    /// `<domain>/<relativePath>` — the layout every backup browser presents,
    /// with the domain as the top-level directory.
    pub path: String,
    /// `true` for a directory row.
    pub is_dir: bool,
    /// Content length in bytes; `0` for a directory.
    pub size: u64,
}

/// A backup viewed as a logical container.
pub struct LogicalView {
    backup: Backup,
    entries: Vec<LogicalEntry>,
}

impl core::fmt::Debug for LogicalView {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LogicalView")
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

/// Join a domain and a relative path into the projected path.
///
/// Neither part is rewritten: an entry corresponds one-to-one with a manifest
/// row, and inventing intermediate directories — or splitting a path that
/// already contains slashes — would make the projection disagree with the
/// evidence it is projecting.
fn projected_path(entry: &BackupFile) -> String {
    if entry.relative_path.is_empty() {
        entry.domain.clone()
    } else {
        format!("{}/{}", entry.domain, entry.relative_path)
    }
}

impl LogicalView {
    /// Open the backup at `path` and project its tree.
    ///
    /// # Errors
    /// As [`Backup::open_with`] — in particular [`Error::PasswordRequired`] when
    /// the backup is encrypted and `credentials` offers nothing.
    pub fn open(path: &Path, credentials: &Credentials) -> Result<Self, Error> {
        let backup = Backup::open_with(path, credentials)?;
        let entries = backup
            .files()
            .iter()
            .map(|entry| LogicalEntry {
                path: projected_path(entry),
                is_dir: !entry.kind.has_content(),
                // A directory has no content length; a file whose size the
                // manifest omitted reports 0 here because the projection's
                // contract has no way to say "unknown". `Backup::files()` keeps
                // the distinction for callers that need it.
                size: if entry.kind.has_content() {
                    entry.size.unwrap_or(0)
                } else {
                    0
                },
            })
            .collect();
        Ok(Self { backup, entries })
    }

    /// The projected tree, in manifest order. Index into this for
    /// [`Self::read_file`].
    #[must_use]
    pub fn entries(&self) -> &[LogicalEntry] {
        &self.entries
    }

    /// The backup underneath, for callers that need domains, protection classes
    /// or timestamps the projection flattens away.
    #[must_use]
    pub fn backup(&self) -> &Backup {
        &self.backup
    }

    /// Read the content of the entry at `index`.
    ///
    /// # Errors
    /// [`Error::NoSuchEntry`] when `index` is out of range, [`Error::NotAFile`]
    /// for a directory, and otherwise as [`Backup::read`].
    pub fn read_file(&mut self, index: usize) -> Result<Vec<u8>, Error> {
        // Indexes into `entries` and `backup.files()` correspond because the
        // projection is a one-to-one map in order; take the entry from the
        // backup so the read carries the full metadata.
        let entry = self
            .backup
            .files()
            .get(index)
            .ok_or(Error::NoSuchEntry(index))?
            .clone();
        self.backup.read(&entry)
    }
}
