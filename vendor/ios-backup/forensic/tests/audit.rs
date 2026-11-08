//! The analyzer's observations over the minted fixtures.
//!
//! Every assertion here is about an *observation*, never a conclusion. A
//! missing app domain is reported as a missing app domain; whether that means
//! someone excluded it, the app was never installed, or iOS declined to back it
//! up is not something a backup can settle, and the analyzer does not pretend
//! otherwise.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ios_backup_core::{Backup, Credentials, Password};
use ios_backup_forensic::{audit, AnomalyKind};

const PASSWORD: &str = "test-password-1234";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

fn plain() -> Backup {
    Backup::open(&fixture("plain-backup")).unwrap()
}

#[test]
fn cancellation_during_audit_never_returns_partial_findings() {
    let polls = std::cell::Cell::new(0);
    let findings = ios_backup_forensic::audit_with_cancel(&plain(), &|| {
        polls.set(polls.get() + 1);
        polls.get() > 3
    });
    assert!(findings.is_none());
}

/// Codes present in a findings list, for readable assertions.
fn codes(findings: &[forensicnomicon::report::Finding]) -> Vec<String> {
    findings.iter().map(|f| f.code.to_string()).collect()
}

#[test]
fn a_healthy_backup_still_reports_its_provenance() {
    // Not "no findings": an examiner needs the encryption state and the device
    // context recorded even when nothing is wrong. Silence would be
    // indistinguishable from an analyzer that never ran.
    let findings = audit(&plain());

    assert!(
        codes(&findings).iter().any(|c| c.contains("UNENCRYPTED")),
        "an unencrypted backup is itself a reportable fact; got {:?}",
        codes(&findings)
    );
}

#[test]
fn an_encrypted_backup_that_was_unlocked_is_recorded_as_such() {
    let backup = Backup::open_with(
        &fixture("encrypted-backup"),
        &Credentials::password(Password::new(PASSWORD)),
    )
    .unwrap();

    let findings = audit(&backup);
    assert!(
        codes(&findings).iter().any(|c| c.contains("ENCRYPTED")),
        "got {:?}",
        codes(&findings)
    );
}

#[test]
fn a_backup_whose_plist_disagrees_with_its_data_is_reported() {
    // Both directions. The reader recovers from the disagreement so the backup
    // still opens; the analyzer is where the disagreement gets said out loud.
    for (name, creds) in [
        (
            "lie-unencrypted",
            Credentials::password(Password::new(PASSWORD)),
        ),
        ("lie-encrypted", Credentials::none()),
    ] {
        let backup = Backup::open_with(&fixture(name), &creds).unwrap();
        let findings = audit(&backup);

        let contradiction = findings
            .iter()
            .find(|f| f.code.contains("ENCRYPTION-STATE-CONTRADICTION"))
            .unwrap_or_else(|| panic!("{name}: got {:?}", codes(&findings)));

        // The exhibit must carry both values, or the reader cannot see what
        // disagreed with what.
        let fields: Vec<&str> = contradiction
            .evidence
            .iter()
            .map(|e| e.field.as_str())
            .collect();
        assert!(fields.contains(&"IsEncrypted"), "{name}: {fields:?}");
        assert!(
            fields.contains(&"manifestActuallyEncrypted"),
            "{name}: {fields:?}"
        );
    }
}

#[test]
fn an_honest_backup_reports_no_contradiction() {
    // The negative control: this finding must not fire on every backup, or it
    // is noise wearing a severity.
    let findings = audit(&plain());
    assert!(
        !codes(&findings)
            .iter()
            .any(|c| c.contains("ENCRYPTION-STATE-CONTRADICTION")),
        "got {:?}",
        codes(&findings)
    );
}

#[test]
fn every_finding_names_this_analyzer_as_its_source() {
    for finding in audit(&plain()) {
        assert_eq!(finding.source.analyzer, "ios-backup-forensic");
        assert!(
            finding.source.version.is_some(),
            "a court-facing finding records the analyzer version"
        );
    }
}

#[test]
fn a_manifest_row_whose_blob_is_absent_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture("plain-backup");
    for name in [
        "Manifest.db",
        "Manifest.plist",
        "Status.plist",
        "Info.plist",
    ] {
        std::fs::copy(source.join(name), dir.path().join(name)).unwrap();
    }
    // Blob directories deliberately not copied: every Files row is now an
    // orphan pointing at a blob that is not there.
    let backup = Backup::open(dir.path()).unwrap();
    let findings = audit(&backup);

    let missing: Vec<_> = findings
        .iter()
        .filter(|f| f.code.contains("BLOB-MISSING"))
        .collect();
    assert_eq!(
        missing.len(),
        4,
        "one finding per file row with no blob; got {:?}",
        codes(&findings)
    );
    // The finding must carry the fileID verbatim — a full 40-character hex
    // identifier, never elided. Without it the reader cannot locate the row
    // the finding is about.
    let file_id = missing[0]
        .evidence
        .iter()
        .find(|e| e.field == "fileID")
        .expect("the finding records a fileID");
    assert_eq!(file_id.value.len(), 40, "got: {}", file_id.value);
    assert!(file_id.value.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn a_blob_with_no_manifest_row_is_reported_as_an_orphan() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture("plain-backup");
    for entry in std::fs::read_dir(&source).unwrap() {
        let entry = entry.unwrap();
        let target = dir.path().join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            for blob in std::fs::read_dir(entry.path()).unwrap() {
                let blob = blob.unwrap();
                std::fs::copy(blob.path(), target.join(blob.file_name())).unwrap();
            }
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
    // A blob the manifest never mentions.
    let orphan_dir = dir.path().join("ff");
    std::fs::create_dir_all(&orphan_dir).unwrap();
    std::fs::write(
        orphan_dir.join("ff00000000000000000000000000000000000000"),
        b"unreferenced",
    )
    .unwrap();

    let backup = Backup::open(dir.path()).unwrap();
    let findings = audit(&backup);

    assert!(
        codes(&findings).iter().any(|c| c.contains("BLOB-ORPHAN")),
        "got {:?}",
        codes(&findings)
    );
}

#[test]
fn a_file_id_that_is_not_the_sha1_of_its_own_key_is_reported() {
    // fileID = SHA1(domain + "-" + relativePath) is a checkable invariant, and
    // a row that breaks it means the manifest was edited after the fact.
    let kind = AnomalyKind::FileIdMismatch {
        file_id: "0000000000000000000000000000000000000000".into(),
        expected: "3d0d7e5fb2ce288813306e4d4636395e047a3d28".into(),
        domain: "HomeDomain".into(),
        relative_path: "Library/SMS/sms.db".into(),
    };

    let note = forensicnomicon::report::Observation::note(&kind);
    assert!(
        note.contains("3d0d7e5f"),
        "the note must carry the expected value verbatim; got: {note}"
    );
}

#[test]
fn findings_are_observations_not_conclusions() {
    // A forensic finding describes what was seen. Verdict words in an
    // analyzer's own note are how an observation becomes an unearned
    // conclusion by the time it reaches a report.
    const VERDICT_WORDS: [&str; 6] = [
        "proves",
        "deliberately",
        "tampered",
        "guilty",
        "intentional",
        "concealed by",
    ];

    for finding in audit(&plain()) {
        let note = finding.note.to_lowercase();
        for word in VERDICT_WORDS {
            assert!(
                !note.contains(word),
                "finding {} states a conclusion ({word}): {}",
                finding.code,
                finding.note
            );
        }
    }
}

#[test]
fn an_expected_domain_that_is_absent_is_reported_without_explaining_it() {
    // The case-relevant observation: a secure-messenger domain missing from a
    // modern-iOS backup. Reported as absent, with no claim about why.
    let findings = audit(&plain());

    let absent: Vec<_> = findings
        .iter()
        .filter(|f| f.code.contains("DOMAIN-ABSENT"))
        .collect();
    assert!(
        !absent.is_empty(),
        "the fixture has no WhatsApp/Signal domains, so their absence is \
         reportable; got {:?}",
        codes(&findings)
    );
    for finding in absent {
        assert!(
            finding.severity.is_none(),
            "absence of a domain has no severity: it is a lead for the \
             examiner, not a graded anomaly. {} was graded {:?}",
            finding.code,
            finding.severity
        );
    }
}
