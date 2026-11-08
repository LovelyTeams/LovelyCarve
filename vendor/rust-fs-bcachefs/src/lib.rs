//! A pure-Rust bcachefs reader: read-only by default, with a small writer
//! behind the `write` feature.
//!
//! Written clean-room: the on-disk format was learned from prose
//! documentation and from black-box observation of images made by the
//! reference tools, never from their source. `docs/clean-room.md` lists
//! every source and every open question.
//!
//! bcachefs is little-endian on disk throughout.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod bkey;
pub mod btree;
#[cfg(feature = "capi")]
#[allow(non_camel_case_types)]
pub mod capi;
#[cfg(all(feature = "write", feature = "capi"))]
#[allow(non_camel_case_types)]
pub mod capi_write;
pub mod check;
pub mod compress;
pub mod crypt;
pub mod csum;
pub mod error;
pub mod extent;
pub mod fs;
pub mod inode;
pub mod journal;
pub mod siphash;
pub mod superblock;
pub(crate) mod util;
#[cfg(feature = "write")]
pub mod write;
pub mod xattr;

pub use error::{Error, Result};
pub use fs::Filesystem;

#[cfg(test)]
mod overflow_checks {
    /// Set by the debug unit tier (`chore test:unit`), and by nothing else.
    const HANDSHAKE: &str = "EXPECT_OVERFLOW_CHECKS";

    #[test]
    fn the_build_the_gate_asked_to_check_does_check() {
        if std::env::var_os(HANDSHAKE).is_none() {
            return;
        }
        let trapped = std::panic::catch_unwind(|| {
            let x: u8 = std::hint::black_box(255);
            std::hint::black_box(x + 1)
        })
        .is_err();
        assert!(
            trapped,
            "{HANDSHAKE} is set but this build wraps on overflow: the debug tier is not checking what it claims"
        );
    }
}
