# 7. Paranoid Gatekeeper — `forbid(unsafe)`, panic-free lints, fuzzed parsing

Date: 2026-07-24
Status: Accepted

## Context

Both crates parse untrusted, attacker-controllable CoreStorage / FileVault
volumes: every offset, length, and count is read from the image. The fleet
"Security & Robustness Standard — Paranoid Gatekeeper" (`ronin-issen/CLAUDE.md`)
is mandatory for every `*-core` / `*-forensic` crate — never panic, never read
out of bounds, never trust a length field. Unlike the mmap container readers
(`ewf`, `memory-forensic`), this crate needs no `unsafe` at all: it reads through
`Read + Seek`, not a memory map.

## Decision

Enforce the panic-free posture through the workspace lint table
(`Cargo.toml` `[workspace.lints]`), inherited by both members via
`[lints] workspace = true`:

- `unsafe_code = "forbid"` — the strongest form (no per-site override possible),
  earning the README's `unsafe forbidden` badge. Confirmed by the
  `#![forbid(unsafe_code)]` at the top of both `core/src/lib.rs` and
  `forensic/src/lib.rs`.
- `unwrap_used = "deny"` + `expect_used = "deny"`, plus `correctness`/`suspicious`
  denied and `all`/`pedantic` warned. Tests opt out via
  `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]` and
  `clippy.toml` (`allow-unwrap-in-tests`).
- Bounds-checked reads and loud failure: fixed-size header/metadata reads go
  through `read_exact_or_err`, which maps a short read to an explicit
  `FileVaultError::Io` rather than an empty result; a zero block size is rejected
  as `OutOfRange` before use (`core/src/lib.rs`). A truncated image is a
  bootstrap failure surfaced loudly, never a silent empty decode.
- **Fuzzing**: a `cargo fuzz` target over the metadata parse path
  (`core/fuzz/fuzz_targets/fuzz_metadata.rs`), invariant "must not panic",
  with a `fuzz.yml` CI workflow.

Supply-chain gates back this: `deny.toml` (permissive-license allowlist, yanked =
deny, unknown registry/git denied) plus `cargo-vet` (commits `510ce8f`,
`ba3ac52`) and a committed `Cargo.lock` (commit `1109a54`) to stabilize vet.

## Consequences

- A crafted volume that lies about a length or offset yields an error, never a
  panic or an out-of-bounds read — verified continuously by the fuzz target.
- The `unsafe forbidden` badge is honest (no downgrade to `deny` + allow, unlike
  the mmap readers) because no `unsafe` site exists.
- Test code keeps `unwrap`/`expect` to fail loudly; production code cannot.
