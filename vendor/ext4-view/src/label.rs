// Copyright 2025 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::format::{BytesDisplay, format_bytes_debug};
use core::fmt::{self, Debug, Formatter};
use core::str::Utf8Error;

/// Filesystem label.
///
/// The label is at most 16 bytes, and may contain null bytes. The
/// encoding is not specified.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Label([u8; 16]);

impl Label {
    /// Create a label from raw bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Convert the label to a UTF-8 string, if possible.
    ///
    /// The first null byte, and any following bytes, are excluded from
    /// the conversion.
    pub fn to_str(&self) -> Result<&str, Utf8Error> {
        core::str::from_utf8(self.as_bytes_up_to_first_null())
    }

    /// Get the raw bytes of the label. This may include null bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Get the bytes up to the first null, or all of the bytes if there
    /// is no null byte.
    #[must_use]
    fn as_bytes_up_to_first_null(&self) -> &[u8] {
        if let Some(index) = self.0.iter().position(|c| *c == 0) {
            &self.0[..index]
        } else {
            &self.0
        }
    }

    /// Get an object that implements [`Display`] to allow conveniently
    /// printing labels that may or may not be valid UTF-8. Non-UTF-8
    /// characters will be replaced with '�'.
    ///
    /// Null bytes are not included.
    ///
    /// [`Display`]: core::fmt::Display
    pub fn display(&self) -> BytesDisplay<'_> {
        BytesDisplay(self.as_bytes_up_to_first_null())
    }
}

impl Debug for Label {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        format_bytes_debug(self.as_bytes_up_to_first_null(), f)
    }
}
