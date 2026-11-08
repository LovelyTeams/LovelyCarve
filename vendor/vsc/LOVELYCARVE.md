# LovelyCarve integration

Reader crate `vsc-core` 0.2.2 from
<https://github.com/SecurityRonin/vsc-forensic>, commit
`0af70bed6910fef542224ecd94928b4bc0ae920b`. Apache-2.0; see LICENSE.
Workspace manifest fields are expanded for a standalone crate. The separate
forensic analyzer, fuzz targets, hosted CI, the external PC-MUS-001 oracle
tests and the optional `forensic-vfs` bridge are not included.

The application uses `core/evidence/vss_image.rs` to list each NTFS volume's shadow
copies and to expose one as a session-only `vss://` image; no temporary
extraction or native DLL is involved.

Local changes:

- Snapshot reconstruction is rewritten to apply the store chain. Upstream
  overlaid each store on the live volume alone, so an older snapshot returned
  post-snapshot bytes for every block first overwritten after a newer snapshot
  was taken (on dfVFS `vss.raw`, 67 of the older store's 5,052 blocks differed
  from pyvshadow, while the newest store matched). The rules now follow the
  libvshadow format documentation: block lists fold forwarders through a
  reverse list, merge overlays and drop not-used and self-forwarding
  descriptors; a read falls through to newer stores, forwarders read the next
  store, overlays apply to the requested store only, and only the newest store
  read directly zero-fills unallocated blocks (current and previous bitmap).
  The module docs in `src/reconstruct.rs` state the full procedure.
- Stores are ordered by creation time, the order the chain is applied in.
- Catalog type-0x03 parsing adds the block-list (+8) and previous-bitmap (+72)
  offsets; the block list is read from that pointer instead of being assumed
  to follow the store header.
- Metadata is validated: a chain block that is out of range, is not a
  version-1 VSS block of the expected record type, is reached twice, or a
  forwarder carrying a store offset is `VssError::CorruptStoreMetadata`.
  Out-of-range data reads and reads past the volume are I/O errors rather than
  zero-filled.
- New owned, `Send + Sync` `StoreMap` / `SnapshotLayout` API so one layout
  serves concurrent positioned readers; `Snapshot` keeps the borrowed API.
- `rust-version` is raised to 1.88 (`Option::is_none_or`, `slice::as_chunks`).
- Full comparison with libvshadow commit
  `fa4776f06ec965cde633d3637227562609138c4c` (2026-10-13) and all 48 public
  functions is recorded in [docs/formats/VSS.md](../../docs/formats/VSS.md).
  No libvshadow C code is copied.
- Volume headers validate versions 1/2 and record type; catalogs validate
  version/type, unknown entries, bounds, cycles, limits and unique GUIDs.
  Scattered type-3 pointers are paired after traversal. Corruption no longer
  returns a silently truncated successful catalog.
- `check_volume_signature` restores the reader position. `header()` exposes
  volume/storage IDs and maximum size; descriptors retain range-list pointer,
  raw NTFS reference and allocated size, and expose `has_in_volume_data()`.
- `store_info` reads/validates the entire 16 KiB header block as libvshadow
  does, without trusting its information-size field. String bounds and odd
  UTF-16 lengths are errors; the existing lossy surrogate decoding remains.
- `store_blocks` enumerates raw nonempty records (not folded map entries);
  `store_ranges` walks/validates optional type-5 range chains. Common metadata
  traversal moved to `metadata.rs`; each store has a 4096-block input budget
  (64 MiB; decoded maps need additional memory). Exceeding it fails explicitly.
- `snapshot_size` separates logical geometry from raw catalog values and
  source length. Consistent in-source sizes use the catalog; otherwise primary
  and backup NTFS geometry must agree. Saved diff-area data is checked against
  source length even for a smaller logical snapshot.
- Snapshot implements `Read + Seek`; positioned reads retain cursor position.
  Store-chain reads are iterative and validate every physical source range
  before calling the reader. Active unaligned descriptors are rejected.
- `inspect` example / `just probe-vsc` exposes metadata and streaming snapshot
  hashes; `just clippy-vsc` lints all targets and `just compare-vsc` runs the
  existing differential corpus runner (including catalog rejections).

Tests: `just test-vsc` runs the synthetic chain tests;
`VSC_ORACLE_IMAGE=<dfVFS vss.raw> just test-vsc` adds the pyvshadow oracle;
`just test vss_image` covers the application adapter (set `LOVELYCARVE_VSS_TEST`
to `vss.raw` for the end-to-end NTFS check). The random multi-store
differential against pyvshadow is described in `tests/oracle/README.md`.
Optional oracles skip when their data is absent, so a passing run alone does
not mean they were exercised.
