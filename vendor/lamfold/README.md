# lamfold

[![Crates.io](https://img.shields.io/crates/v/lamfold.svg)](https://crates.io/crates/lamfold)
[![Docs.rs](https://docs.rs/lamfold/badge.svg)](https://docs.rs/lamfold)
[![CI](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml/badge.svg)](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml)
[![Website](https://img.shields.io/badge/website-lamco.ai-blue.svg)](https://lamco.ai/products/lamboot/)

> `no_std` read-only media filesystem **stack**: The substrate core.

- **Website:** [LamBoot](https://lamco.ai/products/lamboot/), the bootloader lamfold is written for
- **Documentation:** [docs.rs/lamfold](https://docs.rs/lamfold) and the [LamBoot docs](https://lamco.ai/products/lamboot/docs/)
- **Repository:** [github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold)
- **Company:** [Lamco Development LLC](https://lamco.ai/)

A layered, integrity-verifying, read-only filesystem stack: a shared substrate
under thin, clean-room format frontends (the *flock*). The name is a sheepfold —
the enclosure that holds the flock of formats — and the layered/recursive
structure it reads. Spec: `the lamfold design spec`.

**This crate is the substrate (L2).** Format frontends (`lamfold-iso`, `-udf`,
`-squash`, `-erofs`, …) live in their own crates and depend on this one.

## Status — substrate core: real, building, host-tested

| Component | State |
| --------- | ----- |
| `read_cap` — bounded-allocation hardening (no OOM on hostile sizes) | ✅ done + tested |
| `BlockCache` — immutable decompressed-block LRU | ✅ done + tested |
| `codec` registry — **deflate (raw+zlib), lz4 (block), zstd, xz, lzo** | ✅ all five wired + round-trip tested |
| `stream`: `decode_stream` for a whole stream of unknown size (gzip, zstd, xz, legacy `.lzma`, LZ4 legacy frames), capped, reporting the input it used; `StreamCodec::sniff` | ✅ 0.1.5, used by `lamfold-cpio` for compressed initramfs members |
| `BlockSource` / `SliceSource` — the byte source | ✅ done + tested |
| `FoldFrontend` trait — the flock contract | ✅ defined |
| `Verifier` (the shepherd) seam + `NoVerifier` | ✅ seam in; real fs-verity/composefs landed |

Verified: builds with default features **and** `--no-default-features` (genuine
`no_std`); 18 unit tests pass (all 5 codecs round-trip); `#![forbid(unsafe_code)]`.

## Codecs (all permissive `no_std`, opt-in per cargo feature)

`miniz_oxide` (deflate/zlib), `lz4_flex` (lz4 block), `ruzstd` (zstd), `lzma-rust2`
(xz/lzma), `lzokay` (lzo). Parse: `zerocopy`. See `the lamfold design spec` §6
for the pinned set — zero copyleft anywhere in the tree.

## Installation

```sh
cargo add lamfold
```

Frontends depend on it with a caret requirement, so a substrate patch release
reaches the published frontends without republishing them (`RELEASING.md`).

## Build

```bash
cargo build              # default: std (host) + deflate + lz4
cargo test               # host tests
cargo build --no-default-features --features codec-deflate,codec-lz4   # no_std check
```

The six-layer testing and fuzzing bar (`the lamfold design spec` §11) gates
every frontend before any "functional" claim.

## Contributing

Issues and pull requests are welcome at
[github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold).

## License

MIT OR Apache-2.0, copyright Lamco Development LLC.
