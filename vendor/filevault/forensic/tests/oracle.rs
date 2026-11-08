//! Real-data validation of the reader-based analyzer entry points against the
//! dfvfs `fvdetest` CoreStorage oracle. Env-gated on `FVDE_ORACLE_IMAGE`
//! (carved CS partition); skips cleanly when unset.
//!
//! The hermetic synthetic-image test (`entry_points.rs`) already covers these
//! wrappers' *lines*; this test additionally exercises them over the real ~33 MB
//! metadata structure so CI validates the full parse path on genuine data, not a
//! fixture. Runs in CI, which provides the image (see `.github/workflows/ci.yml`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::doc_markdown)]

use std::fs::File;
use std::path::Path;

fn oracle_path() -> Option<String> {
    std::env::var("FVDE_ORACLE_IMAGE").ok()
}

#[test]
fn audit_path_on_real_image() {
    let Some(path) = oracle_path() else {
        return;
    };
    let anomalies = filevault_forensic::audit_path(Path::new(&path)).expect("audit_path oracle");
    // The oracle is fully encrypted (ConversionStatus=Complete), password
    // protector present, PBKDF2 = 90506 (above the weak threshold).
    let codes: Vec<&str> = anomalies.iter().map(|a| a.code).collect();
    assert!(codes.contains(&"FVDE-PROTECTOR-INVENTORY"));
    assert!(codes.contains(&"FVDE-ENCRYPTION-STATE"));
    assert!(
        !codes.contains(&"FVDE-WEAK-KDF-ITERATIONS"),
        "90506 iterations must not be flagged weak"
    );
    let state = anomalies
        .iter()
        .find(|a| a.code == "FVDE-ENCRYPTION-STATE")
        .unwrap();
    assert!(state.note.contains("Complete"));
}

#[test]
fn audit_findings_on_real_image() {
    let Some(path) = oracle_path() else {
        return;
    };
    let findings =
        filevault_forensic::audit_findings(File::open(&path).unwrap(), path.clone()).unwrap();
    assert!(!findings.is_empty());
    for f in &findings {
        assert_eq!(f.source.analyzer, "filevault-forensic");
        assert_eq!(f.source.scope, path);
    }
}
