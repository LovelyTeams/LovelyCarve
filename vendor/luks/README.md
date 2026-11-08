# luks-forensic

> LovelyCarve's vendored build extends the upstream cipher/credential scope.
> See [local integration notes](LOVELYCARVE.md) and [LUKS capability comparison](../../docs/formats/LUKS.md).
> Upstream CI/coverage badges below describe the original upstream project.

[![Crates.io: luks-core](https://img.shields.io/crates/v/luks-core.svg?label=luks-core)](https://crates.io/crates/luks-core)
[![Crates.io: luks-forensic](https://img.shields.io/crates/v/luks-forensic.svg?label=luks-forensic)](https://crates.io/crates/luks-forensic)
[![Docs.rs](https://img.shields.io/docsrs/luks-core?label=docs.rs)](https://docs.rs/luks-core)
[![Rust 1.81+](https://img.shields.io/badge/rust-1.81%2B-blue.svg)](https://www.rust-lang.org)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=githubsponsors)](https://github.com/sponsors/h4x0r)

[![CI](https://github.com/SecurityRonin/luks-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/luks-forensic/actions/workflows/ci.yml)
[![Coverage](https://img.shields.io/badge/coverage-100%25%20lines-brightgreen.svg)](docs/validation.md)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance)
[![Security advisories](https://img.shields.io/badge/advisories-clean-success.svg)](https://rustsec.org)

**Unlock a LUKS container from its passphrase and read the plaintext — a
from-scratch, pure-Rust LUKS1/LUKS2 decryptor, validated sector-for-sector
against `cryptsetup` on real containers.**

No `cryptsetup` C dependency, no `dm-crypt`, no mounting, no root: one library
that parses the on-disk header, derives the master key from a passphrase through
PBKDF2 or Argon2, and decrypts sectors with AES-XTS.

```rust,ignore
use std::fs::File;
use luks::LuksVolume;

// Auto-detects LUKS1 vs LUKS2 from the header.
let mut vol = LuksVolume::unlock_with_passphrase(File::open("container.luks")?, b"luks-TEST")?;

let mut first = [0u8; 512];
vol.read_at(0, &mut first)?;     // decrypted payload sector 0
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Scope

This build parses **LUKS1** partition headers and **LUKS2** binary-header + JSON
metadata, and unlocks both from a passphrase over the `aes-xts-plain64` cipher
(AES-128/256-XTS) — the `cryptsetup` default. LUKS1 keyslots derive with
**PBKDF2**; LUKS2 keyslots derive with **PBKDF2 or Argon2i/Argon2id**. Both format
versions are validated against a `cryptsetup` 2.7.0 oracle:

| Version | Cipher | KDF | Oracle (tier) |
|---|---|---|---|
| LUKS1 | `aes-xts-plain64` (AES-256-XTS) | PBKDF2-sha256 | self-minted `luks1.img` vs `cryptsetup` (Tier-2) |
| LUKS2 | `aes-xts-plain64`, 4096-B sectors | Argon2id | self-minted `luks2.img` vs `cryptsetup` (Tier-2) |

An unsupported cipher/mode/hash is **recognized and refused with a named error**
(the offending value verbatim) — never decrypted by construction. See
[`docs/RESEARCH.md`](docs/RESEARCH.md).

## The two-crate split

Following the fleet reader/analyzer standard:

| Crate | Role | Emits |
|---|---|---|
| **`luks-core`** | reader / decryptor (`pbkdf2` · `argon2` · `aes` · `xts-mode` · `hmac` · `sha2`) | plaintext `Read + Seek` view + typed header metadata |
| **`luks-forensic`** | anomaly analyzer over the header | severity-graded findings |

### Analyzer findings

| Code | Severity | Meaning |
|---|---|---|
| `LUKS-WEAK-CIPHER-MODE` | Low | cipher mode is `cbc`/`ecb` (weaker than `xts-plain64`) |
| `LUKS-WEAK-KDF-HASH` | Low | the KDF/AF hash is `sha1` |
| `LUKS-LOW-KDF-ITERATIONS` | Medium | an active keyslot has < 1000 PBKDF2 iterations (brute-force risk) |
| `LUKS-KEYSLOT-INVENTORY` | Info | count of active keyslots (of 8) |

Findings are **observations, never verdicts** — the examiner draws conclusions.

## Trust but verify

- **Every cryptographic primitive is an audited RustCrypto crate** (`pbkdf2`,
  `argon2`, `aes`, `xts-mode`, `hmac`, `sha1`, `sha2`) — nothing hand-rolled. The
  only bespoke routine is the LUKS **anti-forensic merge** (the TKS1 splitter),
  validated **only** against the independent `cryptsetup` oracle on real
  containers, never a self-authored round-trip.
- **Panic-free, bounds-checked** parsing of untrusted containers;
  `unwrap`/`expect` denied in production code (`#![forbid(unsafe_code)]`); the
  LUKS1/LUKS2 header parsers, the AF-merge, and the full unlock pipeline are
  fuzzed.
- **Tier-2 validated**: decrypted sectors match `cryptsetup` byte-for-byte across
  LUKS1 and LUKS2 — see [`docs/validation.md`](docs/validation.md).

[Privacy Policy](https://securityronin.github.io/luks-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/luks-forensic/terms/) · © 2026 Security Ronin Ltd
