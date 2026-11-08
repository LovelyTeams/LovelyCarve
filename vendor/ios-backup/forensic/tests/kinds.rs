//! Every `AnomalyKind` must render. A finding that panics or comes out empty
//! when a report asks for its note is a finding that fails at exactly the moment
//! it is needed.
//!
//! Regression tests, added after the implementation.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use forensicnomicon::report::{Observation, Severity, Source};
use ios_backup_forensic::{AnomalyKind, EXPECTED_DOMAINS};

/// One of every variant.
fn every_kind() -> Vec<AnomalyKind> {
    vec![
        AnomalyKind::EncryptedBackup {
            was_passcode_set: true,
        },
        AnomalyKind::EncryptedBackup {
            was_passcode_set: false,
        },
        AnomalyKind::UnencryptedBackup,
        AnomalyKind::BlobMissing {
            file_id: "3d0d7e5fb2ce288813306e4d4636395e047a3d28".into(),
            domain: "HomeDomain".into(),
            relative_path: "Library/SMS/sms.db".into(),
            size: Some(316),
        },
        AnomalyKind::BlobOrphan {
            file_id: "ff00000000000000000000000000000000000000".into(),
        },
        AnomalyKind::FileIdMismatch {
            file_id: "0000000000000000000000000000000000000000".into(),
            expected: "3d0d7e5fb2ce288813306e4d4636395e047a3d28".into(),
            domain: "HomeDomain".into(),
            relative_path: "Library/SMS/sms.db".into(),
        },
        AnomalyKind::ExpectedDomainAbsent {
            domain: "AppDomainGroup-group.net.whatsapp",
            what: "WhatsApp messages and media",
        },
        AnomalyKind::BackupNotFinished {
            snapshot_state: "in-progress".into(),
        },
        AnomalyKind::NoFileRows,
        AnomalyKind::EncryptionStateContradiction {
            declared_encrypted: false,
            observed_encrypted: true,
            keybag_present: true,
        },
        AnomalyKind::EncryptionStateContradiction {
            declared_encrypted: true,
            observed_encrypted: false,
            keybag_present: false,
        },
    ]
}

fn source() -> Source {
    Source {
        analyzer: "ios-backup-forensic".into(),
        scope: "test".into(),
        version: Some("0.1.0".into()),
    }
}

#[test]
fn every_kind_renders_a_substantial_note() {
    for kind in every_kind() {
        let note = kind.note();
        assert!(
            note.len() > 40,
            "{} rendered a note too short to be useful: {note:?}",
            kind.code()
        );
        assert!(
            !note.contains("{}") && !note.contains("undefined") && !note.contains("NaN"),
            "{} rendered a broken note: {note}",
            kind.code()
        );
    }
}

#[test]
fn every_code_is_scheme_prefixed_and_unique() {
    let codes: Vec<&str> = every_kind().iter().map(Observation::code).collect();

    for code in &codes {
        assert!(
            code.starts_with("IOS-BACKUP-"),
            "codes are scheme-prefixed so a merged report stays attributable; \
             got {code}"
        );
    }
    // EncryptedBackup appears twice by design (both passcode states).
    let mut unique: Vec<&str> = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 9, "one code per variant; got {codes:?}");
}

#[test]
fn every_kind_assembles_into_a_finding_carrying_its_evidence() {
    for kind in every_kind() {
        let finding = kind.to_finding(source());

        assert_eq!(finding.code, kind.code());
        assert_eq!(finding.severity, kind.severity());
        assert!(!finding.note.is_empty());

        // Every evidence row must name a field and carry a value. A row with an
        // empty value is a claim the exhibit cannot support.
        for evidence in &finding.evidence {
            assert!(!evidence.field.is_empty(), "{}", kind.code());
            assert!(
                !evidence.value.is_empty(),
                "{} has an empty value for {}",
                kind.code(),
                evidence.field
            );
        }
    }
}

#[test]
fn only_the_absent_domain_kind_is_unrated() {
    for kind in every_kind() {
        match kind {
            AnomalyKind::ExpectedDomainAbsent { .. } => assert_eq!(
                kind.severity(),
                None,
                "an absent domain has no severity the evidence can support"
            ),
            _ => assert!(
                kind.severity().is_some(),
                "{} is unrated but has no documented reason to be",
                kind.code()
            ),
        }
    }
}

#[test]
fn the_file_id_invariant_is_the_only_high() {
    // fileID = SHA1(domain-relativePath) is checkable with no external
    // reference, so breaking it is the strongest structural claim this analyzer
    // makes. Nothing else should outrank it.
    for kind in every_kind() {
        if matches!(kind.severity(), Some(Severity::High | Severity::Critical)) {
            assert!(
                matches!(kind, AnomalyKind::FileIdMismatch { .. }),
                "{} is graded {:?}; only the fileID invariant is that strong",
                kind.code(),
                kind.severity()
            );
        }
    }
}

#[test]
fn the_expected_domain_list_is_well_formed() {
    assert!(!EXPECTED_DOMAINS.is_empty());
    for expected in EXPECTED_DOMAINS {
        assert!(!expected.domain.is_empty());
        assert!(
            !expected.what.is_empty(),
            "{} must say what it holds, or the finding cannot explain itself",
            expected.domain
        );
    }
}
