# veracrypt-forensic

[![Crates.io: veracrypt-core](https://img.shields.io/crates/v/veracrypt-core.svg?label=veracrypt-core)](https://crates.io/crates/veracrypt-core)
[![Crates.io: veracrypt-forensic](https://img.shields.io/crates/v/veracrypt-forensic.svg?label=veracrypt-forensic)](https://crates.io/crates/veracrypt-forensic)
[![Docs.rs](https://img.shields.io/docsrs/veracrypt-core?label=docs.rs)](https://docs.rs/veracrypt-core)
[![Rust 1.81+](https://img.shields.io/badge/rust-1.81%2B-blue.svg)](https://www.rust-lang.org)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=githubsponsors)](https://github.com/sponsors/h4x0r)

[![CI](https://github.com/SecurityRonin/veracrypt-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/veracrypt-forensic/actions/workflows/ci.yml)
[![Coverage](https://img.shields.io/badge/coverage-100%25%20lines-brightgreen.svg)](docs/validation.md)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance)
[![Security advisories](https://img.shields.io/badge/advisories-clean-success.svg)](https://rustsec.org)

**Unlock a VeraCrypt (or legacy TrueCrypt) volume from its password and read the
plaintext — a from-scratch, pure-Rust decryptor validated byte-for-byte against
both the real VeraCrypt binary and `cryptsetup` on a real volume.**

No `veracrypt` binary, no `cryptsetup`, no FUSE, no mounting: one library that
brutes the header PRF and cipher from a password, recovers the master key, and
decrypts the data area as AES-256 or Twofish-256 XTS.

```rust,ignore
use std::fs::File;
use veracrypt::VeraVolume;

// Unlock a VeraCrypt volume with its password (no PIM).
let mut vol = VeraVolume::unlock_with_password(File::open("container.vc")?, b"passphrase")?;

let mut first = [0u8; 512];
vol.read_at(0, &mut first)?;     // first decrypted sector of the data area
println!("{:?} / {}", vol.info().flavor, vol.info().cipher.name());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Scope

This build brutes the header across **all five VeraCrypt PRFs** and decrypts
**both single ciphers**, with support for a PIM and for normal *and* hidden
volumes:

| Axis | Supported |
|---|---|
| PRF (header key derivation) | SHA-512 · SHA-256 · Whirlpool · Streebog-512 · RIPEMD-160 |
| Cipher (data area) | AES-256-XTS · Serpent-256-XTS · Twofish-256-XTS · all VeraCrypt cascades |
| PIM | yes — `unlock_with_pim` / `unlock_hidden_with_pim` |
| Volume layout | normal (header @ 0) · hidden (header @ 64 KiB) |
| Flavor | VeraCrypt (`VERA`) · legacy TrueCrypt (`TRUE`) |

`VeraVolume::unlock_with_password(reader, password)` tries every PRF × cipher
until one decrypts the header to a valid `VERA`/`TRUE` signature with both CRC-32s
matching, then exposes a plaintext `Read + Seek` view (`read_at`).
`unlock_hidden_with_password` reads the hidden header at 64 KiB — used to access,
or to prove the presence of, a deniable hidden volume.

All three single ciphers — **AES-256**, **Serpent-256**, **Twofish-256** — are
supported, each via an audited RustCrypto crate (Serpent through
`serpent::Serpent::new_from_slice`, which takes the full 256-bit key), and so are
**all VeraCrypt cipher cascades** (AES-Twofish, AES-Twofish-Serpent, Serpent-AES,
Serpent-Twofish-AES, Twofish-Serpent) — stacked XTS layers keyed exactly as
cryptsetup's `TCRYPT_decrypt_hdr`. `VolumeInfo::cipher_display()` names the chain
(e.g. `aes-twofish-serpent`). See [`docs/RESEARCH.md`](docs/RESEARCH.md).

## The two-crate split

Following the fleet reader/analyzer standard:

| Crate | Role | Emits |
|---|---|---|
| **`veracrypt-core`** | reader / decryptor (`aes` · `twofish` · `xts-mode` · `pbkdf2` · `sha2` · `whirlpool` · `streebog` · `ripemd`) | plaintext `Read + Seek` view + typed `VolumeInfo` |
| **`veracrypt-forensic`** | anomaly analyzer over the recovered facts | graded observations |

### Analyzer findings

| Code | Severity | Meaning |
|---|---|---|
| `VC-LEGACY-TRUECRYPT` | Low | the volume is a legacy TrueCrypt (not VeraCrypt) container |
| `VC-HIDDEN-VOLUME-DECLARED` | Medium | the outer header declares a hidden volume (deniable-encryption indicator) |
| `VC-CIPHER-INVENTORY` | Info | the recovered PRF, cipher, and data-area offset |

Findings are **observations, never verdicts** — the examiner draws conclusions.

## Trust but verify

- **Every primitive is an audited RustCrypto crate** — `aes`, `twofish`,
  `xts-mode`, `pbkdf2`, `hmac`, `sha2`, `whirlpool`, `streebog`, `ripemd`,
  `crc32fast`. No cryptography is hand-rolled.
- **Unimpeachable Tier-1**: on a real VeraCrypt volume with a published password,
  the decrypted sectors of **three independent implementations agree
  byte-for-byte** — VeraCrypt 1.26.20 (Idrix, the format's own reference),
  `cryptsetup` 2.7.0 (an independent reimplementation), and this crate. See
  [`docs/validation.md`](docs/validation.md).
- **Panic-free by lint, bounds-checked** parsing of untrusted volumes;
  `unwrap`/`expect` denied in production code (`#![forbid(unsafe_code)]`); the
  header parser is fuzzed.

[Privacy Policy](https://securityronin.github.io/veracrypt-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/veracrypt-forensic/terms/) · © 2026 Security Ronin Ltd
