# F2FS reader provenance

The metadata modules `checkpoint.rs`, `superblock.rs`, `nat.rs`, `constants.rs`
are adapted from KarpelesLab/fstool 0.5.0 (MIT). `LICENSE-MIT` is preserved.
No writer, host device, formatter or filesystem mutation API is included.
Source package: https://crates.io/crates/fstool/0.5.0
Repository: https://github.com/KarpelesLab/fstool
The pinned package VCS revision is recorded in `origin.json`.

The frontend, inode/indirect/inline reader, directory parser and tests are original
LovelyCarve code, AGPL-3.0-only (`LICENSE`). On-disk layouts and CRC conventions
were checked against Linux v6.12 `include/linux/f2fs_fs.h`, `fs/f2fs/f2fs.h`,
and `fs/f2fs/inode.c`. No kernel C code is built or invoked.

Local metadata changes: checkpoint head/footer CRC and version validation,
wrap-aware pack choice, bounded packs, exact NAT bitmap geometry, and strict
journal counts/duplicate nids. The new frontend validates source/main-area
ranges, node ownership/footers, inode and optional superblock CRCs, dynamic
extra/xattr address counts, full signed 64-bit UTC timestamp roles and nanoseconds.
Unsupported encrypted/compressed/casefold/alias content fails explicitly while
metadata remains visible. Runtime I/O goes through a read-only bounded source.

Scope, validation and limitations: [F2FS](../../docs/formats/F2FS.md).
