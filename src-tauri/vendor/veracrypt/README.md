# veracrypt-core

Pure-Rust reader for VeraCrypt and TrueCrypt volumes: unlock from a password,
PIM and keyfiles, then read the plaintext at any offset. No VeraCrypt binary,
driver or mount is involved, and `unsafe` is forbidden.

```rust,ignore
use veracrypt::{unlock, Control, FileSource, HeaderSelection, UnlockRequest};

let volume = unlock(
    FileSource::open("container.hc")?,
    &UnlockRequest { password: b"passphrase", pim: 0, prf: None, headers: HeaderSelection::Auto },
    &Control { cancelled: &|| false, progress: None, threads: None },
)?;
let mut sector = [0u8; 512];
volume.read_at(0, &mut sector)?;
println!("{} {} / {}", volume.info().flavor.name(), volume.info().chain.name(), volume.info().kdf.prf.name());
```

## Scope

| Axis | Supported |
| --- | --- |
| Key derivation | PBKDF2 over SHA-512, SHA-256, BLAKE2s-256, Whirlpool, Streebog-512, RIPEMD-160; Argon2id (VeraCrypt 1.26.29) |
| Iteration counts | VeraCrypt (PIM 0 or `15000 + 1000·PIM`, pre-boot `2048·PIM`), TrueCrypt 5–7 (1000 / 2000) |
| Ciphers | AES, Serpent, Twofish, Camellia, Kuznyechik and all ten VeraCrypt cascades, XTS with 512-byte data units |
| Credentials | password (UTF-8 bytes, up to 128), PIM, keyfiles (`KeyfilePool`, 64/128-byte pool) |
| Headers | normal, hidden, both embedded backups, TrueCrypt 5 hidden, system encryption (drive sector 62) |
| Layouts | 512–4096-byte sectors, TrueCrypt < 6.0 layout, finished in-place encryption, system partition or whole drive, paused system encryption |

`unlock` derives every candidate header key on all cores (one job per PBKDF2
block, the 64 bytes a single cipher needs first), stops on request every 1024
iterations and reports progress. `Volume::read_at` takes `&self`, so a volume
can be read from several threads; large spans decrypt in parallel. Every
primitive is a RustCrypto crate; XTS and the cascade layout are implemented
here and checked against VeraCrypt's own self-test vectors.

## Tests

- Unit tests: VeraCrypt `Tests.c` PBKDF2 vectors and the `TestSectorBufEncryption`
  CRC-32s for all 15 algorithms, IEEE 1619 vectors, `xts-mode` and `pbkdf2`
  cross-checks, and whole-volume round trips built with those crates.
- `tests/compatibility.rs`: the independent corpus in `tests/data`, generated
  by `scripts/make-veracrypt-compatibility-fixtures.py` at the repository root.
- `tests/cryptsetup_corpus.rs`: cryptsetup's real TrueCrypt / VeraCrypt test
  images (`tests/tcrypt-images.tar.xz` of the cryptsetup repository, not
  redistributed), extracted and named by `VC_TCRYPT_IMAGES`; skipped when unset.
- `tests/oracle_veracrypt.rs`: real VeraCrypt volumes named by `VC_ORACLE`,
  `VC_HIDDEN_ORACLE`, `VC_SERPENT_ORACLE`, `VC_CASCADE_ORACLE` and
  `VC_CASCADE3_ORACLE`; skipped when unset.

Derived from [veracrypt-forensic](https://github.com/SecurityRonin/veracrypt-forensic)
(Apache-2.0); see `LOVELYCARVE.md` for what changed.
