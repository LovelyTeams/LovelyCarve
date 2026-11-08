# jfs-reader 0.1.0

Read-only pure Rust adaptation maintained by LovelyCarve. No C filesystem
implementation, kernel driver, formatter, writer or journal replay is linked.

- Metadata layouts and read-side algorithms are adapted from Dani's
  `danifunker/rusty-backup` 0.1.0, `src/fs/jfs.rs`, revision
  `6e044a314f6523045999058f85a882c1eeb64b50` (AGPL-3.0, original `LICENSE`).
- Bounded xtree/dtree validation is adapted from CaddyGlow/partmgr 0.1.2,
  `src/fs_jfs.rs`, published revision
  `d074f660e81dfa6ade66cd07400b7421ad9eb9b9` (MIT, `LICENSE-MIT`).
  Crate SHA-256:
  `0df45e6c77fb2fb3a18f48c1c055a6224fa3114d7619b3b8cb09e06e0ff1d035`.
  https://github.com/CaddyGlow/partmgr/tree/d074f660e81dfa6ade66cd07400b7421ad9eb9b9

The upstream partmgr public baseline is restricted to 4 KiB allocation blocks,
a single IAG, a contiguous two-page inode map, low-level allocation maps and
files without EA/ACL or external persistent directory-index tables. It is not
used wholesale as a replacement for broader JFS reading. The rusty-backup
metadata is adapted with corrected inode-bit indexing, logical/physical units,
internal xtree addressing and name continuation semantics.

On-disk definitions are cross-checked against Linux v6.12 `fs/jfs` headers as
format references. No Linux C implementation is copied or compiled.
https://kernel.googlesource.com/pub/scm/linux/kernel/git/torvalds/linux/+/refs/tags/v6.12/fs/jfs/

The Linux JFS secondary superblock is at byte 61440, not 8 MiB. An otherwise
recognizable primary header declaring dirty/unsupported state must not be
bypassed using an older clean secondary header. Header copy selection remains
explicit in the reader metadata.

Local hardening additionally validates allocation bitmaps, inode identities,
physical block accounting (including EA/ACL and directory-index allocations),
key/slot/free-list order, sibling/self references and exact source reads. Native
Linux timestamp seconds remain unsigned, including dates after 2038. Metadata
cache and tree traversal limits are explicit. See `docs/formats/JFS.md`.

`tests/fixtures/jfsutils.img.zst` and its generation script are from
the same pinned rusty-backup revision (`test_jfs.img.zst` and
`scripts/generate-jfs-fixtures.sh`). They use mkfs.jfs and guestfish to create a
16 MiB Linux volume with nested files, links, deterministic content and a
200-entry directory. Upstream declares AGPL-3.0 and no separate fixture license.
The retained script replaces the upstream machine-specific output path with
`JFS_FIXTURES_DIR` (defaulting to its own directory) and corrects its stale
version-1 comment; the committed image itself is byte-for-byte unchanged.
No mkfs/guestfish runtime dependency is introduced. Invented fixtures and
`scripts/jfs-fixtures.py` are CC0-1.0; decoded/compressed SHA-256 and source paths
are recorded in `tests/fixtures/manifest.json`.
