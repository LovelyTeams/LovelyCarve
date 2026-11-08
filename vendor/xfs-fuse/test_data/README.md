# Synthetic XFS fixture

`minimal-v4.bin` is a 128 KiB synthetic V4 volume assembled by `generate.py`,
with local directory entries, hard links with raw non-UTF-8 names, a local
symlink, written extents, a hole and an unwritten extent. It contains no user
evidence, third-party source code or downloaded filesystem data. The inode
atime is -2 seconds plus 123 nanoseconds; the unwritten physical block contains
`S` bytes that must never appear in logical reads.

SHA-256: `b1355ddda3a3ac1dac4b0ab468a6dffe561846066faf372bdfa0918926b6103d`.

Regenerate with `python vendor/xfs-fuse/test_data/generate.py`.
Application tests consume the committed bytes. Independent in-memory structures
in `src/libxfuse/regression.rs` exercise versions 1–5, CRCs, B-tree maps,
directories, remote attributes and corrupt metadata. These are synthetic
regressions, not an executed libfsxfs oracle comparison.
