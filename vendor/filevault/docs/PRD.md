# filevault-forensic — Design (Purpose & Scope)

*A library, not a product: `filevault-forensic` ships no examiner-run binary. It
provides two crates that other Rust tools and the Issen fleet link. This
document states what those crates are for, where they sit in the fleet, and how
their design decisions are grounded; the load-bearing decisions live as ADRs
under [`docs/decisions/`](decisions/). Every current-state claim is grounded in a
read of `core/src/` and `forensic/src/` (2026-07-24).*

## Purpose

Decrypt Apple CoreStorage / FileVault 2 volumes in pure Rust — password to
plaintext HFS+ — and audit a *locked* volume's encryption metadata without the
password. No libfvde, no C, RustCrypto only.

Two audiences:

- A **Rust developer / forensic tool** that needs a decryptor: point
  `filevault-core` at an encrypted CoreStorage physical volume plus a password,
  get a `Read + Seek` view of the decrypted logical volume.
- A **forensic examiner** holding a locked volume with no password: run
  `filevault-forensic::audit_path`, get severity-graded observations about the
  volume's protectors, encryption state, and KDF strength.

## What it does

| crate | role | key surface |
|-------|------|-------------|
| **`filevault-core`** (`use filevault`) | reader / decryptor | `FileVaultVolume::unlock_with_password`, `parse_info` → `FileVaultInfo`, `DecryptedVolume`; optional `vfs` feature → `FileVaultLayer` (ADR 0004) |
| **`filevault-forensic`** | analyzer | `audit_path` / `audit` / `audit_findings` → graded `AnomalyKind` / `forensicnomicon::report::Finding` |

The decrypt chain (all RustCrypto — ADR 0003): parse the 512-byte physical volume
header → AES-XTS-128-decrypt the CoreStorage metadata → derive the key hierarchy
from the password (PBKDF2-HMAC-SHA256 → RFC 3394 AES-KW → SHA-256 tweak key) →
AES-XTS-128-decrypt logical-volume sectors. Offsets and algorithms are
cross-checked against libfvde in [`RESEARCH.md`](RESEARCH.md).

The audit needs no password: `parse_info` derives protector inventory, PBKDF2
salt/iterations, and conversion status from metadata without unwrapping any key
(ADR 0005). Three findings — `FVDE-PROTECTOR-INVENTORY`, `FVDE-ENCRYPTION-STATE`,
`FVDE-WEAK-KDF-ITERATIONS` — are emitted as observations, never verdicts.

## Where it sits in the fleet

`filevault-core` is a decryptor that plugs into the fleet's universal
container/filesystem abstraction as an **encryption layer**: behind the `vfs`
feature it implements `forensic-vfs`'s `EncryptionLayer` (ADR 0004), so a stack
such as `E01 → GPT → FileVault → HFS+` reads as one `ImageSource`.
`filevault-forensic` is a PARSER-layer analyzer emitting the shared
`forensicnomicon::report` model, so Issen aggregates its findings alongside every
other analyzer.

## Scope

- CoreStorage / FileVault 2 (macOS 10.7 Lion – 10.15 Catalina), AES-XTS-128,
  password protector.
- Recovery-password / institutional-key protectors unlock through the same code
  path but are untested against an oracle (documented unvalidated —
  [`DEFERRED.md`](DEFERRED.md)).

## Non-goals

- **APFS-native (software) encryption** (10.13+) — a different on-disk format
  with no reference decryptor or oracle; deferred to a future `apfs-forensic`
  effort (ADR 0008, [`DEFERRED.md`](DEFERRED.md)). Not a special case bolted on
  here.
- **A CLI / GUI / MCP front end** — this repo is library-only; the examiner-facing
  surface is Issen / `disk4n6`.
- **Hand-rolled cryptography** — RustCrypto only (ADR 0003).

## Validation approach

Correctness of decryption is proven against an **independent third-party oracle
on a real encrypted volume** — pyfvde (libyal libfvde) on dfvfs's
`fvdetest.qcow2` — not fixtures we authored (ADR 0006,
[`validation.md`](validation.md)). Decrypted sectors are byte-identical to
pyfvde's; offset 1024's decrypted bytes carry a valid HFS+ volume header, a
manual structural observation ([`RESEARCH.md`](RESEARCH.md)) corroborating the
sector digests. Parsing is `forbid(unsafe)`, panic-free by lint, and fuzzed
(ADR 0007).

## Decisions

See [`docs/decisions/`](decisions/):

1. [Two-crate reader/analyzer split](decisions/0001-two-crate-reader-analyzer-split.md)
2. [Package `filevault-core`, import alias `filevault`](decisions/0002-crate-naming-lib-alias.md)
3. [RustCrypto for every primitive](decisions/0003-rustcrypto-no-hand-rolled-primitives.md)
4. [`forensic-vfs` EncryptionLayer adapter behind `vfs`](decisions/0004-vfs-encryptionlayer-adapter.md)
5. [Password-independent audit; observations](decisions/0005-password-independent-audit-observations.md)
6. [Oracle-first validation](decisions/0006-oracle-first-validation.md)
7. [Paranoid Gatekeeper — panic-free](decisions/0007-paranoid-gatekeeper-panic-free.md)
8. [Scope bounded; APFS deferred](decisions/0008-scope-corestorage-apfs-deferred.md)
