# lamfold-romfs

[![Crates.io](https://img.shields.io/crates/v/lamfold-romfs.svg)](https://crates.io/crates/lamfold-romfs)
[![Docs.rs](https://docs.rs/lamfold-romfs/badge.svg)](https://docs.rs/lamfold-romfs)
[![CI](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml/badge.svg)](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml)
[![Website](https://img.shields.io/badge/website-lamco.ai-blue.svg)](https://lamco.ai/products/lamboot/)

> The romfs frontend of the [`lamfold`](../lamfold) read-only media stack:
> Clean-room Linux romfs (`-rom1fs-`).

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`. romfs
is the minimal uncompressed read-only filesystem for tiny embedded and
early-boot images. Spec: `the lamfold design spec` §4.

- **Website:** [LamBoot](https://lamco.ai/products/lamboot/), the bootloader lamfold is written for
- **Documentation:** [docs.rs/lamfold-romfs](https://docs.rs/lamfold-romfs) and the [LamBoot docs](https://lamco.ai/products/lamboot/docs/)
- **Repository:** [github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold)
- **Company:** [Lamco Development LLC](https://lamco.ai/)

## What it reads

| Layer | State |
| ----- | ----- |
| Header (`-rom1fs-`, big-endian size, volume name) and 16-byte aligned file headers | Read |
| Directory chains, regular files, symlinks via `read_link` | Read |
| **Hard links** (type 0, whose `info` names the target's header) | Read as the target (0.1.1) |

Verified against real `genromfs` images. Builds `no_std`;
`#![forbid(unsafe_code)]`.

## Installation

```sh
cargo add lamfold-romfs
```

## Clean-room posture

Derived only from the public romfs format description. The GPL-2 `fs/romfs`
driver is fenced off. **`neotron-romfs` is a different, incompatible format and
GPL-3**, deliberately not consulted.

## Build / test

```bash
cargo build
cargo test                                   # every fixture in tests/fixtures/
cargo build --no-default-features            # no_std check
```

## Contributing

Issues and pull requests are welcome at
[github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold). A
change to the read path should come with a fixture built by a real tool
(`genromfs`, as `tests/fixtures/regenerate-hardlinks.sh` does).

## License

MIT OR Apache-2.0, copyright Lamco Development LLC.
