# lamfold-erofs

[![Crates.io](https://img.shields.io/crates/v/lamfold-erofs.svg)](https://crates.io/crates/lamfold-erofs)
[![Docs.rs](https://docs.rs/lamfold-erofs/badge.svg)](https://docs.rs/lamfold-erofs)
[![CI](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml/badge.svg)](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml)
[![Website](https://img.shields.io/badge/website-lamco.ai-blue.svg)](https://lamco.ai/products/lamboot/)

> The EROFS frontend of the [`lamfold`](../lamfold) read-only media stack:
> Clean-room EROFS, compressed and uncompressed, **plus the shepherd**, the
> fs-verity integrity story.

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`.
EROFS is the forward standard for immutable images — the metadata format
**composefs** builds on for content-addressed OS (bootc/Silverblue) and OCI
images — so this is where the lamfold stack meets the shepherd: read the
metadata here, anchor trust in lamfold's native fs-verity Merkle. Spec:
`the lamfold design spec` §4 + §7.

- **Website:** [LamBoot](https://lamco.ai/products/lamboot/), the bootloader lamfold is written for
- **Documentation:** [docs.rs/lamfold-erofs](https://docs.rs/lamfold-erofs) and the [LamBoot docs](https://lamco.ai/products/lamboot/docs/)
- **Repository:** [github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold)
- **Company:** [Lamco Development LLC](https://lamco.ai/)

## What it reads

| Layer | State |
| ----- | ----- |
| Superblock, compact (32 B) and extended (64 B) inodes, directories, symlinks | Read |
| `FLAT_PLAIN` and `FLAT_INLINE` data | Read |
| Compressed files, full (legacy) and compact indexes, big pclusters | Read, with lz4, lzma (custom lc/lp/pb too), deflate and zstd behind their features |
| Block sizes from 512 bytes to 4 KiB | Read |
| Chunk-based files (`mkfs.erofs --chunksize`) with a block map, holes as zeros | Read (0.1.3) |
| Compression configs after superblock extension slots | Read (0.1.3) |
| The shepherd: `MerkleVerifier` gates every data block read; tampered media fails with `VerifyFailed` | Done |
| Fragments and all-fragments (Fedora's live images, lorax), compressed directories (`--zD`), the 48-bit layout, metabox, extra devices, unknown incompat features | Refused at open with `Unsupported` |
| ztailpacking, interlaced pclusters, 8-byte chunk indexes (mkfs writes them only for a blob device), Linux 7.2's hole pclusters (no released erofs-utils writes them yet) | Refused per file with `Unsupported` |

Nothing on the refused list is misread: An image that uses one of them fails to
open, or the file that uses it fails to read, with a typed error.

Tested against images from erofs-utils 1.8.6 and 1.9.4: Every option above
over a mixed tree, a tree of real executables (which exposed a silent LZ4 misread
in 0.1.1), files whose inline xattrs put the index on a 32-byte boundary, and
smaller blocks. The committed fixtures and the script that builds them are in
`tests/fixtures/`.

## Installation

```sh
cargo add lamfold-erofs
```

The `lz4` codec is on by default; enable `deflate`, `zstd` and `lzma` as needed.

## The shepherd (integrity)

The `verity` feature (on by default) pulls `lamfold/verify`, the substrate's
native fs-verity SHA-256 Merkle (RustCrypto `sha2`). `lamfold::fsverity_digest_sha256`
reproduces the exact measurement of the `fsverity` userspace tool — the digest
composefs/bootc sign — and `lamfold::MerkleVerifier` implements the substrate
`Verifier` seam so any frontend's reads can be trust-gated. Put one in the
`SubstrateCtx` and tampered media fails the read; use `NoVerifier` for unverified
media.

## Clean-room posture

Derived only from the public EROFS format docs. The on-disk header
`fs/erofs/erofs_fs.h` is itself **SPDX MIT**, so its struct layout is referenced
directly; the GPL-2 EROFS *driver* (`fs/erofs/*.c`) is fenced off — never read
or copied.

## Build / test

```bash
cargo build
cargo test --features deflate,zstd,lzma      # every fixture in tests/fixtures/
cargo build --no-default-features            # no_std check
```

## Contributing

Issues and pull requests are welcome at
[github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold). A
change to the read path should come with a fixture built by
`tests/fixtures/regenerate.sh`.

## License

MIT OR Apache-2.0, copyright Lamco Development LLC.
