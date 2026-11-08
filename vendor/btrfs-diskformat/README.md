# BTRFS Disk Format

[![Crates.io](https://img.shields.io/crates/v/btrfs-diskformat.svg)](https://crates.io/crates/btrfs-diskformat)
[![Crates.io](https://img.shields.io/badge/license-BSD%202--Clause-blue)](https://crates.io/crates/btrfs-diskformat)

Clean-room implementation of the [btrfs] disk format in Rust.

[Documentation](https://docs.rs/btrfs-diskformat/)

## License

`btrfs-diskformat` is distributed under the terms of the BSD 2-Clause license.

See the [LICENSE-BSD](LICENSE-BSD) file in this repository for more information.

## Cargo Features

- `alloc`: Enables allocation and the `alloc` feature in `zerocopy`.
- `std`: By default, the crate is `no_std`. This enables `std` features from
  the `zerocopy` and `strum` dependencies.

## Contributing

Because this codebase is developed without knowledge of the Linux btrfs source code and is released under a more permissive license(s) than GPLv2, development is heavily dependent on information released on the [btrfs wiki] and reverse engineering the effects of operations made by `btrfs-progs` and other utilities. As a result, contributions to this codebase must strictly follow the same siloed approach.

[btrfs]: https://btrfs.wiki.kernel.org/index.php/Main_Page
[btrfs wiki]: https://btrfs.wiki.kernel.org/index.php/Main_Page
