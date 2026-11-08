# LovelyCarve integration

Vendored reader crate `bitlocker-core` 0.3.5 from
<https://github.com/SecurityRonin/bitlocker-forensic>, commit
`e6674dcda2413b1a02a915f2255af8fda787a154`. Apache-2.0; see LICENSE.
Workspace manifest fields are expanded for a standalone crate. The separate
forensic analyzer and hosted CI are not included.

Local changes (2026-10-13):

- Authenticated unwrap tries every protector of the requested kind; raw 256-bit
  VMKs authenticate the FVEK without requiring a recovery-password protector.
- All six AES methods, including AES-256-CBC + Elephant Diffuser; data-unit
  length follows the logical sector size, including XTS tweak calculation.
- Vista v1 metadata and plaintext boot restoration (NTFS signature/MFT mirror).
- Used-space-only EOW descriptor/map/dual-record bitmaps, bounded allocations,
  newer-sequence selection and metadata blanking.
- Complete/truncated entry bounds; metadata-copy fallback, including Vista
  offsets from the block header. Full reserved metadata regions (16/64 KiB)
  and the original relocated-header storage read back as zeros.
- Direct FVEK/TWEAK, clear-key and BEK reach application IPC/CLI/UI. BEK GUIDs
  must match protector GUIDs. Raw sector keys have no authentication tag.
- Metadata inspection exposes identity, FILETIME, UTF-16 description and
  protector identifiers/types without sending key payloads through IPC.
- The weak, bounded application source rejects audited synthetic ciphertext;
  no FFI, evidence-handle ownership cycle or whole-volume temporary extraction.

Detailed pinned libbde comparison, limitations and reproduction:
[docs/formats/BITLOCKER.md](../../docs/formats/BITLOCKER.md).

Tests: `just test-bitlocker`, `just clippy-bitlocker -- -D warnings`,
`just test bde_image --lib`, `just test bitlocker_source --lib`, `just test-cli`.
`BDE_ORACLE_IMAGE` enables the independent oracle over the complete dfVFS image;
this corpus was exercised on 2026-10-13. Other external oracle tests skip when
absent; a passing harness status does not imply those datasets were exercised.
Independent Python vectors cover six methods × 512/4096 bytes, not Windows
issuance. Vista/EOW have structure regressions, not new real-image oracles.
Fixture provenance and SHA-256 lists: `tests/data/README.md`.
