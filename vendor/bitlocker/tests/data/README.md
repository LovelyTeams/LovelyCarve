# Minimized BitLocker regression evidence

`bdetogo-projection.bin.gz` is a **partial projection**, not a complete disk
image, of the Apache-2.0 dfVFS `bdetogo.raw` fixture by Joachim Metz and the
dfVFS project. Source:
<https://raw.githubusercontent.com/log2timeline/dfvfs/main/test_data/bdetogo.raw>.
The complete source is 67,108,864 bytes, MD5
`fcba22f9363388101ae66c741588bc45`; its published password is `bde-TEST`.

`bdetogo-projection.json` records the exact retained ciphertext ranges and
SHA-256 of the expanded projection. All other bytes are zeroed. Retained
ranges contain the original header, all three metadata blocks and the entire
relocated header region (which includes the oracle's logical test offsets).
Projection generation copies these ranges unchanged into a zero-filled buffer
of the original length, then gzip-compresses it with `mtime=0`.

Only retained sectors are valid oracle inputs. Tests must not compare a full
export hash to the complete image or treat omitted ciphertext as evidence.
The adapter and CLI tests compare known logical sectors against the independent
pybde SHA-256 values published in upstream `core/tests/oracle_bdetogo.rs`.
The complete unmodified image is separately tested with `BDE_ORACLE_IMAGE`.

`recovery-cbc.bin` is a synthetic AES-128-CBC / recovery-password fixture,
independently encrypted with PyCryptodome by `generate_recovery_fixture.py`.
Its test-only recovery password is eight `111111` groups. The expected relocated
boot plaintext is `bytes((i & 255) ^ 0x3c for i in range(512))`. This checks
crypto interoperability and adapter dispatch, not a Windows-issued volume.


## LovelyCarve libbde capability vectors (2026-10-13)

`parity-8000..8005-{512,4096}.bin` and `parity-startup.bek` are synthetic
projections generated independently by `generate_parity_vectors.py` using
PyCryptodome (CCM, ECB, CBC) and cryptography (XTS). Elephant uses the published
A/B modular-word transforms. No Rust encryption function produces these bytes.
The generator and generated fixtures are LovelyCarve contributions under the
vendored crate's Apache-2.0 license. `parity-sha256.txt` records every digest.

Compact format: u16 method, u16 sector size, u32 metadata-block length, 512-byte
boot header, metadata block, relocated ciphertext sector, ordinary ciphertext
sector. `tests/parity.rs` expands this into an in-memory volume; the omitted
bytes are synthetic test padding. FVEK bytes are 1..key-size; diffuser TWEAK
starts at 65; VMK is 32 `46` bytes; clear/BEK key is 32 `55` bytes. Plaintext
is `(i * 7 + 19) mod 256`, with bytes 3..11 replaced by `NTFS    `.

These vectors prove independent crypto interoperability and reader wiring,
not Windows-generated evidence. Vista and EOW tests alter known structures,
exercise raw/encrypted bits, record selection, backup descriptors and bounds.
No full Windows-issued Vista/EOW/8001/4Kn corpus has been claimed as tested.
