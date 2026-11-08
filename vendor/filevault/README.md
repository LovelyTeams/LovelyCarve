> LovelyCarve vendored patches and current support boundaries: [LOVELYCARVE.md](LOVELYCARVE.md). The remaining README is retained from the pinned upstream checkout.

# filevault-forensic

[![Crates.io (core)](https://img.shields.io/crates/v/filevault-core?label=filevault-core)](https://crates.io/crates/filevault-core)
[![Crates.io (forensic)](https://img.shields.io/crates/v/filevault-forensic?label=filevault-forensic)](https://crates.io/crates/filevault-forensic)
[![Docs.rs](https://img.shields.io/docsrs/filevault-core?label=docs.rs)](https://docs.rs/filevault-core)
[![Rust 1.81+](https://img.shields.io/badge/rust-1.81%2B-orange)](https://www.rust-lang.org)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa)](https://github.com/sponsors/h4x0r)

[![CI](https://github.com/SecurityRonin/filevault-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/filevault-forensic/actions/workflows/ci.yml)
[![Coverage](https://img.shields.io/badge/coverage-100%25%20lines-brightgreen)](https://github.com/SecurityRonin/filevault-forensic/actions/workflows/ci.yml)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success)](https://github.com/rust-secure-code/safety-dance)
[![Security audit](https://img.shields.io/badge/security-cargo--deny-informational)](deny.toml)

**Decrypt Apple CoreStorage / FileVault 2 volumes in pure Rust — password to plaintext HFS+, no libfvde, no C, one static dependency-light crate.**

Point it at an encrypted CoreStorage physical volume and a password; get a
`Read + Seek` view of the decrypted logical volume plus a severity-graded
forensic audit of its protectors and encryption state.

```rust
use std::fs::File;
use filevault::FileVaultVolume;

let img = File::open("corestorage.raw")?;
let mut vol = FileVaultVolume::unlock_with_password(img, "s3cret")?;

let mut boot = [0u8; 512];
vol.read_at(1024, &mut boot)?;      // decrypted HFS+ volume header ("H+")
# Ok::<(), filevault::FileVaultError>(())
```

Validated end-to-end against **pyfvde** (libyal libfvde) on a real encrypted
volume: the decrypted sectors are byte-identical (see
[validation](docs/validation.md)).

## Two crates

| crate | role |
|-------|------|
| **`filevault-core`** (`use filevault`) | the reader/decryptor — parse the CoreStorage volume header + metadata, derive the volume key (PBKDF2-SHA256 → RFC 3394 key-unwrap), AES-XTS-decrypt logical-volume sectors |
| **`filevault-forensic`** | the analyzer — graded [`forensicnomicon::report::Finding`]s over the parsed metadata, **no password required** |

### Audit without the password

```rust
let findings = filevault_forensic::audit_path(std::path::Path::new("corestorage.raw"))?;
for f in &findings {
    println!("{}: {}", f.code, f.note);
}
# Ok::<(), filevault::FileVaultError>(())
```

| code | severity | observation |
|------|----------|-------------|
| `FVDE-PROTECTOR-INVENTORY` | Info | which protectors are present (password / recovery / institutional-key crypto users) |
| `FVDE-ENCRYPTION-STATE` | Info | conversion status — encrypted (Complete) / Converting / Pending |
| `FVDE-WEAK-KDF-ITERATIONS` | Medium | PBKDF2 iteration count below a defensible floor (with the observed count) |

Findings are **observations, never verdicts** — they state what the metadata
shows, not a conclusion.

## Crypto

RustCrypto only — never a hand-rolled primitive: `pbkdf2` + `hmac` + `sha2`
(KEK derivation), `aes-kw` (RFC 3394 key unwrap), `aes` + `xts-mode`
(AES-XTS-128 sector decryption).

## Trust, but verify

Panic-free by construction (`unsafe` forbidden; `unwrap`/`expect`/unchecked
indexing denied in non-test code; every offset and length from the image is
bounds-checked before use), fuzzed (`cargo fuzz` metadata target, must-not-panic),
and validated against an **independent oracle on real data** — not fixtures we
authored. See [docs/validation.md](docs/validation.md).

## Scope

CoreStorage / FileVault 2 (macOS 10.7 Lion – 10.15 Catalina), AES-XTS-128,
password protector. **APFS-native encryption (10.13+) is a separate format and
is deferred** — see [docs/DEFERRED.md](docs/DEFERRED.md).

[Privacy Policy](https://securityronin.github.io/filevault-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/filevault-forensic/terms/) · © 2026 Security Ronin Ltd
