# 6. Validate decryption against an independent oracle (pyfvde / libfvde) on real data

Date: 2026-07-24
Status: Accepted

## Context

A decryptor is the textbook case where self-authored fixtures are dangerous: a
synthetic image you encode yourself validates only your own assumptions and
passes while the code is wrong (the "LZNT1 trap" the fleet disciplines call out —
`~/.claude/CLAUDE.core.md` → Doer-Checker / Research-First). A wrong offset or an
inverted key half would still round-trip a fixture you built to match the bug,
and in a forensic tool that means silently fabricating plaintext. Correctness
here must be proven against an **independent oracle on real data** (Tier-1),
not fixtures we authored.

## Decision

Validate `filevault-core` against **pyfvde** (the Python binding of libyal
`libfvde`, the reference CoreStorage implementation) on a real encrypted volume:

- **Oracle artifact**: `fvdetest.qcow2` from log2timeline/dfvfs `test_data/`
  (Apache-2.0), password `fvde-TEST`, md5 `dd7b1d584f2e07112ec7003d5fcd9864`
  (`docs/validation.md`). The CoreStorage partition is carved with
  `qemu-img convert` + `dd`.
- **Tier-1 sector check** (`core/tests/oracle_fvde.rs`), env-gated on
  `FVDE_ORACLE_IMAGE` so it skips cleanly when the image is absent: unlock, then
  assert the SHA-256 of decrypted 512-byte sectors at logical offsets
  0 / 1024 / 163840 / 1048576 / 10485760 equal the pyfvde values. Offset 1024's
  decrypted bytes carry the HFS+ volume header (`482b0004…4846534a` = "H+" v4
  "HFSJ"); this is a manual structural observation recorded in `docs/RESEARCH.md`
  that corroborates the sector digest — the automated test asserts only the
  SHA-256, so it is not a separate independent check.
- **Doer-Checker cross-check**: the decryptor was additionally compared against a
  *fresh* pyfvde read at offsets the ground-truth reference never used (512,
  2048, 4096, 8704, 5 MiB, 96 MiB); all matched. Every intermediate crypto stage
  (PBKDF2 output, unwrapped KEK, VMK, tweak key) was reconciled against a debug
  build of `fvdeinfo` (`docs/validation.md`, `docs/RESEARCH.md` ground-truth
  table).
- The Tier-1 oracle runs in CI on real data (commit `92d5acd`).

The authoritative format reference is libfvde's FVDE documentation, cross-checked
offset-by-offset in `docs/RESEARCH.md` (Research-First discipline).

## Consequences

- Correctness is anchored to a third party's answer key on a real volume, not to
  our own fixtures — the strongest tier the discipline defines.
- The heavy oracle image is env-gated and not committed. A hermetic synthetic
  image (`core/src/test_support.rs`) exercises the full unlock/decrypt wiring
  from committed bytes, but the `coverage` job (`.github/workflows/ci.yml`) also
  fetches and carves the real dfvfs oracle before running `cargo llvm-cov` — that
  real-decrypt reach is what covers the `vfs` adapter's oracle-gated test, whose
  skip arm is marked `// cov:unreachable: CI provides the oracle`. So the
  committed coverage gate depends on the network-fetched oracle, not on committed
  bytes alone; this deviates from the fleet's "coverage gate satisfiable from
  committed bytes; live tools in a separate skip-when-absent job" rule (a gap to
  close by covering that arm hermetically). Tier-1 correctness
  (`core/tests/oracle_fvde.rs`) additionally runs as its own env-gated,
  skip-when-absent test.
- Recovery-password / institutional-key protectors are unlocked by the same
  `PassphraseWrappedKEKStruct` path but are **not** oracle-validated (the
  available volume carries only a user-password protector); they are flagged
  unvalidated in `docs/DEFERRED.md`, not claimed as tested.
