# LovelyCarve integration

backhand 0.25.5 from crates.io, source bf678c7f923799a57d972cf695daf69d8c6ddafb (MIT OR Apache-2.0).

The standalone manifest trims upstream benches, integration-test harnesses and dev dependencies, retains xz/gzip/zstd/parallel and readable error strings, and enables no_std_io2/std for host I/O compatibility. Other compression features remain optional and disabled in the host. SquashFS root names use the on-disk slash rather than the host platform root separator.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).

## Firmware compatibility (2026-10-21)

The host enables `v3_ddwrt` and `v3_lzma`, which also enable v3/gzip. The DD-WRT feature
decodes DD-WRT's explicit four-byte pb/lc/lp prefix with pure Rust `lzma-rust2`,
bounded by the metadata/data block limit. It adds `hsqt` / `tqsh` kinds without
enabling any adaptive C/C++ LZMA dependency. Blocks may vary their parameters.
The version-neutral filesystem trait now exposes exact bounded range reads for v3/v4.
The newly enabled v3 path also checks table/metadata bounds, directory cycles and
depth, rejects malformed directory data and Windows-unrepresentable names,
uses 32-bit UID/GID tables with GID-255 inheritance, and bounds gzip output.

Format reference: Firmware Mod Kit commit
`c72f45d7a062156125ae00cb867d8a614b50b963`,
`src/squashfs-3.0-lzma-damn-small-variant/squashfs_fs.h` and
`lzma/C/7zip/Compress/LZMA_Lib/ZLib.cpp`. These upstream files are references,
not bundled source. This Rust implementation does not invoke FMK executables.
See [firmware scope and verification](../../../docs/formats/FIRMWARE.md).

## Rust adaptive LZMA

The upstream per-image parameter search/cache is retained, but `v3_lzma` and
`v4_lzma` no longer depend on `lzma-adaptive-sys`. The v3 standard decoder also
uses Rust instead of its liblzma fallback. Only v3 LZMA is enabled by the host.
The ordinary v3 kinds try gzip first, then the bounded LZMA decoder per block;
explicit DD-WRT kinds keep their own prefix interpretation.

Raw / five-byte properties / 13-byte LZMA-alone streams must terminate cleanly,
consume their entire compressed input, and fit the actual metadata/data bound.
Known alone lengths must agree. The adaptive grid matches upstream's lc+lp<=4,
pb<=4 and offsets 0..=10. Redundant dictionary candidates are eliminated because
all are at least the maximum permitted block size; decoder history is capped
to the block output bound. Standard declared dictionaries are limited to 8 MiB.
Legacy streams lacking an end marker are explicitly outside this scope.

`lzma-rust2` 0.20.1's push decoder checks final range-code state, but may use
carry-buffer zeroes for a missing final source byte. Its pull decoder can ignore
an EOF during final normalisation. Both paths are required to agree; the pull
source records every read past EOF. This prevents either tolerance from
publishing a truncated block. Failure never alters the caller's existing output.

Independent Python fixtures cover gzip/raw/alone/properties, both endians,
changing block parameters, cross-block ranges, truncation, trailing bytes,
length mismatch and oversized output. See [fixture provenance](test_data/README.md).
Unblob's external LE/BE fixtures are compared with separately recorded output
bytes through `LOVELYCARVE_SQUASHFS_LZMA`; see the firmware guide for invocation.
