# LovelyCarve integration

backhand 0.25.5 from crates.io, source bf678c7f923799a57d972cf695daf69d8c6ddafb (MIT OR Apache-2.0).

The standalone manifest trims upstream benches, integration-test harnesses and dev dependencies, retains xz/gzip/zstd/parallel and readable error strings, and enables no_std_io2/std for host I/O compatibility. Other compression features remain optional and disabled in the host. SquashFS root names use the on-disk slash rather than the host platform root separator.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).

## Firmware compatibility (2026-10-21)

The host enables `v3_ddwrt`, which also enables v3/gzip. This optional feature
decodes DD-WRT's explicit four-byte pb/lc/lp prefix with pure Rust `lzma-rust2`,
bounded by the metadata/data block limit. It adds `hsqt` / `tqsh` kinds without
enabling the adaptive C/C++ LZMA search dependency. Blocks may vary their parameters.
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
