//! The analyzer's typed observations, and their mapping onto the fleet's
//! normalized report model.
//!
//! Keeping a typed enum rather than building `Finding`s directly is the fleet
//! pattern (ADR-0007): the analyzer reasons in its own vocabulary and converts
//! once, at the boundary, through [`Observation`].
//!
//! # On severity
//!
//! Several kinds here are deliberately **unrated**. `None` is not "we forgot to
//! score it" — it is the analyzer declining to grade something the evidence
//! cannot support a grade for. A missing app domain is the clearest case: it is
//! equally consistent with deliberate exclusion, an app that was never
//! installed, and iOS choosing not to back the app up. Scoring it would import
//! a judgement the backup does not contain.

use forensicnomicon::report::{Category, Evidence, Observation, Severity};

/// Something the analyzer observed in a backup.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnomalyKind {
    /// The backup is encrypted.
    EncryptedBackup {
        /// Whether a device passcode was set when it was taken.
        was_passcode_set: bool,
    },

    /// The backup is not encrypted.
    ///
    /// Reportable in its own right: an unencrypted backup omits the keychain,
    /// Health and Safari history that an encrypted one carries, so it changes
    /// what the evidence can be asked, not merely how it is read.
    UnencryptedBackup,

    /// A `Files` row names a content blob that is not present on disk.
    BlobMissing {
        /// The row's `fileID`, which is also the blob's filename.
        file_id: String,
        /// The row's domain.
        domain: String,
        /// The row's path within that domain.
        relative_path: String,
        /// The size the manifest records for the absent blob, or `None` when
        /// it records none.
        size: Option<u64>,
    },

    /// A blob exists on disk that no `Files` row references.
    BlobOrphan {
        /// The blob's filename.
        file_id: String,
    },

    /// A `fileID` is not `SHA1(domain + "-" + relativePath)`.
    FileIdMismatch {
        /// The `fileID` as recorded.
        file_id: String,
        /// The value the row's own domain and path hash to.
        expected: String,
        /// The row's domain.
        domain: String,
        /// The row's path within that domain.
        relative_path: String,
    },

    /// A domain an examiner would expect on a modern iPhone is absent.
    ExpectedDomainAbsent {
        /// The domain prefix that was looked for.
        domain: &'static str,
        /// What that domain holds, in examiner terms.
        what: &'static str,
    },

    /// `Status.plist` does not record the backup as finished.
    BackupNotFinished {
        /// The `SnapshotState` value as recorded.
        snapshot_state: String,
    },

    /// The manifest has rows but none of them is a file.
    NoFileRows,

    /// `Manifest.plist`'s `IsEncrypted` disagrees with `Manifest.db` itself.
    EncryptionStateContradiction {
        /// What the plist declared.
        declared_encrypted: bool,
        /// What the bytes turned out to be.
        observed_encrypted: bool,
        /// Whether a `BackupKeyBag` was present.
        keybag_present: bool,
    },
}

impl Observation for AnomalyKind {
    fn severity(&self) -> Option<Severity> {
        match self {
            // Provenance: true of the backup, not suspicious in itself.
            Self::EncryptedBackup { .. } | Self::UnencryptedBackup => Some(Severity::Info),

            // A manifest that disagrees with the bytes on disk is a structural
            // contradiction — the index and the content cannot both be right.
            Self::BlobMissing { .. } => Some(Severity::Medium),
            Self::BlobOrphan { .. } => Some(Severity::Low),

            // The one invariant a backup can be checked against with no
            // external reference. A row that breaks it was not written by the
            // device's own backup process.
            Self::FileIdMismatch { .. } => Some(Severity::High),

            Self::BackupNotFinished { .. } | Self::NoFileRows => Some(Severity::Medium),

            // A plist that disagrees with its own data. Medium rather than
            // High: it is a real contradiction, but a partial copy or a tool
            // that rewrote metadata separately from data explains it as well as
            // tampering does, and the backup reveals no way to tell which.
            Self::EncryptionStateContradiction { .. } => Some(Severity::Medium),

            // Deliberately unrated: see the module note. Absence of a domain is
            // a lead, and the backup holds nothing that distinguishes its
            // possible causes.
            Self::ExpectedDomainAbsent { .. } => None,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::EncryptedBackup { .. } => "IOS-BACKUP-ENCRYPTED",
            Self::UnencryptedBackup => "IOS-BACKUP-UNENCRYPTED",
            Self::BlobMissing { .. } => "IOS-BACKUP-BLOB-MISSING",
            Self::BlobOrphan { .. } => "IOS-BACKUP-BLOB-ORPHAN",
            Self::FileIdMismatch { .. } => "IOS-BACKUP-FILEID-MISMATCH",
            Self::ExpectedDomainAbsent { .. } => "IOS-BACKUP-DOMAIN-ABSENT",
            Self::BackupNotFinished { .. } => "IOS-BACKUP-NOT-FINISHED",
            Self::NoFileRows => "IOS-BACKUP-NO-FILE-ROWS",
            Self::EncryptionStateContradiction { .. } => {
                "IOS-BACKUP-ENCRYPTION-STATE-CONTRADICTION"
            }
        }
    }

    fn category(&self) -> Category {
        match self {
            Self::EncryptedBackup { .. } | Self::UnencryptedBackup => Category::Provenance,
            Self::BlobMissing { .. }
            | Self::BlobOrphan { .. }
            | Self::FileIdMismatch { .. }
            | Self::NoFileRows
            | Self::BackupNotFinished { .. }
            | Self::EncryptionStateContradiction { .. } => Category::Integrity,
            // What a backup does *not* contain is a question about coverage,
            // which is the medium's biography rather than its integrity.
            Self::ExpectedDomainAbsent { .. } => Category::History,
        }
    }

    fn note(&self) -> String {
        match self {
            Self::EncryptedBackup { was_passcode_set } => format!(
                "Backup is encrypted (Manifest.plist IsEncrypted = true); a device \
                 passcode was {} set when it was taken. An encrypted backup carries \
                 the keychain, Health and Safari history that an unencrypted one omits.",
                if *was_passcode_set { "" } else { "not" }
            ),
            Self::UnencryptedBackup => {
                "Backup is not encrypted (Manifest.plist IsEncrypted = false). \
                 Consistent with a backup taken without a backup password; the \
                 keychain, Health data and Safari history that an encrypted backup \
                 carries are not present in this one."
                    .to_owned()
            }
            Self::BlobMissing {
                file_id,
                domain,
                relative_path,
                size,
            } => format!(
                "Manifest row {domain}-{relative_path} (fileID {file_id}) records {}, \
                 but no content blob is present at {}/{file_id}. \
                 Consistent with an interrupted backup, a partial copy of the backup \
                 directory, or removal of the blob after the manifest was written.",
                // Say what the manifest recorded, including when it recorded no
                // size at all. Printing "0 bytes" for an absent size would state
                // a figure the manifest never claimed.
                match size {
                    Some(bytes) => format!("a {bytes}-byte file"),
                    None => "a file of unrecorded length".to_owned(),
                },
                file_id.get(..2).unwrap_or("??")
            ),
            Self::BlobOrphan { file_id } => format!(
                "Content blob {file_id} is present on disk but no Manifest.db Files \
                 row references it. Consistent with a manifest written before the \
                 blob was removed from it, or with a blob left by an earlier backup."
            ),
            Self::FileIdMismatch {
                file_id,
                expected,
                domain,
                relative_path,
            } => format!(
                "Manifest row {domain}-{relative_path} records fileID {file_id}, but \
                 SHA1(\"{domain}-{relative_path}\") is {expected}. The identifier a \
                 backup derives for a file does not match the one recorded, which is \
                 not consistent with a manifest written by the device's own backup \
                 process."
            ),
            Self::ExpectedDomainAbsent { domain, what } => format!(
                "No file in this backup belongs to the {domain} domain ({what}). A \
                 backup records what was captured and not why something was not, so \
                 this is equally consistent with the app never having been installed, \
                 with the app's data being excluded from the backup, and with iOS \
                 declining to back it up on this version."
            ),
            Self::BackupNotFinished { snapshot_state } => format!(
                "Status.plist records SnapshotState = \"{snapshot_state}\" rather than \
                 \"finished\". Consistent with a backup that was interrupted, which \
                 would also explain any missing content blobs."
            ),
            Self::EncryptionStateContradiction {
                declared_encrypted,
                observed_encrypted,
                keybag_present,
            } => format!(
                "Manifest.plist records IsEncrypted = {declared_encrypted}, but Manifest.db \
                 {} a plaintext SQLite database, and {}. The backup's metadata disagrees \
                 with its own data. Consistent with a partial copy of the backup, with \
                 metadata written separately from the data, or with the property list \
                 having been altered after the backup was taken; the backup records \
                 nothing that distinguishes these.",
                if *observed_encrypted { "is not" } else { "is" },
                if *keybag_present {
                    "a BackupKeyBag is present"
                } else {
                    "no BackupKeyBag is present"
                }
            ),
            Self::NoFileRows => {
                "Manifest.db Files table has rows, but none of them describes a file \
                 with content. Consistent with a directory-only manifest or a partial \
                 manifest read."
                    .to_owned()
            }
        }
    }

    fn evidence(&self) -> Vec<Evidence> {
        let row = |field: &str, value: String| Evidence {
            field: field.to_owned(),
            value,
            location: None,
        };
        match self {
            Self::EncryptedBackup { was_passcode_set } => vec![
                row("IsEncrypted", "true".to_owned()),
                row("WasPasscodeSet", was_passcode_set.to_string()),
            ],
            Self::UnencryptedBackup => vec![row("IsEncrypted", "false".to_owned())],
            Self::BlobMissing {
                file_id,
                domain,
                relative_path,
                size,
            } => vec![
                row("fileID", file_id.clone()),
                row("domain", domain.clone()),
                row("relativePath", relative_path.clone()),
                row(
                    "recordedSize",
                    // "(not recorded)" rather than "0": an exhibit that prints a
                    // figure the manifest never carried is asserting one.
                    size.map_or_else(|| "(not recorded)".to_owned(), |b| b.to_string()),
                ),
            ],
            Self::BlobOrphan { file_id } => vec![row("fileID", file_id.clone())],
            Self::FileIdMismatch {
                file_id,
                expected,
                domain,
                relative_path,
            } => vec![
                row("recordedFileID", file_id.clone()),
                row("computedFileID", expected.clone()),
                row("domain", domain.clone()),
                row("relativePath", relative_path.clone()),
            ],
            Self::ExpectedDomainAbsent { domain, what } => vec![
                row("domain", (*domain).to_owned()),
                row("holds", (*what).to_owned()),
            ],
            Self::BackupNotFinished { snapshot_state } => {
                vec![row("SnapshotState", snapshot_state.clone())]
            }
            Self::NoFileRows => Vec::new(),
            Self::EncryptionStateContradiction {
                declared_encrypted,
                observed_encrypted,
                keybag_present,
            } => vec![
                row("IsEncrypted", declared_encrypted.to_string()),
                row("manifestActuallyEncrypted", observed_encrypted.to_string()),
                row("BackupKeyBagPresent", keybag_present.to_string()),
            ],
        }
    }
}
