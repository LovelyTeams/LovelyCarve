# LovelyCarve integration

backhand 0.25.5 from crates.io, source bf678c7f923799a57d972cf695daf69d8c6ddafb (MIT OR Apache-2.0).

The standalone manifest trims upstream benches, integration-test harnesses and dev dependencies, retains xz/gzip/zstd/parallel and readable error strings, and enables no_std_io2/std for host I/O compatibility. Other compression features remain optional and disabled in the host. SquashFS root names use the on-disk slash rather than the host platform root separator.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).

## Native Rust XZ and Zstandard reading

The host's `xz` feature now uses `lzma-rust2` 0.20.1, removing liblzma and
liblzma-sys. The `xz-static` compatibility alias also selects that Rust codec.
The `zstd` feature reads through `ms-compress-ruzstd` 0.9.1. Upstream's C-backed
Zstandard writer is retained behind a separate `zstd-write` feature, disabled
in the host. Other application features can still have native dependencies;
this statement describes the firmware reader, not the entire application.

XZ framing is independently checked against the XZ specification before
constructing the Rust reader: header/index/footer CRCs, backward index size,
block extents, LZMA2 chunk lengths and the declared dictionary must agree.
The library then validates payload checksums and complete consumption. One
stream and up to three BCJ/Delta prefilters are supported, including combined
chains, with an 8 MiB dictionary and 4 MiB output bound. Zstandard requires one complete
frame, validates a present checksum and limits its window to 8 MiB and output
to 4 MiB. Failed decoding never appends partial bytes to caller output.
XZ writing also uses Rust, trying the requested alternative BCJ filters
independently instead of chaining unrelated architecture transforms.

Independent Python/liblzma and Python zstandard fixtures cover all six
SquashFS BCJ types, supported checks, every truncated length, trailing input,
CRC-valid false index/footer sizes, combined prefilters, oversized history and output. See
[codec fixture hashes](test_data/native-codecs.json).

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

## SquashFS v1/v2

The `v1_v2` feature exposes a bounded legacy reader through the same
`FilesystemReaderTrait`. It reads 1.0 and 2.0/2.1 in both byte orders, old packed
UID/GID and inode fields, 16-/32-bit block lists, 32 KiB zero-length block
encoding, cross-metadata records, shared fragments and long directories.
The parser and vtable stay inside this crate behind an erased byte source.

The wire layout is independently implemented using format facts from
squashfs-tools `db038ef265ed00bb17aaf695a72f9b0c3489eb20`:
`squashfs_compat.h`, `unsquash-1.c` and `unsquash-2.c`. No C implementation is
copied or built. Other surveyed Rust readers (`squashfs_reader`,
`squashfs-async`) restrict their superblocks to v4.

Stored/zlib blocks are bounded and validated. Property-header LZMA with EOS
reuses the existing Rust decoder. For v2 streams without EOS, known-length
push/pull decoding must agree, and the separate vendored `lzma-rs` strict API
requires final range code zero plus genuine EOF. A truncated EOS stream must
not be accepted as a markerless stream; independent fixtures cover that case.

The neutral node header carries signed seconds plus explicit mtime presence:
old special inode types have no mtime, and mkfs time is not substituted.
Existing v3/v4 headers retain their unsigned on-disk timestamp values.
Capabilities, limits and public fixture verification are described in
[SquashFS](../../../docs/formats/SQUASHFS.md).
