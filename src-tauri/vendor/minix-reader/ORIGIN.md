# Minix reader provenance

Superblock and inode layout parsing is adapted from Dani's
[danifunker/rusty-backup](https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/src/fs/minix.rs),
revision `6e044a314f6523045999058f85a882c1eeb64b50`, package 0.1.0,
manifest license `AGPL-3.0`. The original AGPL v3 text is retained in LICENSE;
author attribution is retained in Cargo.toml. This adaptation is AGPL-3.0-only.
No upstream writer, repair code, formatter or GUI is included.

Local changes: checked endian-aware fields, full geometry/bitmap/source bounds,
V3 derived first zone, independent V1/V2 timestamp roles, live allocation bits,
lazy target-zone lookup through all indirect levels, correct block-sized fanout
for multi-block zones, exact sparse/ranged reads, cycle and alias checks,
bounded reader-owned inode/metadata caches, safe directory names and paths.
The generic frontend uses the existing lamfold contract. No C/FFI, system
mount or extraction command is needed at runtime.

Native layout/addressing reference (no C implementation copied or built):
- MINIX `minix/fs/mfs/read.c`, `read_map` / `rd_indir`:
  https://github.com/Stichting-MINIX-Research-Foundation/minix/blob/master/minix/fs/mfs/read.c
- Linux v6.12 `include/uapi/linux/minix_fs.h` and `fs/minix/minix.h`.

Reference data: util-linux
`fbfd2ef405d048e8fc01b309b051d2b403b09853`, tests/expected/minix and
tests/ts/minix/fsck-images (GPL-2.0-or-later). The original dumps, generation
script and GPL text are retained under tests/fixtures/util-linux. Each dump
records a successful mkfs.minix and fsck.minix; the image bytes are reconstructed
losslessly, then zlib-compressed. These reference volumes contain only the root
directory. They are not a claim that Linux tools ran locally during testing.

Populated fixtures and scripts/minix-fixtures.py are invented CC0-1.0 data.
They encode layouts independently of the Rust reader. Hashes and sources are in
tests/fixtures/manifest.json. Supported dialects and verification limits:
[Minix FS](../../../docs/formats/MINIX.md).
