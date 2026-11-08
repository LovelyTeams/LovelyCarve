# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.4.2] - 2026-10-06

### Changed

- `lzma-rust2` moved to 0.21.

## [0.4.1] - 2026-09-10

### Changed

- Every decoder is now pure Rust: `lzfse` → `lzfse_rust`, `xz2` → `lzma-rust2`,
  `bzip2` 0.5 → 0.6 for its `libbz2-rs-sys` backend. The crate no longer builds
  or calls C, and LZFSE no longer needs a 2x scratch buffer to decode

### Fixed

- Build the DMG plist through `plist` rather than concatenating strings; a
  partition name containing `&` or `<` produced an unreadable image

### Removed

- `xml-rs`, unreferenced, and `base64`, now handled by `plist`

## [0.4.0] - 2026-09-07

### Changed

- **Breaking:** DMGs written by udif before 0.4.0 stored a short trailing chunk
  under a block run declaring a full sector, and are now rejected on read rather
  than silently zero-filled; other DMG readers accept them. Re-create affected
  images with 0.4.0 or later.

### Fixed

- Reject LZFSE output that disagrees with the block map, instead of clamping it
  and leaving the rest of the block zeroed

- Reject a raw block run that stores more bytes than the sector count it declares,
  instead of underflowing the zero-fill length

- Pad the trailing chunk of a partition out to the sector count its block run
  declares, so the stored stream decodes to its full declared length
- Fail when a decoder yields fewer bytes than a block run declares, instead of
  leaving the remainder of the block zero-filled and reporting a successful read

## [0.3.4] - 2026-04-12

### Changed

- Include `LICENSE` file in the published crate

## [0.3.3] - 2026-02-16

### Added

- `parallel` feature flag for rayon-based parallel block decompression of DMG partitions
- `decompress_partition_parallel()` and `decompress_partition_to_parallel()` methods on `DmgReader`
- Auto-selecting `_auto` methods that use parallel decompression when the feature is enabled
- `DmgArchive` high-level API now automatically uses parallel decompression when available

## [0.3.2] - 2026-02-16

### Changed

- Rust edition upgraded from 2021 to 2024
- Renamed test fixture `Kernel_Debug_Kit_26.3_build_25D5087f.dmg` → `kdk.dmg`

### Fixed

- Clippy fixes for Rust 2024 edition
- Removed `#[allow(dead_code)]` by dropping unused `options` field from `DmgReader`

## [0.3.1] - 2026-02-16

### Fixed

- Clippy warnings: `manual_strip`, `manual_div_ceil`, `op_ref`, `needless_range_loop`

## [0.3.0] - 2026-02-12

### Fixed

- Block type `0x80000008` is XZ (LZMA2) compressed, not LZVN. Renamed `BlockType::Lzvn` to `BlockType::Xz` and added proper XZ decompression via the `xz2` crate. This fixes DMGs like Google Chrome that use XZ compression.

### Changed

- `CompressionInfo::lzvn_blocks` renamed to `xz_blocks`
- Added `xz2` dependency for XZ decompression

## [0.2.0] - 2026-02-11

### Changed

- Fixture-dependent DMG tests now use `#[ignore]` instead of silent path-exists guards

## [0.1.0] - 2026-02-04

### Added

- DMG/UDIF reading with partition listing and data extraction
- DMG creation via `DmgBuilder`
- Compression support: LZFSE, XZ, Zlib, Bzip2, Raw
- Streaming decompression for partition extraction
- CRC32 checksum verification on read and write
- Cross-platform support (Windows, Linux, macOS)
