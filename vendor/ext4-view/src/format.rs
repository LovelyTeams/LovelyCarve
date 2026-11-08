// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use alloc::string::String;
use core::fmt::{self, Display, Formatter};
use core::str;

/// Format `bytes` in a way that is suitable for `Debug` implementations.
///
/// This is used for string-like data (like a UNIX path) that isn't
/// required to be ASCII, UTF-8, or any other particular encoding, but
/// in practice it often is valid UTF-8.
pub(crate) fn format_bytes_debug(bytes: &[u8], f: &mut Formatter<'_>) -> fmt::Result {
    if let Ok(s) = str::from_utf8(bytes) {
        // For valid UTF-8, print it unmodified except for escaping
        // special characters like newlines.
        write!(f, "\"{}\"", s.escape_debug())
    } else {
        // Otherwise, print valid ASCII characters (again, with special
        // characters like newlines escaped). Non-ASCII bytes are
        // printed in "\xHH" format.
        write!(f, "\"{}\"", bytes.escape_ascii())
    }
}

/// Helper for formatting string-like data.
///
/// The data is lossily converted to UTF-8, with invalid UTF-8 sequences
/// converted to '�'.
#[must_use]
pub struct BytesDisplay<'a>(pub(crate) &'a [u8]);

impl Display for BytesDisplay<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Debug;

    struct S<'a>(&'a [u8]);

    impl<'a> Debug for S<'a> {
        fn fmt(&self, f: &mut Formatter) -> fmt::Result {
            format_bytes_debug(self.0, f)
        }
    }

    #[test]
    fn test_format_bytes_debug() {
        let f = |b: &[u8]| format!("{:?}", S(b));

        // Valid UTF-8.
        assert_eq!(f("abc😁".as_bytes()), r#""abc😁""#);
        assert_eq!(f("abc\n".as_bytes()), r#""abc\n""#);

        // Invalid UTF-8.
        assert_eq!(f(&[0xc3, 0x28]), r#""\xc3(""#);
        assert_eq!(f(&[0xc3, 0x28, b'\n']), r#""\xc3(\n""#);
    }

    #[test]
    fn test_bytes_display() {
        let f = |b: &[u8]| format!("{}", BytesDisplay(b));

        // Valid UTF-8.
        assert_eq!(f("abc😁".as_bytes()), "abc😁");
        assert_eq!(f("abc\n".as_bytes()), "abc\n");

        // Invalid UTF-8.
        assert_eq!(f(&[0xc3, 0x28]), "�(");
    }
}
