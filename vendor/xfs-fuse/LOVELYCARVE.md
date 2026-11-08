# LovelyCarve integration

xfuse source audited at KhaledEmaraDev/xfuse 2ea4e250db00d3f0f845499c8a83f85b9a77b7f8 (BSD-2-Clause).

Portable read-only fork with no FUSE server or Unix ABI. Includes upstream v1 inode link-count/attribute support and real-time flags/superblock parsing. Preserves explicit invalid-inode errors, portable FileAttr, per-volume source and guarded panic conversion. A real-time inode is rejected because the host API has no independent real-time device source; it must never read those addresses from the data device. The upstream service-level file cache and FUSE methods are not part of this fork.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).


2026-10-13: compared libyal/libfsxfs at cf474697de8a32f7eba4122e469b2ecd818054c6.
Local independently written Rust modules add checked geometry/open, versions 1–5,
legacy and CRC directory reads, extent plans, binary/namespaced/remote attributes
with payload extents, exact data/range reads, metadata UUID, signed nanosecond
and BIGTIME values, optional birth time and device identifiers. Bmap inode fork
pointers follow fork capacity instead of inherited alignment guesses. Directory
and data caches are bounded and volume-local. Raw path bytes remain lossless.

No libfsxfs C code or corpus is copied or linked. BSD-2-Clause remains the
vendored crate license. Synthetic test data is generated locally; see test_data.
Full 82-function mapping, host integration and unverified real-image/oracle limits:
[docs/formats/XFS.md](../../docs/formats/XFS.md).
