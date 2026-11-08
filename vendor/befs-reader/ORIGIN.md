# befs-reader 0.1.0

AGPL-3.0-only read-only adaptation maintained by LovelyCarve.

Existing Rust code was examined before implementation: Dani's
`danifunker/rusty-backup` 0.1.0, revision
`6e044a314f6523045999058f85a882c1eeb64b50`, `src/fs/bfs.rs` and its on-disk
layout notes (AGPL-3.0, original license retained as LICENSE). This source
provides the endian-aware packed layouts, block-run / stream algorithms,
inline links / small_data and B+tree node array offsets. It is adapted into
focused modules over lamfold's immutable BlockSource; no writer code is used.

https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/src/fs/bfs.rs

Primary references are Haiku revision
`660ea78079ca9ee36623a0562208476bce89b11e`: `bfs.h`, `Volume.cpp`,
`BlockAllocator.cpp`, `Inode.cpp`, `BPlusTree.h` / `.cpp`, and
`headers/posix/sys/stat.h`. Their format/validation semantics and time
interpretation are expressed in Rust. Haiku's MIT notice is retained in
LICENSE-HAIKU. No C code is compiled or linked, and no native driver or external
filesystem program is required at runtime.

Maintained changes from the Rust source:

- exact source length/range checks, allocation bitmap verification and bounded
  caches; partial final allocation-group bitmaps do not invent extra reserved
  blocks; journal cursors are absolute volume blocks;
- explicit refusal of dirty/nonempty journals, incomplete maps, free or
  overlapping data/inode/array blocks, invalid metadata and excessive work;
- direct/indirect/double-indirect boundary and unit validation, clipped
  preallocation, coalesced data runs and binary-searched logical range reads;
- page-based B+tree traversal with parent key bounds, all-level sibling and
  free-chain validation, dot identities, UTF-8 without lossy substitution;
- native inline/external attribute bytes and type codes, including Haiku's
  S_ATTR mode without requiring the obsolete attribute flag;
- exact signed raw timestamps, conservative unknown/BeOS interpretation and
  explicit provenance-controlled Haiku subsecond/status-time interpretation.

## Independent invented fixtures

`scripts/befs-fixtures.py`, its invented bytes and the virtual large-source
test are CC0-1.0. No Rust formatter or reader is imported by the Python encoder.
Decoded and compressed hashes are in `tests/fixtures/manifest.json`. Synthetic
tests are not a native formatter/fsck oracle.

## Native public input

The opt-in native audit uses the official Haiku hrev60295 x86_64 anyboot image:

- ZIP URL: https://haiku-nightly.cdn.haiku-os.org/x86_64/haiku-master-hrev60295-x86_64-anyboot.zip
- ZIP bytes: `484684629`; publisher SHA-256:
  `c235ac08f39e7a9b6a83b84a18d39bad264e0cc98a5616b5dcff498304c70715`
  (also published at the URL plus `.sha256`).
- Decoded ISO bytes: `690814976`; SHA-256:
  `d5817bb4b77692f152156a45af3d9344fbf3c28914773bb38f3833bd8477a316`.
- Native BFS: MBR type 0xeb, LBA `12288`, length `1331200` 512-byte sectors;
  filesystem blocks are 2048 bytes.
- Independent package URL:
  https://eu.hpkg.haiku-os.org/haiku/master/x86_64/r1~beta6_hrev60295/packages/haiku-r1~beta6_hrev60295-1-x86_64.hpkg
- Package bytes: `40048778`; SHA-256:
  `f624ff224ac0cc68c22300e15babc3e058b09fc145f0af19f3257c0c5e9c85dc`.
  The test compares this independently downloaded artifact to the full file
  `/system/packages/haiku-r1~beta6_hrev60295-1-x86_64.hpkg` in native BFS.

`just test-befs-native` runs `scripts/befs-native-oracle.py` to check both
publisher artifacts and the decoded ISO before executing the explicitly
ignored native test. `--offline` reuses verified inputs; `--cache <directory>`
selects the task cache. An incomplete HTTP body is resumed with validated
Content-Range/ETag, never accepted as a complete input.

Native OS and package content remains external, with its own upstream licenses;
it is not copied into this repository. The audit does not prove real BeOS/PPC,
real double-indirect maps or every block size. See
[BEFS.md](../../docs/formats/BEFS.md) for exact capabilities and limits.
