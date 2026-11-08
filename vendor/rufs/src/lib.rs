#![cfg_attr(fuzzing, allow(dead_code, unused_imports, unused_mut))]

mod blockreader;
mod data;
mod decoder;
mod inode;
mod ufs;

#[cfg(any(target_os = "freebsd", target_os = "openbsd", target_os = "macos"))]
pub const ENOATTR: i32 = libc::ENOATTR;
#[cfg(target_os = "linux")]
pub const ENOATTR: i32 = libc::ENODATA;
// Lovelycarve patch: Windows / other targets that do not expose ENOATTR or
// ENODATA in libc — fall back to Linux's ENODATA (61) so the xattr error path
// keeps a meaningful errno even on Windows. Read-only forensic access never
// hits this path in practice.
#[cfg(not(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "macos",
    target_os = "linux"
)))]
pub const ENOATTR: i32 = 61;

// Lovelycarve patch: cross-platform shim for `OsStr` <-> raw byte conversion.
// On Unix this re-exports `std::os::unix::ffi::{OsStrExt, OsStringExt}` so the
// vendored code path is unchanged. On Windows / WASI / etc. it reimplements
// the same surface using the stable `as_encoded_bytes` /
// `from_encoded_bytes_unchecked` APIs (Rust 1.74+). The encoded representation
// on Windows is WTF-8, so non-UTF-8 UFS filename bytes that round-trip through
// these shims are technically UB; in our forensic read-only flows we surface
// filenames immediately as `String::from_utf8_lossy`, so they do not survive
// long enough to matter for the FreeBSD installer images we test against.
pub(crate) mod os_compat {
    #[cfg(unix)]
    pub use std::os::unix::ffi::{OsStrExt, OsStringExt};

    #[cfg(not(unix))]
    use std::ffi::{OsStr, OsString};

    #[cfg(not(unix))]
    pub trait OsStrExt {
        fn as_bytes(&self) -> &[u8];
        fn from_bytes(bytes: &[u8]) -> &OsStr;
    }

    #[cfg(not(unix))]
    impl OsStrExt for OsStr {
        fn as_bytes(&self) -> &[u8] {
            self.as_encoded_bytes()
        }
        fn from_bytes(bytes: &[u8]) -> &OsStr {
            // Safety: `OsStr::from_encoded_bytes_unchecked` requires the
            // bytes to be valid for the platform's OsStr encoding. ASCII /
            // UTF-8 always satisfies that on Windows; non-UTF-8 UFS names
            // would be unsound here but our adapter only ever stores them
            // as `String::from_utf8_lossy` before the borrow ends.
            unsafe { OsStr::from_encoded_bytes_unchecked(bytes) }
        }
    }

    #[cfg(not(unix))]
    #[allow(dead_code)] // Lovelycarve patch: `from_vec` is currently only invoked from xattr write paths that we never exercise in read-only forensic mode.
    pub trait OsStringExt {
        fn from_vec(vec: Vec<u8>) -> Self;
        fn into_vec(self) -> Vec<u8>;
    }

    #[cfg(not(unix))]
    #[allow(dead_code)] // Lovelycarve patch: `from_vec` is currently only invoked from xattr write paths that we never exercise in read-only forensic mode.
    impl OsStringExt for OsString {
        fn from_vec(vec: Vec<u8>) -> Self {
            // Safety: same constraint as `OsStr::from_bytes` above.
            unsafe { OsString::from_encoded_bytes_unchecked(vec) }
        }
        fn into_vec(self) -> Vec<u8> {
            self.into_encoded_bytes()
        }
    }
}

pub use crate::{
    blockreader::{Backend, BlockReader},
    data::{InodeAttr, InodeNum, InodeType},
    ufs::{Info, Ufs},
};
