# bitlocker-forensic

This is the upstream README snapshot. LovelyCarve local capabilities and
validation boundaries supersede its scope claims: see [LOVELYCARVE.md](LOVELYCARVE.md).

[![Crates.io: bitlocker-core](https://img.shields.io/crates/v/bitlocker-core.svg?label=bitlocker-core)](https://crates.io/crates/bitlocker-core)
[![Crates.io: bitlocker-forensic](https://img.shields.io/crates/v/bitlocker-forensic.svg?label=bitlocker-forensic)](https://crates.io/crates/bitlocker-forensic)
[![Docs.rs](https://img.shields.io/docsrs/bitlocker-core?label=docs.rs)](https://docs.rs/bitlocker-core)
[![Rust 1.81+](https://img.shields.io/badge/rust-1.81%2B-blue.svg)](https://www.rust-lang.org)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=githubsponsors)](https://github.com/sponsors/h4x0r)

[![CI](https://github.com/SecurityRonin/bitlocker-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/bitlocker-forensic/actions/workflows/ci.yml)
[![Coverage](https://img.shields.io/badge/coverage-100%25%20lines-brightgreen.svg)](docs/validation.md)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance)
[![Security advisories](https://img.shields.io/badge/advisories-clean-success.svg)](https://rustsec.org)

**Unlock a BitLocker volume from its password and read the plaintext — a
from-scratch, pure-Rust BitLocker (BDE) decryptor, validated byte-for-byte
against `pybde` on real disk images.**

No `dislocker` C dependency, no FUSE, no mounting: one library that parses the
FVE metadata, derives the keys from a password or recovery password, and
decrypts sectors — AES-CBC (± Elephant Diffuser) and AES-XTS, 128- and 256-bit.

```rust,ignore
use std::fs::File;
use bitlocker::BitLockerVolume;

// Unlock the dfvfs BitLocker To Go test image with its published password.
let mut vol = BitLockerVolume::unlock_with_password(File::open("bdetogo.raw")?, "bde-TEST")?;

let mut boot = [0u8; 512];
vol.read_at(0, &mut boot)?;      // decrypted FAT boot sector
assert_eq!(&boot[3..11], b"MSWIN4.1");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Scope

This build unlocks **four of the five** BitLocker unlock protectors — **password**
(`0x2000`), **recovery-password** (`0x0800`), **clear-key** (`0x0000`, no
credential — a suspended volume), and **startup-key** (`0x0200`, a `.BEK` external
key) — and decrypts **five of the six** BitLocker ciphers, each validated against a
`pybde` oracle:

| Method | Cipher | Oracle (tier) |
|---|---|---|
| `0x8000` | AES-128-CBC + Elephant Diffuser | dfvfs `bdetogo.raw` (Tier-1) |
| `0x8002` | AES-128-CBC | picoCTF 2025 `bitlocker-1.dd` (Tier-1) |
| `0x8003` | AES-256-CBC | self-minted `m8003` (Tier-2) |
| `0x8004` | XTS-AES-128 | BelkaCTF6 `vault` (Tier-1) + `m8004` (Tier-2) |
| `0x8005` | XTS-AES-256 | self-minted `m8005` (Tier-2) |

`BitLockerVolume::unlock_clear_key(reader)` unlocks a **clear-key** volume with no
credential (Tier-2, self-minted `clearkey` vs `pybde`), and
`BitLockerVolume::unlock_with_startup_key(reader, bek_bytes)` unlocks a
**startup-key** volume from its `.BEK` external-key file (Tier-2, self-minted
`sk8004` vs `pybde` `read_startup_key`, cross-checked against the recovery
password). The dispatch decodes all six ciphers (`0x8000`–`0x8005`) into their axes
and ships a decrypt for a cipher only once a real oracle validates it. The remaining
method, AES-256-CBC + Elephant Diffuser (`0x8001`), is **recognized and refused with
a named error** — never decrypted by construction — so it lights up as a one-line
change plus a test the moment it gets an oracle. The TPM protector is out of scope
for *unlock*, but the metadata parser still **reports** every protector and cipher
it finds. See [`docs/RESEARCH.md`](docs/RESEARCH.md).

## The two-crate split

Following the fleet reader/analyzer standard:

| Crate | Role | Emits |
|---|---|---|
| **`bitlocker-core`** | reader / decryptor (`aes` · `cbc` · `ccm` · `xts-mode` · `sha2`) | plaintext `Read + Seek` view + typed FVE metadata |
| **`bitlocker-forensic`** | anomaly analyzer over the metadata | graded `forensicnomicon::report` `Finding`s |

### Analyzer findings

| Code | Severity | Meaning |
|---|---|---|
| `BDE-CLEAR-KEY-PRESENT` | High | a clear-key protector (`0x0000`) is present ⇒ the volume is effectively unencrypted |
| `BDE-PROTECTOR-INVENTORY` | Info | one per protector (password / recovery / TPM / startup key / …) |
| `BDE-WEAK-CIPHER` | Low | AES-CBC ± diffuser is weaker than AES-XTS — consistent with an older OS |
| `BDE-TO-GO` | Info | a BitLocker To Go removable-media volume |

Findings are **observations, never verdicts** — the examiner draws conclusions.

## Trust but verify

- **Every primitive is an audited RustCrypto crate** (`aes`, `cbc`, `ccm`,
  `sha2`). The only hand-written cryptographic routine is the **Elephant
  Diffuser** — no crate exists for it — implemented to the `libbde` reference and
  validated **only** against the independent `pybde` oracle on the real
  `bdetogo.raw` image, never a self-authored round-trip.
- **Panic-free, bounds-checked** parsing of untrusted volumes; `unwrap`/`expect`
  denied in production code (`#![forbid(unsafe_code)]`); the metadata parser is
  fuzzed.
- **Tier-1 validated**: decrypted sectors match `pybde` byte-for-byte — see
  [`docs/validation.md`](docs/validation.md).

[Privacy Policy](https://securityronin.github.io/bitlocker-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/bitlocker-forensic/terms/) · © 2026 Security Ronin Ltd
