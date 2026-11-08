//! `ios-backup-forensic` — anomaly auditor for iOS device backups.
//!
//! Reads a backup through [`ios_backup_core`] and emits
//! [`forensicnomicon::report::Finding`]s. It states what is *observable in the
//! backup* and stops there: a backup can show that a domain is absent, and
//! cannot show why. Exclusion, an app that was never installed, and iOS
//! declining to back the app up all leave the same trace, so the finding names
//! the absence and leaves the question open.
//!
//! ```no_run
//! use ios_backup_core::{Backup, Credentials, Password};
//!
//! let backup = Backup::open_with(
//!     std::path::Path::new("/evidence/00008110-001641201A29401E"),
//!     &Credentials::password(Password::new("hunter2")),
//! )?;
//! for finding in ios_backup_forensic::audit(&backup) {
//!     println!("{} — {}", finding.code, finding.note);
//! }
//! # Ok::<(), ios_backup_core::Error>(())
//! ```

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod domains;
mod kinds;

pub use domains::EXPECTED_DOMAINS;
pub use kinds::AnomalyKind;

use std::collections::BTreeSet;

use forensicnomicon::report::{Finding, Observation, Source};
use ios_backup_core::{Backup, FileKind};

/// This analyzer's name and version, recorded on every finding so a report can
/// be reproduced against the exact code that produced it.
fn source(scope: String) -> Source {
    Source {
        analyzer: "ios-backup-forensic".to_owned(),
        scope,
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
    }
}

/// Audit `backup` and return every observation, most structural first.
///
/// The list is never empty for a readable backup: encryption state and device
/// provenance are recorded even when nothing is irregular, because silence is
/// indistinguishable from an analyzer that did not run.
#[must_use]
pub fn audit(backup: &Backup) -> Vec<Finding> {
    audit_with_cancel(backup, &|| false).unwrap_or_default()
}

/// LovelyCarve extension: poll cancellation during directory and manifest
/// traversal. A cancelled audit returns `None`, never a partial report.
#[must_use]
pub fn audit_with_cancel(backup: &Backup, cancelled: &dyn Fn() -> bool) -> Option<Vec<Finding>> {
    if cancelled() {
        return None;
    }
    let mut kinds = Vec::new();

    kinds.extend(encryption_state(backup));
    kinds.extend(encryption_contradiction(backup));
    kinds.extend(manifest_blob_integrity(backup, cancelled));
    kinds.extend(file_id_integrity(backup, cancelled));
    kinds.extend(absent_domains(backup, cancelled));
    kinds.extend(backup_completeness(backup));

    let scope = backup
        .metadata()
        .unique_identifier
        .clone()
        .unwrap_or_else(|| backup.root().display().to_string());

    let findings = kinds
        .iter()
        .take_while(|_| !cancelled())
        .map(|k| k.to_finding(source(scope.clone())))
        .collect();
    if cancelled() {
        None
    } else {
        Some(findings)
    }
}

/// Whether the backup is encrypted, and whether this run could read it.
fn encryption_state(backup: &Backup) -> Vec<AnomalyKind> {
    let metadata = backup.metadata();
    if metadata.is_encrypted {
        vec![AnomalyKind::EncryptedBackup {
            was_passcode_set: metadata.was_passcode_set,
        }]
    } else {
        // An unencrypted backup is a finding in its own right: it means the
        // keychain, health and Safari history a *encrypted* backup would carry
        // are absent, which changes what the evidence can be asked.
        vec![AnomalyKind::UnencryptedBackup]
    }
}

/// Whether `Manifest.plist` agrees with `Manifest.db` about encryption.
///
/// `IsEncrypted` is a declaration and the bytes are the fact; the reader keeps
/// both precisely so this comparison can be made.
fn encryption_contradiction(backup: &Backup) -> Vec<AnomalyKind> {
    let state = backup.encryption_state();
    if !state.is_contradictory() {
        return Vec::new();
    }
    vec![AnomalyKind::EncryptionStateContradiction {
        declared_encrypted: state.declared,
        observed_encrypted: state.observed,
        keybag_present: state.keybag_present,
    }]
}

/// Rows whose blob is missing, and blobs no row references.
fn manifest_blob_integrity(backup: &Backup, cancelled: &dyn Fn() -> bool) -> Vec<AnomalyKind> {
    let mut kinds = Vec::new();
    let mut referenced = BTreeSet::new();

    for entry in backup.files().iter().take_while(|_| !cancelled()) {
        if !entry.kind.has_content() {
            continue;
        }
        referenced.insert(entry.file_id.clone());

        let present = backup.blob_path(entry).is_some_and(|path| path.is_file());
        if !present {
            kinds.push(AnomalyKind::BlobMissing {
                file_id: entry.file_id.clone(),
                domain: entry.domain.clone(),
                relative_path: entry.relative_path.clone(),
                // The manifest may record no size; the finding says so rather
                // than printing a 0 the manifest never claimed.
                size: entry.size,
            });
        }
    }

    for file_id in on_disk_blobs(backup.root(), cancelled)
        .into_iter()
        .take_while(|_| !cancelled())
    {
        if !referenced.contains(&file_id) {
            kinds.push(AnomalyKind::BlobOrphan { file_id });
        }
    }
    kinds
}

/// Every blob filename present under the backup root's two-hex directories.
///
/// A directory that cannot be read is skipped rather than reported as empty:
/// claiming zero orphans when the scan failed would be refusal counted as zero.
fn on_disk_blobs(root: &std::path::Path, cancelled: &dyn Fn() -> bool) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };

    for entry in entries.flatten().take_while(|_| !cancelled()) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // Blob directories are exactly two hex characters.
        if name.len() != 2 || !name.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(blobs) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        for blob in blobs.flatten().take_while(|_| !cancelled()) {
            if let Some(blob_name) = blob.file_name().to_str() {
                found.insert(blob_name.to_owned());
            }
        }
    }
    found
}

/// `fileID` must be `SHA1(domain + "-" + relativePath)`.
///
/// A checkable invariant, and one of the few things in a backup that can be
/// verified without any external reference: a row that breaks it was written by
/// something other than the device.
fn file_id_integrity(backup: &Backup, cancelled: &dyn Fn() -> bool) -> Vec<AnomalyKind> {
    use sha1::{Digest, Sha1};

    backup
        .files()
        .iter()
        .take_while(|_| !cancelled())
        .filter_map(|entry| {
            let mut hasher = Sha1::new();
            hasher.update(entry.domain.as_bytes());
            hasher.update(b"-");
            hasher.update(entry.relative_path.as_bytes());
            let expected = hex::encode(hasher.finalize());

            (!entry.file_id.eq_ignore_ascii_case(&expected)).then(|| AnomalyKind::FileIdMismatch {
                file_id: entry.file_id.clone(),
                expected,
                domain: entry.domain.clone(),
                relative_path: entry.relative_path.clone(),
            })
        })
        .collect()
}

/// Domains an examiner would expect on a modern iPhone that this backup lacks.
fn absent_domains(backup: &Backup, cancelled: &dyn Fn() -> bool) -> Vec<AnomalyKind> {
    let present: BTreeSet<&str> = backup
        .files()
        .iter()
        .take_while(|_| !cancelled())
        .map(|f| f.domain.as_str())
        .collect();

    EXPECTED_DOMAINS
        .iter()
        .filter(|expected| !present.iter().any(|d| d.starts_with(expected.domain)))
        .map(|expected| AnomalyKind::ExpectedDomainAbsent {
            domain: expected.domain,
            what: expected.what,
        })
        .collect()
}

/// Whether the backup claims to have finished.
fn backup_completeness(backup: &Backup) -> Vec<AnomalyKind> {
    let metadata = backup.metadata();
    let mut kinds = Vec::new();

    if let Some(state) = &metadata.snapshot_state {
        if !state.eq_ignore_ascii_case("finished") {
            kinds.push(AnomalyKind::BackupNotFinished {
                snapshot_state: state.clone(),
            });
        }
    }

    let files = backup
        .files()
        .iter()
        .filter(|f| f.kind == FileKind::File)
        .count();
    if files == 0 && !backup.files().is_empty() {
        kinds.push(AnomalyKind::NoFileRows);
    }
    kinds
}
