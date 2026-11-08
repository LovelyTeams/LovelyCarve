# bcachefs evidence adapter

Original AGPL-3.0-only Rust adapter over the MIT
[rust-fs-bcachefs](../rust-fs-bcachefs/ORIGIN.md) and
[rust-fs-core](../rust-fs-core/ORIGIN.md) libraries. All production parsing,
checksumming, decompression and supported decryption are Rust. The evidence
source has no write interface; native tools exist only in a disposable oracle.

## Research before adaptation

- `rust-fs-bcachefs` 0.4.2, commit
  `53eb12044f3f6dab498c11c2044748f4591cb4e5`: complete reusable immutable
  Rust reader, MIT. Selected and pinned rather than rewritten.
- `unearth` 0.4.0, commit
  `bc891261221f5f5134269780d6c206725b21d52e`, `src/bcachefs.rs`: detection
  only, no file traversal; not copied or linked for this format.
- The official bcachefs tools' Rust commands wrap their native C engine;
  they are not a pure Rust reader and are not a production dependency.
- Prose: <https://bcachefs.org/bcachefs-principles-of-operation.pdf>, revision
  `1.39.7+efec39c`; codec and checksum sources remain listed in the MIT
  reader's `docs/clean-room.md`. No native implementation source was consulted.

## Adapter semantics

Opaque handles include snapshot identity; displayed inode numbers are the
native numbers, not those handles. `lamfold::FoldFrontend::inode_number`
defaults to the previous identity behavior for existing readers. Directory
ancestry and caches continue to use opaque identity. Names and symlink bytes
are lossless; unsupported non-UTF-8 names fail explicitly. Source-read errors
are latched through any upstream recovery fallback and returned to the host.
Timestamp roles and signed nanoseconds are preserved; unobserved high-word
timestamp encodings are refused rather than truncated.

## Independent fixtures

`scripts/bcachefs-fixtures.py` and its invented image are CC0-1.0. Python
packs explicit fields and CRC32C independently of the Rust writer. The image
has same-inode native subvolume/snapshot roots with different bytes, hard
links, inline and checked extents, reflinks, unwritten allocation, a >5 GiB
sparse file, symlinks, a FIFO, negative/nanosecond times and a spare superblock.
Its full length/hash are in `tests/fixtures/manifest.json`.
An additional virtual 8 GiB source relocates checked allocated data to 6 GiB,
validates its physical pointer and allocator entry, and measures bounded I/O
for a 14-byte window without allocating an 8 GiB image.
Corruption tests re-sign CRCs before exercising stale allocation and snapshot
cycles; source failures remain errors despite a readable spare superblock.

## Native oracle

`scripts/bcachefs-native-fixtures.py` creates new image files only in a
disposable Linux environment, using official bcachefs-tools 1.39.7. The
binary SHA-256 is
`a1390a469c5a28c50b5433866f92e285e9014c35c9a026cfaafd57d61081b4f8`.
Debian's repositories and the official repository were authenticated through
APT; the latter signing-key fingerprint was checked against its published
`EA483B991020C72A8A5035ADA0620B5E0E01C1DD`.

Native manifests record every name, type, mode, inode, size, timestamp,
regular-file SHA-256 and symlink target. The native fsck runs before/after
read-only inspection; image hashes must stay unchanged. Native images remain
external under `BCACHEFS_ORACLE_DIR`, never committed. Each is 512 MiB:

| Image | SHA-256 |
| --- | --- |
| 512-none | `ccf970d3280bd8c65245a765bf7020f38a87f8a63487b27d6fe5d6469877639e` |
| 4096-lz4 | `aca5717ec9b574d681eeca1b9af1efdcb3de931232d0464b2fe89a284a925733` |
| 4096-zstd | `3734ae84c8a02579aa3a94190110e608a4b2c8d1e55fe7b2c076f6cbd246add0` |
| 4096-gzip | `68cc3aa6464ffa62ae6d84d8aff5e70cb9198e5fb450bd7592081c921667dc5c` |

All four passed the Rust native audit (1514 entries each), including all
regular-file hashes and ranged reads. The first three manifests were observed
through the official FUSE reader. That reader crashed on gzip data, so gzip
was independently inspected through the official bcachefs 1.39.7 kernel module
on Debian kernel `7.2.9+deb13-amd64`, mounted with `loop,ro,nochanges`.
The external tool's extent dump confirms gzip-compressed extents. Neither
the module nor the native tools are a production dependency.
Creation time is an on-disk `otime` value, not an independently observed FUSE
birth time. Native snapshot/reflink/unwritten cases and 64 KiB blocks have not
yet been independently mounted here; their invented tests are distinct from
the native controls. Actual Windows Dokan mounting remains unverified.
