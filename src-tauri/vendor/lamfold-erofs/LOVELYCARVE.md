# LovelyCarve EROFS integration

Source: crates.io `lamfold-erofs 0.1.3`, VCS commit
`fabaad6af0c28b80535307b339b84110f3a3c12e`; MIT OR Apache-2.0.
Original licenses and NOTICE retained. The existing vendored lamfold substrate
is used by path; std, lz4, deflate, zstd and lzma are enabled, verity is not.

Local changes enforce declared volume bounds, exact allocated/pcluster source
ranges, inline-tail boundaries, superblock CRC32C, valid UTF-8/path components,
and explicit allocation/cache limits. Compressed files cache at most one decoded
file; UTC mtime retains its compact-superblock versus extended-inode origin.
The substrate adds a default optional timestamp-role method for this and F2FS;
existing CramFS/ROMFS readers continue to return no unsupported timestamp roles.

Upstream integration tests are built as library tests so `just test-erofs` runs
them. The upstream manually mutated `hole_full.erofs` fixture had an invalid
superblock CRC; the local fixture recalculates only that CRC so its intended
unsupported-hole behavior is still exercised after checksum validation.
Other fixture bytes are unchanged. Generator scripts and hashes are retained
under `tests/fixtures/`. Scope and limits: [EROFS](../../../docs/formats/EROFS.md).
