# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.4.0] - 2026-09-19

### Added

- **Extended attributes.** `attributes` module reads the Attributes B-tree;
  `HfsVolume::get_xattr` and `HfsVolume::list_xattrs` expose it. The tree is
  read on first use, so a damaged one fails only the calls that need it rather
  than `HfsVolume::open`
- `testutil::HfsPlusImageBuilder::add_file_with_xattrs` builds a file with
  extended attributes and a resource fork
- `XattrEntry` and the re-exported `XattrKind` classify each listed attribute
  as user data or compression machinery. Nothing is filtered out, so a caller
  reading the volume still sees everything on it, but one replicating metadata
  onto an extracted file can skip the attributes that would break it —
  `read_file` has already applied them
- `ForkData` and `ExtentDescriptor` derive `PartialEq` and `Eq`

### Changed

- **Breaking behaviour:** `stat` derives its kind from the file mode, so a
  symlink is `EntryKind::Symlink` in `stat` as it always was in
  `list_directory`, instead of `File` in one and `Symlink` in the other
- **Breaking behaviour:** `list_directory` and `walk` report the decmpfs
  uncompressed size for a compressed file, matching `stat`; the raw data
  fork of such a file is empty, so listings previously said 0 where `stat`
  said the real length
- **Breaking:** `btree::search_btree` takes a fallible comparator
  (`Fn(&[u8]) -> Result<Ordering>`). A B-tree key that cannot be decoded now
  fails the operation with the parse error instead of ordering `Less`, which
  silently steered the search past the damage and reported records that
  exist — catalog entries, overflow extents, extended attributes — as absent
- **Breaking behaviour:** `read_file` and `read_file_to` now decompress
  transparently compressed (`decmpfs`) files. Their data fork is empty, so
  these previously returned zero bytes for a file `stat` reported as
  non-empty — most files on a macOS system volume are stored this way
- **Breaking behaviour:** `open_file` fails on a compressed file instead of
  returning a reader over the empty data fork, which would have reported a
  successful read of nothing. Use `read_file`
- **Breaking:** `FileStat` gains `compression`, and its `size` is the
  decompressed size for a compressed file
- **Breaking:** `list_xattrs` returns `Vec<XattrEntry>` rather than
  `Vec<String>`

## [0.3.0] - 2026-09-07

### Changed

- **Breaking behaviour:** `HfsVolume::read_file` now fails with
  `CorruptedData` when a fork's extents end before its declared
  `logical_size`, instead of returning the truncated bytes as a complete
  file. `read_file_to` is unchanged and still returns the recovered byte
  count, so callers that want the partial data can take it deliberately
- Internal cleanup for the lints added in clippy 1.98

### Added

- `testutil::HfsPlusImageBuilder::add_file_with_declared_size` builds a volume
  whose fork claims more bytes than it allocates, for testing short reads

## [0.2.4] - 2026-04-12

### Changed

- Include `LICENSE` file in the published crate

## [0.2.3] - 2026-02-21

### Fixed

- Use checked arithmetic and fallible indexing in B-tree and catalog parsing to prevent panics on malformed images

## [0.2.2] - 2026-02-16

### Added

- `testutil` feature with `HfsPlusImageBuilder` for generating synthetic HFSX images in memory
- Synthetic image tests exercising volume header parsing, directory listing, file reading, walk, stat, and streaming I/O — all without external fixtures

### Changed

- Rust edition upgraded from 2021 to 2024
- Renamed test fixture `kdk.raw` → `hfsp.raw`

## [0.2.1] - 2026-02-16

### Fixed

- Clippy warnings: `derivable_impls`, `manual_is_multiple_of`, `ptr_arg`, `empty_line_after_doc_comments`
- Rustdoc warnings: escaped `Vec<u16>` in doc comments

## [0.2.0] - 2026-02-11

### Changed

- Fixture-dependent tests now use `#[ignore]` instead of silent path-exists guards

## [0.1.0] - 2026-02-10

### Added

- HFS+ and HFSX (case-sensitive) volume parsing
- B-tree catalog traversal
- Extent overflow file handling
- `ForkReader` with `Read + Seek` streaming I/O
- Unicode filename decomposition
- Directory listing, file reading, recursive walk
- Path resolution (Unix-style paths)
- File metadata (BSD permissions, timestamps, fork info)
