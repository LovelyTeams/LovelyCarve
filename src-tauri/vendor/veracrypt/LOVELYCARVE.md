# LovelyCarve integration

Derived from https://github.com/SecurityRonin/veracrypt-forensic at
`e3883a5b347f50b51de70ca08655b2fd4b0885b2` (`veracrypt-core` 0.2.4, Apache-2.0;
the original license is retained). Version 0.3.0 rewrites the reader against
VeraCrypt's own sources (`src/Common/Volumes.c`, `Pkcs5.c`, `Crypto.c`,
`Keyfiles.c`, `Tests.c`, `src/Volume/VolumeLayout.cpp`, `Volume.cpp`) and
cryptsetup's `lib/tcrypt/tcrypt.c`:

- Key derivation adds BLAKE2s-256 and Argon2id, fixes the RIPEMD-160 PIM count
  (`15000 + 1000·PIM` for volumes), adds the pre-boot counts and TrueCrypt's
  own counts (TrueCrypt volumes did not open before).
- Ciphers add Camellia and Kuznyechik and the five cascades that use them.
  XTS is implemented over the RustCrypto block ciphers with one key schedule
  per volume and batched blocks, replacing per-sector re-keying through
  `xts-mode` (now a test-only cross-check).
- Header checks follow VeraCrypt: the field CRC from header version 4, the
  sector size from version 5 (512–4096), finished in-place encryption opens,
  an unfinished one fails; newer header versions are refused.
- Header locations add the embedded backups, TrueCrypt 5's hidden header and
  the system encryption header; `HeaderSelection::Auto` tries the normal and
  hidden headers as VeraCrypt does.
- Keyfiles (`KeyfilePool`, `keyfile::expand` / `read`) and the 128-byte pool
  for passwords over 64 bytes.
- The header search runs derivations in parallel with cancellation and
  progress; reads go through the positional `ReadAt` trait and take `&self`.
- The optional `forensic-vfs` adapter and the per-call `VeraVolume`
  constructors are removed; the application uses `unlock` directly.
- A system encryption view only has to lie inside the source: the encrypted
  area the header records may extend past a drive image cut short after the
  chosen partition, as VeraCrypt accepts (cryptsetup's test drives are such
  copies).

Kept from the earlier integration: data geometry must lie inside the source
and be whole 512-byte units, truncated ciphertext fails instead of being
synthesized, and passwords or keys never appear in `Debug` output.
