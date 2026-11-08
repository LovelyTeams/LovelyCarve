//! Shared helpers for the env-gated Tier-1/2 oracle tests: a windowed reader
//! (BitLocker volumes sit at a partition offset inside the raw image) and a
//! SHA-256 hex helper.

#![allow(dead_code)]

use std::io::{self, Read, Seek, SeekFrom};

use sha2::{Digest, Sha256};

/// A `Read + Seek` window over an inner reader starting at `base`, presenting
/// the partition's byte offset 0 at file offset `base`.
pub struct OffsetReader<R> {
    inner: R,
    base: u64,
    pos: u64,
}

impl<R: Read + Seek> OffsetReader<R> {
    pub fn new(mut inner: R, base: u64) -> io::Result<Self> {
        inner.seek(SeekFrom::Start(base))?;
        Ok(OffsetReader {
            inner,
            base,
            pos: 0,
        })
    }
}

impl<R: Read + Seek> Read for OffsetReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for OffsetReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(o) => {
                self.inner.seek(SeekFrom::Start(self.base + o))?;
                self.pos = o;
            }
            SeekFrom::Current(o) => {
                let new = self.pos as i64 + o;
                self.inner.seek(SeekFrom::Start(self.base + new as u64))?;
                self.pos = new as u64;
            }
            SeekFrom::End(o) => {
                let abs = self.inner.seek(SeekFrom::End(o))?;
                self.pos = abs.saturating_sub(self.base);
            }
        }
        Ok(self.pos)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
