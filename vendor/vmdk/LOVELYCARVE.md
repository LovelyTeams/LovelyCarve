# LovelyCarve integration

Vendored from <https://github.com/SecurityRonin/vmdk-forensic>, core crate
`vmdk-core` 0.8.4, commit `8a9831125c6db5cff9a24414481037487a92d410`.
Apache-2.0; see LICENSE. Only the reader crate and committed test images are
included, not the forensic analyzer or CLI. The standalone Cargo manifest
expands workspace dependencies; test paths point to local `tests/data`.

Local modifications:

- Descriptor detection accepts BOM and leading whitespace and limits text to
  1 MiB; opening and dependency enumeration use the same encoding decoder.
- Metadata keys accept whitespace around `=`; malformed CIDs/extents fail;
  flat offsets allow trailing comments and reject arithmetic overflow.
- Split sparse extents validate capacity and checked byte geometry; compressed
  split extents are explicitly rejected instead of treated as raw data.
- Companion paths use the reader's existing directory confinement policy.
- seSparse descriptors retain CID/parent metadata and validate extent count
  and capacity against the binary extent.
- `has_layer_data` distinguishes inherited holes from explicit zero grains.
  The application adapter checks parent identity, capacity and cycles, records
  every source file, and propagates short reads as read faults. It rejects
  multi-extent sparse delta layers because the upstream composition erases
  allocation provenance. Base split sparse images remain supported.

Verification: `just test-vmdk`, `just test`, and `just test-cli`.
Committed synthetic/QEMU images are tested locally; optional external corpora
require separately supplied evidence and are not implied by a passing test run.
