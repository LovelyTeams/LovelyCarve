# reiserfs-reader 0.1.0

Pure Rust read-only ReiserFS v3 adaptation maintained by LovelyCarve.

Existing Rust implementations were examined before implementation:

- Dani's `danifunker/rusty-backup` 0.1.0, revision
  `6e044a314f6523045999058f85a882c1eeb64b50`, `src/fs/reiserfs.rs`
  (AGPL-3.0; retained LICENSE). On-disk layouts and read-side algorithms are
  adapted with source bounds, allocation verification, logical-offset handling,
  directory visibility, lossless UTF-8 handling and bounded key-range traversal.
  https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/src/fs/reiserfs.rs
- Marcel Roozekrans's `unearth` 0.4.0 (MIT), revision
  `810689419bbcf99d25932dabf2ac7ebe4fad86b9`, `src/reiserfs.rs`, is a detector,
  not a directory/content reader. It was inspected, not copied or linked.
  Published crate SHA-256:
  `d976dadf706bf234219e0b6fb6f253b684bf1e367828988700b8c15a1b8506ba`.
  https://docs.rs/crate/unearth/0.4.0/source/src/reiserfs.rs

Format semantics were checked against Linux v6.12 `fs/reiserfs/reiserfs.h`,
`super.c`, `bitmap.c`, `stree.c`, `inode.c` and `namei.c` as primary references.
No Linux C implementation is copied, compiled or linked. In particular:

- per-item key/stat-data versions differ from the volume version; internal
  delimiters infer their key version, while v3.6 directories and symlinks may
  still use v1 data keys;
- the 8192-byte superblock uses contiguous bitmaps, while the 65536-byte layout
  distributes later bitmaps; `ReIsEr3Fs` takes its format from `s_version`;
- object locality ID is not necessarily the current parent directory ID;
  the root's `..` points to the special `(0,1)` object;
- timestamps retain unsigned on-disk seconds and distinct POSIX roles;
  recorded `i_blocks` has historical quota-accounting issues and is not used
  to override validated file extents;
- only explicit zero indirect pointers synthesize holes. Missing logical
  coverage, free/out-of-volume blocks, source errors and short reads fail.

https://kernel.googlesource.com/pub/scm/linux/kernel/git/torvalds/linux/+/refs/tags/v6.12/fs/reiserfs/

The upstream whole-tree DFS per object is replaced by pruned key-range walks,
with bounded caches and explicit metadata limits. Adjacent physical blocks and
holes coalesce into runs. Dirty volumes fail without replay or writes. Reiser4
is a separate unsupported format.

`tests/fixtures/reiserfsprogs.img.zst` is the byte-for-byte unchanged upstream
`tests/fixtures/test_reiserfs_v3_6.img.zst` at the pinned revision, created by
mkfs.reiserfs 3.6 and guestfish. It contains nested files, packed tails, indirect
blocks and a symlink. The retained generation script changes the output path
to `REISERFS_FIXTURES_DIR` (resolved before changing directories), renames the
output and replaces stale kernel/version comments. Generating the sample
requires a guest kernel that still has ReiserFS enabled, such as Linux 6.12.
No formatter or kernel dependency is introduced at runtime. Upstream declares
AGPL-3.0 and no separate fixture license. Invented samples and
`scripts/reiserfs-fixtures.py` are CC0-1.0. Their decoded/compressed hashes and
provenance are recorded in `tests/fixtures/manifest.json`.
