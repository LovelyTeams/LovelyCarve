# 1. Two-crate reader/analyzer split (`filevault-core` + `filevault-forensic`)

Date: 2026-07-24
Status: Accepted

## Context

The repo handles one on-disk format family — Apple CoreStorage / FileVault 2
(FVDE). Two audiences want different things from it: a decryptor that turns a
password + ciphertext into a plaintext `Read + Seek` stream, and a forensic
auditor that grades a *locked* volume's metadata (protectors, encryption state,
KDF strength) with no password at all. Bundling both into one crate would force
a consumer that only wants the decryptor to compile the `forensicnomicon`
reporting stack, and vice-versa.

The fleet constitution (`ronin-issen/CLAUDE.md` → "Crate-structure standard —
reader/analyzer split" and "Crate naming grammar → Pattern A") mandates exactly
this shape for a single-format repo: one workspace named `<x>-forensic`, a
`core/` crate (the reader) and a `forensic/` crate (the analyzer).

## Decision

Ship one Cargo workspace (`Cargo.toml` `members = ["core", "forensic"]`) with two
library crates:

- **`core/` → `filevault-core`** — parses the physical volume header, decrypts
  the CoreStorage metadata, derives the volume-key hierarchy, and exposes the
  decrypted logical volume (`core/src/lib.rs`: `FileVaultVolume`,
  `DecryptedVolume`, `parse_info`).
- **`forensic/` → `filevault-forensic`** — classifies the parsed metadata into
  graded anomalies and maps them to `forensicnomicon::report::Finding`
  (`forensic/src/lib.rs`: `AnomalyKind`, `audit_path`, `audit_findings`).

The analyzer depends on the reader (`forensic/Cargo.toml`:
`filevault = { workspace = true }`) — the default direction — because the
reader's password-independent `parse_info` / `FileVaultInfo` already exposes
everything the audit needs (protector inventory, conversion status, PBKDF2
parameters), so there is no reason to re-parse the raw structure at a lower
level. Shared package fields (`version`, `edition`, `rust-version`, `license`)
are inherited from `[workspace.package]` (DRY).

## Consequences

- A downstream Rust tool that only wants the decryptor depends on
  `filevault-core` alone; a triage tool that only wants graded findings depends
  on `filevault-forensic`, which pulls the reader transitively.
- Both crates carry a workspace-uniform MSRV floor of **1.81**
  (`[workspace.package] rust-version = "1.81"`). This is above the fleet's usual
  1.75/1.80 library floor; the specific dependency that forces 1.81 is not
  recovered from available history (Rationale reconstructed from structure;
  original intent not recovered in available history).
- The layering must stay acyclic. If a future audit needs to see raw structure
  the reader normalizes away (slack, malformed records), the analyzer is free to
  drop to lower-level parsing per the constitution's "`-forensic` is NOT required
  to depend on `-core`" principle — but no current finding needs that.
