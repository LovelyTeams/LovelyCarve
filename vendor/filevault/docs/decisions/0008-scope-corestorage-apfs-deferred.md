# 8. Scope bounded to CoreStorage / FileVault 2; APFS-native encryption deferred

Date: 2026-07-24
Status: Accepted

## Context

"FileVault" names two distinct on-disk formats:

1. **CoreStorage / FileVault 2** (macOS 10.7 Lion – 10.15 Catalina) — AES-XTS-128
   over CoreStorage metadata. It has an authoritative reference (libyal
   `libfvde`) and a working oracle (`pyfvde`).
2. **APFS-native (software) encryption** (10.13+, the default once the boot
   volume is APFS) — a *different* format: the key hierarchy and encrypted
   extents live inside the APFS container (keybag, per-volume VEK/KEK,
   cryptexts), not in CoreStorage metadata. `libfvde` does not cover it, there is
   no settled open-source reference decryptor, no Rust crate, and no ready oracle
   with a known password (`docs/DEFERRED.md`, `docs/RESEARCH.md`).

The oracle-first discipline (ADR 0006) forbids writing a decryptor with no
independent oracle: coding APFS-native crypto from memory would be the exact
"decrypt-to-wrong-plaintext-silently" failure that discipline exists to prevent.

## Decision

Scope this crate to **CoreStorage / FileVault 2, AES-XTS-128, password
protector** (README "Scope", `core/src/lib.rs` module docs). Unsupported inputs
fail loud: a non-CoreStorage image returns `FileVaultError::NotCoreStorage`, an
unsupported encryption method is rejected rather than mis-decrypted.

**Defer APFS-native encryption** explicitly (`docs/DEFERRED.md`). It is a separate
future phase belonging with an APFS container reader (`apfs-forensic`) and needs
its own authoritative spec + independent oracle before any crypto is written —
not a special case bolted onto this crate.

## Consequences

- The crate does one format correctly and provably, rather than two formats one
  of which cannot be validated.
- The scope boundary is stated wherever a reader might assume broader coverage
  (README, lib docs, `DEFERRED.md`), so "FileVault" is never over-claimed.
- Recovery-password and institutional-key protectors are in-scope by construction
  (same unlock path) but untested against an oracle; documented as unvalidated in
  `docs/DEFERRED.md` rather than claimed.
- When APFS-native encryption is undertaken, this ADR records why it was a
  separate effort: a different format with a different key hierarchy and no
  available oracle at the time.
