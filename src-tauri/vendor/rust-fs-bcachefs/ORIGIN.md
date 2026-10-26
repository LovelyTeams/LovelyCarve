# rust-fs-bcachefs

- Upstream: <https://github.com/antimatter-studios/rust-fs-bcachefs>.
- Version 0.4.2, fixed commit `53eb12044f3f6dab498c11c2044748f4591cb4e5`.
- MIT; upstream `LICENSE`, `THIRD-PARTY-LICENSES.md`, documentation and the
  byte fixtures used by its unit tests are retained.
- Upstream learns the disk format from prose and black-box native-tool
  observations. Its full provenance record remains in `docs/clean-room.md`.
  LovelyCarve consulted that Rust code and the public prose specification,
  not the native C implementation.

## Local changes

- Standalone Cargo workspace, `rlib` only, no automatic binaries/examples;
  upstream integration tests, build scripts and CI orchestration are not
  vendored. The 69 source-local read/check/crypto tests remain runnable.
- C ABI moved behind an opt-in `capi` feature; the application enables neither
  it nor `write` / `cli`. Evidence implements only `BlockRead`.
- Public component lookup and directory-entry conversion preserve `Node`'s
  `(inode, snapshot)` instead of assuming the root subvolume.
- Validate snapshot parent cycles / missing parents, bound snapshot and
  per-inode key counts, and bound the node cache by both count and bytes.
- Range reads of unchecked and unwritten extents allocate/read the requested
  window, including shared reflink extents. Check physical spans against the
  member's declared geometry, reject logical size underflow, and cap one
  checked/decompressed region at 64 MiB before allocation.
- Three `chunks_exact` loops use equivalent `as_chunks` on the repository's
  Rust toolchain; SipHash's published vector remains a regression.

The application adapter, independent invented fixtures, native oracle and
their verification boundaries are recorded in
[bcachefs-reader/ORIGIN.md](../bcachefs-reader/ORIGIN.md) and
[BCACHEFS.md](../../../docs/formats/BCACHEFS.md).
