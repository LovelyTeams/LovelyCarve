# 5. Password-independent forensic audit; findings are observations via `forensicnomicon::report`

Date: 2026-07-24
Status: Accepted

## Context

A forensic examiner frequently holds a locked FileVault volume with no password.
The interesting forensic signal — which protectors (crypto users) exist, whether
the volume finished converting, how many PBKDF2 rounds the password protector
uses — all lives in the CoreStorage metadata, which is derivable *without*
unwrapping any key. Making the audit require the password would make it useless
in exactly the common case.

The fleet also standardizes analyzer output on one model
(`ronin-issen/CLAUDE.md` → "The Reporting Model — `forensicnomicon::report`") so
orchestration renders every analyzer's findings uniformly, and requires that
findings be **observations, never legal conclusions**.

## Decision

Split parsing so the password-independent metadata is its own product. The reader
exposes `parse_info` returning `FileVaultInfo` — protector inventory, PBKDF2
salt/iterations, conversion status, LV identity/size — and its doc comment states
"Producing it never unwraps any key" (`core/src/lib.rs`).

The analyzer consumes only that: `audit_path` / `audit` call `parse_info`, then
`audit_info` classifies `FileVaultInfo` into three graded `AnomalyKind`s
(`forensic/src/lib.rs`):

| code | severity | category |
|------|----------|----------|
| `FVDE-PROTECTOR-INVENTORY` | Info | Provenance |
| `FVDE-ENCRYPTION-STATE` | Info | Structure |
| `FVDE-WEAK-KDF-ITERATIONS` | Medium | Integrity |

Each `AnomalyKind` owns its severity/code/category/note/evidence and implements
`forensicnomicon::report::Observation`, so `audit_findings` maps every anomaly to
a canonical `Finding` tagged with the producing `Source`. Notes are phrased
"consistent with", carry the offending value verbatim (e.g. the observed
iteration count in the note and in `evidence`), and never assert a verdict — a
low iteration count is reported as *consistent with* weakened key-stretching, not
a determination of compromise (`forensic/src/lib.rs` docs and note strings).

The weak-KDF threshold is a fixed `WEAK_KDF_THRESHOLD = 20_000`
(`forensic/src/lib.rs`), documented as a defensible floor below modern FileVault
provisioning (fvdetest itself uses 90506 rounds).

## Consequences

- The analyzer runs on a locked volume with no password — its differentiator.
- Findings flow into the shared `forensicnomicon::report` aggregation unchanged,
  so Issen / a future GUI renders FileVault findings alongside every other
  analyzer.
- `code` values are a published contract (scheme-prefixed `FVDE-…`, SCREAMING-
  KEBAB); a shipped code must never change, new variants get new codes.
