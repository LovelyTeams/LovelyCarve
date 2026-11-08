# amiga-reader 0.1.0

LovelyCarve's AGPL-3.0-only read-only adapter for the existing pure Rust
`amiga-ffs` 0.4.0 and `amiga-rdb` 0.4.1 crates by Simon Dick (2026), each
MIT OR Apache-2.0. Their original licenses remain in the adjacent vendor
directories. No C filesystem code or host mounting API is used. The source
implements only `BlockSource`, never the separate `BlockSink` writer trait.

Existing Rust crates were examined first: `affs-read` 0.5.1 (MIT), revision
`d3fbbe4d4f8ad7d47266a019a8a9949db9568a87`, published crate SHA-256
`88294fd323fd08e5f7e784fd7399167944df314ff8c825d0d9c981a83e437798`,
and fstool 0.5.0 (MIT), revision
`b30823f25b6eef5ea9b6de45876f31c2b8f2d70a`. They were inspected, not copied
or linked. amiga-ffs was selected for DOS0..7, LNFS, OFS cross-reference
validation, seeking and an explicit read/write trait boundary.

The wrapper checks allocation bitmap provenance, root/header/name/hash
geometry, hardlink type and cycles, per-file block overlap, source bounds and
exact logical reads. It coalesces contiguous FFS pointers; OFS retains checked
per-block reads. Dates are offset-less 1978-epoch civil times with 50 Hz ticks.
Native Latin-1 bytes round-trip through Unicode display strings. Unknown
POSIX mode is not fabricated. Dirty volumes require external disk validation;
BADB remapping and nonstandard RDB sector-origin layouts are refused.

The only maintained amiga-ffs source patch bounds collected file pointers
by the logical byte size before allocation/growth. The lamfold trait adds
default canonical-inode and native-link-text methods; these preserve alias
header dates separately from shared hardlink identity and raw link bytes.

Primary references: AmigaOS DCFS/LNFS structures, AmigaOS file-system guide,
and Amiga ROM Kernel Reference Manual: DOS. See the links and precise support
boundary in [AMIGA.md](../../docs/formats/AMIGA.md).

## Independent fixtures

`scripts/amiga-fixtures.py` and its invented content are CC0-1.0. It uses
Christian Vogelgsang and contributors' amitools 0.8.1, GPL-2.0-or-later,
as an external Python formatter/reader oracle, not a runtime dependency.
Source archive:
https://files.pythonhosted.org/packages/de/01/b837b2fbb7e1dc2a5f5b94149ba59c71293578f2373ae4bb926db0e53c04/amitools-0.8.1.tar.gz
SHA-256: `f622c0725c15737e7d4820ed147f930f4cfb7b80b04c786c2bc3943b799faf7e`.

No formatter source is copied into this crate. DOS0..7 files and RDB volumes
are created and reopened with the independent oracle. LNFS names use an
explicit raw NaC edit to work around the oracle writer's name-as-comment bug;
the unmodified oracle reader verifies the 107-byte name and file content.
Link headers use independent documented field edits over an oracle-created
allocation/hash layout; the oracle does not support parsing those link types.
The decoded/compressed hashes and licenses are recorded in
`tests/fixtures/manifest.json`. Tests separately mutate malformed structures.
