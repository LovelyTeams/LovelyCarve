// Copyright 2025 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

/// Absolute block index within the filesystem.
pub(crate) type FsBlockIndex = u64;

/// Block index relative to the start of a file.
pub(crate) type FileBlockIndex = u32;
