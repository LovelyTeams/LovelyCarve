# 4. Present the decrypted volume as a `forensic-vfs` EncryptionLayer, behind an optional `vfs` feature

Date: 2026-07-24
Status: Accepted

## Context

The fleet's universal container/filesystem abstraction (`ronin-issen/CLAUDE.md` →
"VFS & Universal Container Abstraction") composes an evidence stack —
`container → volume system → encryption layer → filesystem` — so a consumer reads
`E01 → GPT → FileVault → HFS+` as one `ImageSource` without knowing one crypto
format from another. For FileVault to slot into that stack, the decryptor must
implement `forensic-vfs`'s `EncryptionLayer` contract: given the ciphertext byte
source and a credential, hand back the decrypted volume as a `DynSource`.

Not every consumer of `filevault-core` wants the `forensic-vfs` dependency (a
Rust tool that only needs the standalone `FileVaultVolume` decryptor should not
pull the whole VFS contract crate).

## Decision

Add a `FileVaultLayer` adapter implementing `forensic_vfs::EncryptionLayer`
(`core/src/vfs.rs`), gated behind an optional, non-default `vfs` feature:

```toml
# core/Cargo.toml
[features]
default = []
vfs = ["dep:forensic-vfs"]
```

The adapter wraps the ciphertext `DynSource`, and on `open` tries each offered
`Credential::Password` over a fresh `Read + Seek` view via
`FileVaultVolume::unlock_with_password`; a non-password credential is skipped
(`NeedCredentials`), a wrong password surfaces as `VfsError::Decode`. The
decryption is `filevault-core`'s own audited RustCrypto (ADR 0003); this module
only wires the contract. The adapter tracks the `forensic-vfs` registry version
(currently `0.7`).

The `vfs` feature is left out of `default` because it is the batteries-included
"genuinely optional subsystem for outside consumers" exception
(`ronin-issen/CLAUDE.md` → "Batteries-Included"): the library default stays lean
for third-party reuse, while any fleet consumer that composes evidence stacks
turns `vfs` on.

## Consequences

- FileVault composes into the fleet VFS engine as a first-class encryption layer;
  a mount or carver reads a decrypted FileVault volume through the same
  `ImageSource` as any other stack, no special-casing.
- The adapter must chase `forensic-vfs` API changes; the git history shows this
  (migrations across 0.1→0.7, and the `CryptoLayer`→`EncryptionLayer` rename at
  commit `3caf502`). Committing `Cargo.lock` (ADR — see the workspace lock)
  stabilizes cargo-vet against that churn.
- The `vfs`-gated code is validated both against the real oracle
  (`filevault_cryptolayer_decrypts_fvdetest`) and by always-on synthetic branch
  tests so the coverage gate holds with the feature on but the oracle absent
  (`docs/validation.md` → "vfs EncryptionLayer adapter").
