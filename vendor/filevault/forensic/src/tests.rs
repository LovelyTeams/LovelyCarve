//! Unit tests for the anomaly auditor over synthetic `FileVaultInfo`.

use super::*;
use filevault::{Protector, ProtectorKind};

fn info(iterations: u32, status: Option<&str>, kinds: &[ProtectorKind]) -> FileVaultInfo {
    FileVaultInfo {
        physical_volume_identifier: "3273a055-3b8b-47e8-b970-df35eecda81b".to_string(),
        pbkdf2_iterations: iterations,
        pbkdf2_salt: [0u8; 16],
        family_uuid: "1F01CA34-5F6C-4123-AC0C-B0A256889DB2".to_string(),
        lv_identifier: Some("420AF122-CF73-4A30-8B0A-A593A65FBEF5".to_string()),
        lv_name: Some("TestLV".to_string()),
        lv_size: 167_772_160,
        encryption_method: "AES-XTS-128",
        conversion_status: status.map(str::to_string),
        protectors: kinds
            .iter()
            .map(|&kind| Protector { user_type: 0, kind })
            .collect(),
    }
}

#[test]
fn reports_protector_inventory() {
    let anomalies = audit_info(&info(90506, Some("Complete"), &[ProtectorKind::Password]));
    let inv = anomalies
        .iter()
        .find(|a| a.code == "FVDE-PROTECTOR-INVENTORY")
        .expect("protector inventory finding");
    assert_eq!(inv.severity, Severity::Info);
    assert!(inv.note.contains("password"));
    assert!(inv.note.contains('1'));
}

#[test]
fn protector_summary_is_sorted_and_deduped_labels() {
    let anomalies = audit_info(&info(
        90506,
        Some("Complete"),
        &[ProtectorKind::Recovery, ProtectorKind::Password],
    ));
    let inv = anomalies
        .iter()
        .find(|a| a.code == "FVDE-PROTECTOR-INVENTORY")
        .unwrap();
    // Sorted: "password, recovery".
    assert!(inv.note.contains("password, recovery"));
}

#[test]
fn empty_protectors_reports_none() {
    let anomalies = audit_info(&info(90506, None, &[]));
    let inv = anomalies
        .iter()
        .find(|a| a.code == "FVDE-PROTECTOR-INVENTORY")
        .unwrap();
    assert!(inv.note.contains("none"));
}

#[test]
fn reports_encryption_state() {
    let anomalies = audit_info(&info(90506, Some("Complete"), &[ProtectorKind::Password]));
    let state = anomalies
        .iter()
        .find(|a| a.code == "FVDE-ENCRYPTION-STATE")
        .expect("encryption state finding");
    assert_eq!(state.severity, Severity::Info);
    assert!(state.note.contains("Complete"));
}

#[test]
fn no_conversion_status_omits_encryption_state() {
    let anomalies = audit_info(&info(90506, None, &[ProtectorKind::Password]));
    assert!(anomalies.iter().all(|a| a.code != "FVDE-ENCRYPTION-STATE"));
}

#[test]
fn fvdetest_iterations_not_flagged_weak() {
    // 90506 (fvdetest) must NOT be flagged — that is the correct behaviour.
    let anomalies = audit_info(&info(90506, Some("Complete"), &[ProtectorKind::Password]));
    assert!(anomalies
        .iter()
        .all(|a| a.code != "FVDE-WEAK-KDF-ITERATIONS"));
}

#[test]
fn low_iterations_flagged_weak() {
    let anomalies = audit_info(&info(1000, Some("Complete"), &[ProtectorKind::Password]));
    let weak = anomalies
        .iter()
        .find(|a| a.code == "FVDE-WEAK-KDF-ITERATIONS")
        .expect("weak KDF finding");
    assert_eq!(weak.severity, Severity::Medium);
    assert!(weak.note.contains("1000"));
    assert_eq!(weak.kind.category(), Category::Integrity);
}

#[test]
fn observation_trait_maps_to_finding() {
    let anomalies = audit_info(&info(90506, Some("Complete"), &[ProtectorKind::Password]));
    let source = Source {
        analyzer: ANALYZER.to_string(),
        scope: "test.raw".to_string(),
        version: None,
    };
    let finding = anomalies[0].to_finding(source);
    assert_eq!(finding.code, "FVDE-PROTECTOR-INVENTORY");
    assert_eq!(finding.source.analyzer, "filevault-forensic");
    assert!(!finding.evidence.is_empty());
}

#[test]
fn mitre_and_subjects_are_empty() {
    let a = Anomaly::new(AnomalyKind::EncryptionState {
        status: "Complete".to_string(),
    });
    assert!(a.mitre().is_empty());
    assert!(Observation::subjects(&a).is_empty());
    assert_eq!(Observation::code(&a), "FVDE-ENCRYPTION-STATE");
    assert_eq!(Observation::note(&a), a.note);
    assert_eq!(Observation::severity(&a), Some(Severity::Info));
}
