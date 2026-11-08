//! # filevault-forensic — CoreStorage / FileVault 2 anomaly auditor
//!
//! Audits a CoreStorage / FileVault volume WITHOUT the password, emitting
//! severity-graded [`forensicnomicon::report::Finding`]s over the metadata
//! parsed by [`filevault`]. Findings are OBSERVATIONS, never verdicts: a low
//! PBKDF2 iteration count is reported as *consistent with* weak key-stretching,
//! not a determination of compromise.
//!
//! ```no_run
//! use std::path::Path;
//!
//! for anomaly in filevault_forensic::audit_path(Path::new("cs_partition.raw"))? {
//!     println!("{}: {}", anomaly.code, anomaly.note);
//! }
//! # Ok::<(), filevault::FileVaultError>(())
//! ```

#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::{Read, Seek};
use std::path::Path;

use filevault::{FileVaultError, FileVaultInfo};
use forensicnomicon::report::{
    Category, Evidence, Finding, Observation, Severity, Source, SubjectRef,
};

#[cfg(test)]
mod tests;

/// The producing analyzer name embedded in emitted findings' `Source`.
pub const ANALYZER: &str = "filevault-forensic";

/// PBKDF2 iteration counts below this are flagged as notably weak. Modern
/// FileVault provisions tens of thousands of rounds (fvdetest = 90506); a count
/// well under 20000 is a defensible "notably low" threshold.
const WEAK_KDF_THRESHOLD: u32 = 20_000;

/// A classified CoreStorage / FileVault forensic anomaly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnomalyKind {
    /// The protectors (crypto users) present on the volume.
    ProtectorInventory {
        /// Comma-separated protector kinds (password / recovery / …).
        summary: String,
        /// Number of protectors.
        count: usize,
    },
    /// The logical volume's conversion (encryption) state.
    EncryptionState {
        /// The raw `ConversionStatus` string (`Complete` / `Converting` / …).
        status: String,
    },
    /// The PBKDF2 iteration count is notably low.
    WeakKdfIterations {
        /// Observed iteration count.
        iterations: u32,
    },
}

impl AnomalyKind {
    /// Severity — the single source of truth for this kind.
    #[must_use]
    pub fn severity(&self) -> Severity {
        match self {
            AnomalyKind::ProtectorInventory { .. } | AnomalyKind::EncryptionState { .. } => {
                Severity::Info
            }
            AnomalyKind::WeakKdfIterations { .. } => Severity::Medium,
        }
    }

    /// Stable, scheme-prefixed machine code (published contract).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            AnomalyKind::ProtectorInventory { .. } => "FVDE-PROTECTOR-INVENTORY",
            AnomalyKind::EncryptionState { .. } => "FVDE-ENCRYPTION-STATE",
            AnomalyKind::WeakKdfIterations { .. } => "FVDE-WEAK-KDF-ITERATIONS",
        }
    }

    /// Analytical lens.
    #[must_use]
    pub fn category(&self) -> Category {
        match self {
            AnomalyKind::ProtectorInventory { .. } => Category::Provenance,
            AnomalyKind::EncryptionState { .. } => Category::Structure,
            AnomalyKind::WeakKdfIterations { .. } => Category::Integrity,
        }
    }

    /// Human-readable, "consistent with" note including the offending values.
    #[must_use]
    pub fn note(&self) -> String {
        match self {
            AnomalyKind::ProtectorInventory { summary, count } => {
                format!("{count} protector(s) present in the encryption context: {summary}")
            }
            AnomalyKind::EncryptionState { status } => format!(
                "logical volume conversion status is \"{status}\" \
                 (Complete = fully encrypted; Converting/Pending = in progress)"
            ),
            AnomalyKind::WeakKdfIterations { iterations } => format!(
                "PBKDF2 iteration count is {iterations}, below the {WEAK_KDF_THRESHOLD} \
                 threshold; consistent with weakened password key-stretching"
            ),
        }
    }

    /// MITRE ATT&CK technique ids this kind is consistent with.
    #[must_use]
    pub fn mitre(&self) -> &'static [&'static str] {
        &[]
    }

    // Symmetry with the fleet auditor shape; no current kind carries a subject
    // (the volume itself is named by the finding `Source::scope`).
    #[allow(clippy::unused_self)]
    fn subjects(&self) -> Vec<SubjectRef> {
        Vec::new()
    }

    fn evidence(&self) -> Vec<Evidence> {
        match self {
            AnomalyKind::ProtectorInventory { summary, count } => vec![
                evidence("protectors", summary.clone()),
                evidence("count", count.to_string()),
            ],
            AnomalyKind::EncryptionState { status } => {
                vec![evidence("conversion_status", status.clone())]
            }
            AnomalyKind::WeakKdfIterations { iterations } => {
                vec![evidence("pbkdf2_iterations", iterations.to_string())]
            }
        }
    }
}

fn evidence(field: &str, value: String) -> Evidence {
    Evidence {
        field: field.to_string(),
        value,
        location: None,
    }
}

/// A CoreStorage / FileVault anomaly: an observation graded by severity, with a
/// stable code and note derived from its [`AnomalyKind`] so they cannot drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anomaly {
    /// Severity, derived from `kind`.
    pub severity: Severity,
    /// Stable machine-readable code, derived from `kind`.
    pub code: &'static str,
    /// The classified anomaly.
    pub kind: AnomalyKind,
    /// Human-readable note, derived from `kind`.
    pub note: String,
}

impl Anomaly {
    /// Build an [`Anomaly`], deriving severity/code/note from `kind`.
    #[must_use]
    pub fn new(kind: AnomalyKind) -> Self {
        Anomaly {
            severity: kind.severity(),
            code: kind.code(),
            note: kind.note(),
            kind,
        }
    }
}

impl Observation for Anomaly {
    fn severity(&self) -> Option<Severity> {
        Some(self.severity)
    }
    fn code(&self) -> &'static str {
        self.code
    }
    fn note(&self) -> String {
        self.note.clone()
    }
    fn category(&self) -> Category {
        self.kind.category()
    }
    fn subjects(&self) -> Vec<SubjectRef> {
        self.kind.subjects()
    }
    fn evidence(&self) -> Vec<Evidence> {
        self.kind.evidence()
    }
    fn mitre(&self) -> &'static [&'static str] {
        self.kind.mitre()
    }
}

/// Classify the parsed [`FileVaultInfo`] into anomalies (no password needed).
#[must_use]
pub fn audit_info(info: &FileVaultInfo) -> Vec<Anomaly> {
    let mut out = Vec::new();

    let summary = protector_summary(info);
    out.push(Anomaly::new(AnomalyKind::ProtectorInventory {
        summary,
        count: info.protectors.len(),
    }));

    if let Some(status) = &info.conversion_status {
        out.push(Anomaly::new(AnomalyKind::EncryptionState {
            status: status.clone(),
        }));
    }

    if info.pbkdf2_iterations > 0 && info.pbkdf2_iterations < WEAK_KDF_THRESHOLD {
        out.push(Anomaly::new(AnomalyKind::WeakKdfIterations {
            iterations: info.pbkdf2_iterations,
        }));
    }

    out
}

/// A stable, human-readable summary of the protector kinds present.
fn protector_summary(info: &FileVaultInfo) -> String {
    if info.protectors.is_empty() {
        return "none".to_string();
    }
    let mut labels: Vec<&str> = info.protectors.iter().map(|p| p.kind.label()).collect();
    labels.sort_unstable();
    labels.join(", ")
}

/// Parse a reader's metadata (no password) and audit it.
///
/// # Errors
/// Any [`FileVaultError`] from header/metadata parsing.
pub fn audit<R: Read + Seek>(reader: R) -> Result<Vec<Anomaly>, FileVaultError> {
    let info = filevault::parse_info(reader)?;
    Ok(audit_info(&info))
}

/// Parse a file's metadata (no password) and audit it.
///
/// # Errors
/// [`FileVaultError::Io`] on open/read failure, or any parse error.
pub fn audit_path(path: &Path) -> Result<Vec<Anomaly>, FileVaultError> {
    let file = std::fs::File::open(path)?;
    audit(file)
}

/// Audit a reader and map each anomaly to a canonical [`Finding`], tagged with
/// the producing [`Source`] (`scope` names the evidence, e.g. the image path).
///
/// # Errors
/// Any [`FileVaultError`] from parsing.
pub fn audit_findings<R: Read + Seek>(
    reader: R,
    scope: impl Into<String>,
) -> Result<Vec<Finding>, FileVaultError> {
    let source = Source {
        analyzer: ANALYZER.to_string(),
        scope: scope.into(),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
    };
    Ok(audit(reader)?
        .into_iter()
        .map(|anomaly| anomaly.to_finding(source.clone()))
        .collect())
}
