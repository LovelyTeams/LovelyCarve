# classic-hfs 0.1.0

Read-only pure Rust adaptation, maintained by LovelyCarve. No C filesystem
library, FUSE, kernel driver, formatter or writer is compiled or invoked.

- Metadata field layouts, extent/leaf traversal starting point and MacRoman
  decoder: KarpelesLab/fstool 0.5.0, revision
  `b30823f25b6eef5ea9b6de45876f31c2b8f2d70a`, `src/fs/hfs/mod.rs` and
  `src/macroman.rs`, MIT (`LICENSE-MIT`, original copyright retained).
  https://github.com/KarpelesLab/fstool/tree/b30823f25b6eef5ea9b6de45876f31c2b8f2d70a
- Classic MacRoman collation ranks: Dani's danifunker/rusty-backup 0.1.0,
  revision `6e044a314f6523045999058f85a882c1eeb64b50`,
  `src/fs/hfs_common.rs::HFS_CHARORDER`, AGPL-3.0 (`LICENSE`). That source
  attributes the table to hfsutils `libhfs/data.c`, copyright (C) 1996–1998
  Robert Leslie, GPL-2.0-or-later; its original license is retained as
  `LICENSE-hfsutils`. The combined adaptation is AGPL-3.0-only.
  https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/src/fs/hfs_common.rs
- External fixture `tests/fixtures/hfsutils.img.zst`: same rusty-backup revision,
  originally `tests/fixtures/test_hfs.img.zst`. Original generation script is
  retained as `tests/fixtures/create_test_fixtures.sh`: hformat/hmount/hcopy,
  volume `test_hfs`, hello.txt and subdir/nested.txt, 819200 bytes after unpacking.
  The upstream package is AGPL-3.0; no separate fixture license was declared.
- Invented fixtures and `scripts/hfs-fixtures.py`: CC0-1.0. Compressed and
  decoded fixture hashes are recorded in `tests/fixtures/manifest.json`.

`hfs-reader` 0.1.1 was also examined before implementation; its borrowed
whole-image / whole-fork interface does not fit positioned evidence sources.
It is not a dependency and no code is copied from it.
https://docs.rs/hfs-reader/0.1.1/hfs_reader/

Local changes replace permissive partial reads with exact logical/physical
length, allocation bitmap, overlap and overflow-map checks; validate rooted
B-tree topology, key order, allocation maps, leaf links and directory parents;
bound tree files to 64 MiB each and directory depth to 256. Metadata trees may
bootstrap their own extents only from accessible linked leaf nodes, and the
result must match the fully rooted overflow tree. The one-fork map cache is
bounded. Data and resource forks share the same random-access path.

Classic timestamps retain local wall time without a UTC offset. MDB and raw
catalog names/backup dates remain available in the reader API. See
`docs/formats/CLASSIC_HFS.md` for application exposure and limitations.

Format references (not code sources): Apple Inside Macintosh: Files and
TN1150, https://developer.apple.com/library/archive/technotes/tn/tn1150.html
(HFS/HFS+ differences, local dates, fixed classic node/index key sizes).
