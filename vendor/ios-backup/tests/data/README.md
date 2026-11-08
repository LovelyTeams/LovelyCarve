# Test data provenance

Every artifact this repository's tests read is listed here with its origin, how
it was produced, and which test consumes it. An artifact not listed here is a
defect: a fixture whose lineage nobody recorded is unverifiable by construction.

## Classification

| Tier | Meaning |
|---|---|
| **T1** | A third party authored both the artifact and the answer key. |
| **T2** | Real engine/tool output, or a real-world artifact, with ground truth derivable from documented construction or confirmed by an independent oracle. |
| **T3** | We authored both the fixture and the expected answer — proves self-consistency only. |

---

## `encrypted-backup/` — synthetic, T2

- **Source:** minted by [`tools/mint_encrypted_backup.py`](../../tools/mint_encrypted_backup.py)
  in this repository.
- **Generator command (verbatim, reproduces byte for byte):**

  ```bash
  python3 tools/mint_encrypted_backup.py tests/data/encrypted-backup
  ```

- **Password:** `test-password-1234`
- **Seed:** `20260821` (fixed; all key material is derived from it, so the
  fixture is deterministic).
- **Contents:** `Manifest.plist` (encrypted, with `BackupKeyBag` and
  `ManifestKey`), an AES-256-CBC-encrypted `Manifest.db`, `Status.plist`,
  `Info.plist`, and four content blobs under their two-hex directories.
- **Redistribution:** ours, Apache-2.0 with the repository. No personal data:
  every byte is generated.
- **Consumed by:** `core/tests/backup.rs`, `core/tests/logical.rs`,
  `forensic/tests/audit.rs`.

**Why this is T2 and not T3.** The generator's cryptography comes from Python's
[`cryptography`](https://pypi.org/project/cryptography/) package (OpenSSL-backed)
and its property lists from the standard library's `plistlib`. Neither shares any
lineage with the RustCrypto crates or the `plist` crate under test. "Python
encrypted it, Rust decrypted it, and the bytes match" is therefore a
cross-implementation agreement on a published format, not a round trip through
one implementation.

**What it still does not prove.** We wrote the generator, so it encodes our
reading of the format. A misreading shared by generator and reader would pass.
That residual risk is what the RFC vectors and the real-backup tests exist to
close — see [`docs/validation.md`](../../docs/validation.md).

## `plain-backup/` — synthetic, T2

- **Generator command:**

  ```bash
  python3 tools/mint_encrypted_backup.py tests/data/plain-backup --plain
  ```

- Same seed, so it holds **byte-identical plaintext** to `encrypted-backup/`.
  That is deliberate: it makes the plain fixture the answer key for the
  encrypted one, and a decrypt is correct exactly when it reproduces it.
- **Consumed by:** as above.

### Fixture contents

Chosen to exercise the reader rather than to look realistic:

| Domain | Path | Payload | Exercises |
|---|---|---|---|
| `HomeDomain` | `Library/Preferences/com.apple.example.plist` | 4 bytes | Shorter than one AES block. |
| `HomeDomain` | `Library/SMS/sms.db` | 316 bytes | Spans several blocks; its fileID is a published constant. |
| `AppDomain-com.example.app` | `Documents/exact.bin` | 32 bytes | **Exactly** two AES blocks — the case a padding-stripping reader silently truncates. |
| `CameraRollDomain` | `Media/DCIM/100APPLE/IMG_0001.JPG` | 768 bytes | Binary payload spanning many blocks. |
| `HomeDomain` | `Library/SMS` | — | A directory row: metadata, no blob, no encryption key. |

## `no-size-backup/` — synthetic, T3

- **Generator command:**

  ```bash
  python3 tools/mint_encrypted_backup.py tests/data/no-size-backup --plain --omit-size
  ```

- Identical to `plain-backup/` except that the **first file's metadata carries no
  `Size` key**.
- **Consumed by:** `core/tests/missing_size.rs`.

## `no-size-encrypted/` — synthetic, T3

- **Generator command:**

  ```bash
  python3 tools/mint_encrypted_backup.py tests/data/no-size-encrypted --omit-size
  ```

- The encrypted twin of the above: the first file has no `Size` **and** its blob
  is AES-CBC encrypted, so reading it exercises the PKCS#7 fallback rather than
  truncation.
- **Password:** `test-password-1234`
- **Consumed by:** `core/tests/missing_size.rs`.

T3 and honestly so: we authored both the malformation and the expected result.
It is a regression guard for a defect found by reading
`datatags/mount-ios-backup`, not independent evidence — an absent `Size` was
being read as `0`, truncating a real file to nothing while reporting success.

## `evil-file-id/` — synthetic, T3, adversarial

- **Generator command:**

  ```bash
  python3 tools/mint_encrypted_backup.py tests/data/evil-file-id --plain --evil-file-id
  ```

- The first file's `fileID` is `../../../../../../../../etc/passwd` rather than a
  digest, and **no blob is written for it** — the question is whether the reader
  *resolves* the path, not whether something happens to be there.
- **Consumed by:** `core/tests/path_traversal.rs`.

An adversarial fixture, not a realistic one. It exists because `Manifest.db` is
attacker-controllable (ADR-0012) and `fileID` becomes a filesystem path; the
traversal it encodes was live in this crate until 2026-08-21 (ADR-0008), found
by reading MVT.

## `lie-unencrypted/` and `lie-encrypted/` — synthetic, T3, adversarial

- **Generator commands:**

  ```bash
  python3 tools/mint_encrypted_backup.py tests/data/lie-unencrypted --lie unencrypted
  python3 tools/mint_encrypted_backup.py tests/data/lie-encrypted --plain --lie encrypted
  ```

- `--lie` flips only the **declaration** in `Manifest.plist`, never the bytes.
  `lie-unencrypted` is a fully encrypted backup declaring `IsEncrypted = false`;
  `lie-encrypted` is plaintext declaring `true`.
- **Password** (`lie-unencrypted`): `test-password-1234`
- **Consumed by:** `core/tests/encryption_state.rs`, `forensic/tests/audit.rs`.

They exist because `IsEncrypted` is a claim and the bytes are the fact. A reader
that trusts the flag refuses a readable backup in one direction and parses
ciphertext as SQLite in the other. See ADR-0009.

## `blob-file-id/` — synthetic, T3, adversarial

- **Generator command:**

  ```bash
  python3 tools/mint_encrypted_backup.py tests/data/blob-file-id --plain --blob-file-id
  ```

- The first file's `fileID` is stored as a **BLOB** rather than TEXT. Legal:
  `Manifest.db` declares the column `TEXT`, but SQLite enforces affinity, not
  type.
- **Consumed by:** `core/tests/row_loss.rs`.

It exists because a reader matching only `Value::Text` drops such a row —
silently, at the collection stage, which this crate did until 2026-08-21.

### A note on padding

The generator pads with **PKCS#7**, not zeros. That is what iOS actually writes,
confirmed against the same reference implementation, whose `removePadding` reads
the final byte as a pad count (RFC 1423) and works on real backups. The fixtures
were zero-padded until 2026-08-21; a zero-padded fixture would let a reader that
mishandles PKCS#7 pass.

Every test stayed green across that change, with no code modification — which is
itself evidence that truncate-to-recorded-size is padding-agnostic (ADR-0003).

---

## Real iOS backups — not committed, env-gated

Real backups are personal evidence and are **never** committed to this
repository, regardless of size. `core/tests/real_backup.rs` reads one in place
when pointed at it:

```bash
IOS_BACKUP_DIR="$HOME/Library/Application Support/MobileSync/Backup/<UDID>" \
  cargo test -p ios-backup-core --test real_backup -- --nocapture

# Add the production-KDF case (a 10,000,000-round derivation, ~3s):
IOS_BACKUP_SLOW_KDF=1 IOS_BACKUP_DIR=... cargo test --release -p ios-backup-core --test real_backup

# Add the byte-exact decrypt case:
IOS_BACKUP_PASSWORD='<the backup password>' IOS_BACKUP_DIR=... cargo test ...
```

Absent the variables the tests skip in ~0.00s. **That near-zero duration is the
fingerprint of work not done** — a green run here is not validation unless the
variables were set.

### Recorded observations from a real encrypted backup (T2)

Read on 2026-08-21 from a macOS `MobileSync` backup dated 2023-03-19. The device
identifiers are deliberately omitted; what matters is the structure.

| Property | Observed |
|---|---|
| `BackupKeyBag` size | 1388 bytes |
| Keybag `VERS` | 5 |
| Keybag `TYPE` | 1 (Backup) |
| `ITER` (PBKDF2-HMAC-SHA1) | 10,000 |
| `DPIC` (PBKDF2-HMAC-SHA256) | 10,000,000 |
| Class keys | 11, recorded in **descending** order 11…1 |
| `WPKY` width | 40 bytes each (RFC 3394 wrapping of a 32-byte key) |
| `ManifestKey` | 44 bytes (4-byte LE class + 40-byte wrapped key) |

The descending class order is why `KeyBag::class()` looks up by class number and
never by position.

## Corpora we do not yet have

Recorded as gaps rather than left implicit:

- **An encrypted backup with a known password**, for a byte-exact end-to-end
  decrypt against real evidence. `a_real_backup_opens_and_reads_with_the_correct_password`
  is written and wired; it needs `IOS_BACKUP_PASSWORD`.
- **A backup from iOS 18.3+ / 26.x**, to document domain-exclusion behaviour
  empirically rather than assuming it.
- **A backup written by iTunes for Windows**, to confirm the keybag and manifest
  are identical to the macOS Finder path.

---

*Fleet standard: `~/src/ronin-issen/docs/test-corpus-standard.md`. This file is
also indexed in the fleet-wide `docs/test-data-catalog.md`.*
