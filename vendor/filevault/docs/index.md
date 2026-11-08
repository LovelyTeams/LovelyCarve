# filevault-forensic

Pure-Rust decryptor and forensic analyzer for Apple **CoreStorage / FileVault 2
(FVDE)** encrypted volumes (macOS 10.7 Lion – 10.15 Catalina, AES-XTS-128).

- **`filevault-core`** (`use filevault`) — parse the CoreStorage volume header +
  metadata, derive the volume master key (PBKDF2-SHA256 → RFC 3394 AES key
  unwrap), and AES-XTS-decrypt the logical volume. Exposes
  `FileVaultVolume::unlock_with_password` with `read_at` + a `Read + Seek`
  decrypted view.
- **`filevault-forensic`** — severity-graded
  [`forensicnomicon::report::Finding`]s over the parsed metadata (protector
  inventory, encryption state, weak-KDF), no password required.

## Quick start

```rust
use std::fs::File;
use filevault::FileVaultVolume;

let mut vol = FileVaultVolume::unlock_with_password(
    File::open("corestorage.raw")?, "s3cret")?;
let mut buf = [0u8; 512];
vol.read_at(1024, &mut buf)?;   // decrypted HFS+ volume header
# Ok::<(), filevault::FileVaultError>(())
```

## Trust, but verify

- **Panic-free**: `unsafe` forbidden; `unwrap`/`expect`/unchecked indexing denied
  in non-test code; every image-supplied offset/length is bounds-checked.
- **Fuzzed**: a `cargo fuzz` metadata target (must-not-panic).
- **Validated against an independent oracle on real data** — see
  [Validation](validation.md).

## Scope

CoreStorage / FileVault 2 only. APFS-native encryption (10.13+) is a distinct
format and is deferred — see [Deferred Scope](DEFERRED.md).
