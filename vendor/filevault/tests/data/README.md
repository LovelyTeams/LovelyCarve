# Test data provenance

This repo's integration tests validate against a **real, third-party encrypted
CoreStorage / FileVault 2 volume** with a published password. The image is large
and **not committed** — it is downloaded and carved manually, and the Tier-1 test
(`core/tests/oracle_fvde.rs`) is env-gated on `FVDE_ORACLE_IMAGE` (it skips
cleanly when the variable is unset).

See the fleet catalog: [`issen/docs/corpus-catalog.md`](../../../issen/docs/corpus-catalog.md).

## fvdetest CoreStorage volume (Tier-1 oracle)

- **Source**: [log2timeline/dfvfs](https://github.com/log2timeline/dfvfs)
  `test_data/fvdetest.qcow2` (Apache-2.0 licensed → redistributable).
- **Download**: <https://github.com/log2timeline/dfvfs/raw/main/test_data/fvdetest.qcow2>
- **md5 (fvdetest.qcow2)**: `dd7b1d584f2e07112ec7003d5fcd9864`
- **Password**: `fvde-TEST` (dfvfs constant `_FVDE_PASSWORD`).
- **Layout**: GPT disk; CoreStorage partition = GPT part 1, LBA 40 → byte offset
  20480, length 536829952 (511 MiB). Logical volume `TestLV`, 160 MiB,
  AES-XTS-128, PBKDF2 90506 iterations.
- **Oracle**: pyfvde (`pip install libfvde-python`).

### Carve the CS partition the test reads

```bash
qemu-img convert -O raw fvdetest.qcow2 /tmp/fvde.raw
dd if=/tmp/fvde.raw of=/tmp/fvde-oracle/fvde_cs_p1.raw bs=512 skip=40 count=1048496
# md5(fvde_cs_p1.raw) = 70d12eb04d4bea58472c9c1d445e024a
export FVDE_ORACLE_IMAGE=/tmp/fvde-oracle/fvde_cs_p1.raw
```

Per the fleet Test-Data Provenance Standard, extract working copies to `/tmp`,
never under `~/src`.

### Tests that consume it

- Reader Tier-1: `cargo test -p filevault-core --test oracle_fvde`
- Analyzer Tier-1: `cargo test -p filevault-forensic --test oracle`
- `vfs` EncryptionLayer adapter Tier-1 (needs `--all-features`):
  `cargo test -p filevault-core --all-features --lib vfs`

All skip cleanly when `FVDE_ORACLE_IMAGE` is unset.

### Trap avoided

dfvfs also ships `cs_single_volume.raw`, which is an **unencrypted** CoreStorage
volume (it "decrypts" identically with any password). It is **not** a FileVault
oracle and is not used here — the only encrypted oracle is `fvdetest.qcow2`.
