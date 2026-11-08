# ext4-view-rs

[![Crates.io](https://img.shields.io/crates/v/ext4-view)](https://crates.io/crates/ext4-view) 
[![Docs.rs](https://docs.rs/ext4-view/badge.svg)](https://docs.rs/ext4-view)
[![codecov.io](https://codecov.io/gh/nicholasbishop/ext4-view-rs/coverage.svg?branch=main)](https://app.codecov.io/gh/nicholasbishop/ext4-view-rs)

This repository provides a Rust crate that allows read-only access to an
[ext4] filesystem. It also works with [ext2] and [ext3] filesystems.
Write access is an explicit non-goal. The crate is `no_std`, so it can
be used in embedded contexts. However, it does require `alloc`.

[ext2]: https://en.wikipedia.org/wiki/Ext2
[ext3]: https://en.wikipedia.org/wiki/Ext3
[ext4]: https://en.wikipedia.org/wiki/Ext4

## Usage

Add the dependency:
```console
cargo add ext4-view
```

Basic example:
```rust
use ext4_view::{Ext4, Metadata};

// Load the filesystem. The data source can be be anything that
// implements the `Ext4Read` trait. The simplest source is a
// `Vec<u8>` containing the whole filesystem.
let fs_data: Vec<u8> = get_fs_data_from_somewhere();
let fs = Ext4::load(Box::new(fs_data))?;

// If the `std` feature is enabled, you can load a filesystem by path:
let fs = Ext4::load_from_path(std::path::Path::new("some-fs.bin"))?;

// The `Ext4` type has methods very similar to `std::fs`:
let path = "/some/file/path";
let file_data: Vec<u8> = fs.read(path)?;
let file_str: String = fs.read_to_string(path)?;
let exists: bool = fs.exists(path)?;
let metadata: Metadata = fs.metadata(path)?;
for entry in fs.read_dir("/some/dir")? {
    let entry = entry?;
    println!("{}", entry.path().display());
}
```

See [`examples/cat.rs`](examples/cat.rs) for an example of using the
`File` interface to read a file in chunks.

## Design Goals

In order of importance:

1. Correct
   * All valid ext2/ext3/ext4 filesystems should be readable.
   * Invalid data should never cause crashes, panics, or non-terminating loops.
   * No `unsafe` code in the main package (it is allowed in dependencies).
   * Well tested.
2. Easy to use
   * The API should follow the conventions of [`std::fs`] where possible.
3. Good performance
   * Performance should not come at the expense of correctness or ease of use.

Non-goals:
* Write support.
* Recovery of corrupt filesystems.

[`std::fs`]: https://doc.rust-lang.org/std/fs/index.html

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE)
or [MIT license](LICENSE-MIT) at your option.

## Contributing

See the [code of conduct] and [contributing.md].

Bug reports and PRs are welcome!

[code of conduct]: docs/code-of-conduct.md
[contributing.md]: docs/contributing.md

## Disclaimer

This project is not an official Google project. It is not supported by
Google and Google specifically disclaims all warranties as to its quality,
merchantability, or fitness for a particular purpose.
