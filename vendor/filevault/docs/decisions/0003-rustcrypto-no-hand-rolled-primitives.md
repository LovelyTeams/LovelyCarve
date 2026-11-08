# 3. RustCrypto for every cryptographic primitive — never hand-rolled

Date: 2026-07-24
Status: Accepted

## Context

Unlocking a CoreStorage / FileVault volume is a chain of standard primitives:
PBKDF2-HMAC-SHA256 (password → passphrase key), RFC 3394 AES key-unwrap
(passphrase key → KEK → volume master key), SHA-256 (VMK ‖ familyUUID → tweak
key), and AES-XTS-128 (metadata and logical-volume sector decryption). Every one
of these is a solved, audited primitive. The fleet constitution's single
hard-inverting rule — "Never hand-roll a cryptographic primitive … use a mature,
audited crate" (`~/.claude/CLAUDE.core.md` → Robustness) — applies directly: a
hand-derived key schedule or XTS tweak is wrong, unaudited, and in a forensic
decryptor would silently fabricate plaintext (the worst failure class).

## Decision

Depend only on RustCrypto crates for all cryptography; hand-roll nothing
(`core/Cargo.toml`):

- `pbkdf2` (`default-features = false`) + `hmac` + `sha2` — KEK derivation.
- `aes-kw` (RFC 3394 key unwrap, `alloc` feature).
- `aes` + `xts-mode` — AES-XTS-128 sector/metadata decryption
  (`core/src/xts.rs` wraps `xts_mode::Xts128` with `get_tweak_default`).

The XTS tweak encoding is taken from `xts-mode`'s `get_tweak_default`
(little-endian 128-bit unit index), which the RESEARCH notes verify matches
CoreStorage exactly rather than re-implementing (`core/src/xts.rs` doc comment).

## Consequences

- Correctness of the primitives is inherited from audited upstream crates; the
  crate's own logic is limited to *locating* the right bytes and *sequencing* the
  primitives, which is what the oracle validation (ADR 0006) checks.
- Wrong or absent passwords are rejected structurally: the RFC 3394 unwrap fails
  its integrity check, so decryption never proceeds to emit wrong plaintext
  (`docs/validation.md` → "Password enforcement").
- No `unsafe`, no C-FFI crypto dependency — consistent with `forbid(unsafe)`
  (ADR 0007).
