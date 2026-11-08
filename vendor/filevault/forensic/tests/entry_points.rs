//! Cover the reader-based analyzer entry points (`audit`, `audit_path`,
//! `audit_findings`) and the `EncryptionState` / `WeakKdfIterations` finding
//! arms WITHOUT the env-gated oracle image, so the `--workspace` coverage gate
//! passes in CI (which runs without `FVDE_ORACLE_IMAGE`).
//!
//! These are real behavior tests: they build a minimal but structurally valid
//! synthetic CoreStorage image (a `0x0011` pointer block → an AES-XTS-encrypted
//! metadata region carrying the encryption-context plist) that `parse_info`
//! accepts, then assert the findings' codes, severities, and evidence. The
//! synthetic image is Tier-3 scaffolding under the Tier-1 oracle — it exercises
//! the analyzer plumbing, not the crypto (which the oracle test validates).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::doc_markdown)]

use std::io::Cursor;

use forensicnomicon::report::Severity;

fn synthetic_image(iterations: u32, status: &str) -> Vec<u8> {
    filevault::test_support::build_image_with_parameters(iterations, status)
}

#[test]
fn audit_over_synthetic_image_yields_all_kinds() {
    let img = synthetic_image(1000, "Converting");
    let anomalies =
        filevault_forensic::audit(Cursor::new(img)).expect("audit parses synthetic image");
    let codes: Vec<&str> = anomalies.iter().map(|a| a.code).collect();
    assert!(codes.contains(&"FVDE-PROTECTOR-INVENTORY"));
    assert!(codes.contains(&"FVDE-ENCRYPTION-STATE"));
    assert!(codes.contains(&"FVDE-WEAK-KDF-ITERATIONS"));

    let state = anomalies
        .iter()
        .find(|a| a.code == "FVDE-ENCRYPTION-STATE")
        .unwrap();
    assert!(state.note.contains("Converting"));
    let weak = anomalies
        .iter()
        .find(|a| a.code == "FVDE-WEAK-KDF-ITERATIONS")
        .unwrap();
    assert_eq!(weak.severity, Severity::Medium);
    assert!(weak.note.contains("1000"));
}

#[test]
fn audit_findings_maps_every_kind_with_evidence() {
    // Exercises audit_findings() AND the per-kind evidence()/category() arms
    // (EncryptionState, WeakKdfIterations) via Observation::to_finding.
    let img = synthetic_image(1000, "Converting");
    let findings = filevault_forensic::audit_findings(Cursor::new(img), "synthetic.raw")
        .expect("audit_findings");
    assert_eq!(findings.len(), 3);
    for f in &findings {
        assert_eq!(f.source.analyzer, "filevault-forensic");
        assert_eq!(f.source.scope, "synthetic.raw");
        assert!(!f.evidence.is_empty(), "{} carries evidence", f.code);
    }
    let state = findings
        .iter()
        .find(|f| f.code == "FVDE-ENCRYPTION-STATE")
        .unwrap();
    assert!(state
        .evidence
        .iter()
        .any(|e| e.field == "conversion_status"));
    let weak = findings
        .iter()
        .find(|f| f.code == "FVDE-WEAK-KDF-ITERATIONS")
        .unwrap();
    assert!(weak.evidence.iter().any(|e| e.field == "pbkdf2_iterations"));
}

#[test]
fn audit_path_reads_a_file() {
    let img = synthetic_image(50_000, "Complete");
    let mut path = std::env::temp_dir();
    path.push(format!("fvde_synth_{}.raw", std::process::id()));
    std::fs::write(&path, &img).unwrap();

    let anomalies = filevault_forensic::audit_path(&path).expect("audit_path parses the file");
    // 50000 iterations is above the weak threshold → not flagged.
    assert!(anomalies
        .iter()
        .all(|a| a.code != "FVDE-WEAK-KDF-ITERATIONS"));
    assert!(anomalies
        .iter()
        .any(|a| a.code == "FVDE-PROTECTOR-INVENTORY"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn audit_path_missing_file_errors() {
    let err = filevault_forensic::audit_path(std::path::Path::new(
        "/nonexistent/fvde/definitely/missing.raw",
    ));
    assert!(err.is_err());
}
