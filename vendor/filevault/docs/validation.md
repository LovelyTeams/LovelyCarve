# Validation

`filevault-core` is validated against an **independent third-party oracle on a
real encrypted volume** — not fixtures we authored (Evidence-Based Rigor,
tier 1). The oracle is **pyfvde** (the Python binding of libyal **libfvde**, the
reference CoreStorage/FileVault implementation).

## The oracle artifact

| | |
|---|---|
| Image | `fvdetest.qcow2` from [log2timeline/dfvfs](https://github.com/log2timeline/dfvfs) `test_data/` (Apache-2.0) |
| md5 (qcow2) | `dd7b1d584f2e07112ec7003d5fcd9864` |
| CoreStorage partition | GPT part 1, LBA 40 → byte offset 20480, length 536829952 (511 MiB) |
| Password | `fvde-TEST` (dfvfs `_FVDE_PASSWORD`) |
| Logical volume | `TestLV`, 160 MiB, AES-XTS-128, PBKDF2 90506 iterations |

Carve the CS partition: `qemu-img convert -O raw fvdetest.qcow2 x.raw &&
dd if=x.raw of=cs.raw bs=512 skip=40 count=1048496`.

## Tier-1 sector check (`core/tests/oracle_fvde.rs`)

Env-gated on `FVDE_ORACLE_IMAGE` (skips cleanly when unset). Unlocks with the
password and asserts the SHA-256 of decrypted 512-byte sectors at logical
offsets 0 / 1024 / 163840 / 1048576 / 10485760 equal the values pyfvde produces.
Offset 1024 decrypts to the HFS+ volume header (`482b0004…4846534a` = "H+" v4
"HFSJ") — the structural proof of correct decryption.

```
FVDE_ORACLE_IMAGE=/path/to/cs.raw cargo test -p filevault-core --test oracle_fvde
```

## `vfs` EncryptionLayer adapter (`core/src/vfs.rs`)

The optional `vfs` feature exposes a `forensic-vfs` `EncryptionLayer`
(`FileVaultLayer`) that presents the decrypted logical volume as a `DynSource`.
It is validated two ways:

- **Tier-1**, against the same oracle: `filevault_cryptolayer_decrypts_fvdetest`
  unlocks the real `fvdetest` volume through the adapter and asserts the
  decrypted sector at LV offset 1024 matches the pyfvde-derived SHA-256
  (`ebedb804…008990af`). Run with the oracle + `--all-features`:

  ```
  FVDE_ORACLE_IMAGE=/path/to/cs.raw \
    cargo test -p filevault-core --all-features --lib vfs
  ```

- **Always-on (hermetic)**: synthetic-image tests drive every adapter branch
  (open success, wrong-password → `VfsError::Decode`, non-password credential →
  skipped → `NeedCredentials`, empty credentials, `read_at` EOF/clamp) without
  the oracle, so the coverage gate holds even when the image is absent.

## Independent cross-check (Doer-Checker)

Beyond the committed test offsets, the decryptor was compared against a *fresh*
pyfvde read at offsets the ground-truth reference never used — 512, 2048, 4096,
8704, 5 MiB, and 96 MiB (near the logical-volume end). All six SHA-256 digests
matched byte-for-byte. Every intermediate crypto stage (PBKDF2 output, unwrapped
KEK, volume master key, tweak key) was additionally reconciled against a debug
build of `fvdeinfo`.

## Password enforcement

A wrong or absent password is **rejected** (the RFC 3394 unwrap fails its
integrity check) — decryption never proceeds to produce wrong plaintext. This is
confirmed against the oracle: `fvde-TEST` unlocks; `wrong-pw` errors.

## Deferred

APFS-native encryption has no reference decryptor or oracle and is not
implemented — see [Deferred Scope](DEFERRED.md).
