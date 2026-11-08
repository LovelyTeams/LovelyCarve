# Deferred scope

## APFS-native encryption (macOS 10.13+)

FileVault has **two distinct on-disk formats**:

1. **CoreStorage / FileVault 2** (10.7 Lion – 10.15 Catalina) — **implemented
   here.** Reference: libyal `libfvde`; oracle: `pyfvde`.
2. **APFS-native (software) encryption** (10.13 High Sierra onward, the default
   once the boot volume is APFS) — **deferred.**

APFS-native encryption is a *different* format: the key hierarchy and encrypted
extents live inside the APFS container (keybag, per-volume VEK/KEK, cryptexts),
not in CoreStorage metadata. `libfvde` does **not** cover it, there is no
settled open-source reference decryptor, no Rust crate, and no ready oracle with
a known password. Attempting it from memory would be the exact
"decrypt-to-wrong-plaintext-silently" failure the oracle-first discipline exists
to prevent.

It is a separate future phase: it belongs with an APFS container reader
(`apfs-forensic`) and needs its own authoritative spec + independent oracle
before any crypto is written.

## Recovery-password / institutional-key protectors

The `PassphraseWrappedKEKStruct` unlock path is protector-agnostic — a recovery
password (`XXXX-XXXX-…`, used verbatim including dashes as the PBKDF2 password)
would unlock through the same code. It is **not validated** here because the
available oracle volume carries only a user-password protector. Unlocking with a
recovery password should work by construction but is untested against an oracle;
treat it as unvalidated until a recovery-key oracle is sourced.
