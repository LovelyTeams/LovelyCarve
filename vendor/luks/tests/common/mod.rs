//! Shared helpers for env-gated oracle tests.

#![allow(dead_code)]

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
