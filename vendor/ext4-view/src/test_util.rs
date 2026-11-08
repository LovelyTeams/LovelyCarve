// Copyright 2025 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// In addition to being used as a regular module in lib.rs, this module
// is used in `tests` via the `include!` macro.

use super::Ext4;

/// Decompress a file into memory with zstd.
pub(crate) fn load_compressed_data(name: &str) -> Vec<u8> {
    // This operation executes quickly, so don't bother caching.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test_data")
        .join(name);
    let file = std::fs::File::open(path).unwrap();
    zstd::stream::decode_all(file).unwrap()
}

/// Decompress a file with zstd, then load it into an `Ext4`.
pub(crate) fn load_compressed_filesystem(name: &str) -> Ext4 {
    let bytes = load_compressed_data(name);
    Ext4::load(Box::new(bytes)).unwrap()
}

pub(crate) fn load_test_disk1() -> Ext4 {
    load_compressed_filesystem("test_disk1.bin.zst")
}
