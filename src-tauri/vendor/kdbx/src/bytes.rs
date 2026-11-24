//! Bounds-checked little-endian reads over a byte slice.

use crate::error::{Error, Result};

pub(crate) struct Cursor<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    pub(crate) fn remaining(&self) -> &'a [u8] {
        &self.data[self.position..]
    }

    /// The next `length` bytes; `what` names the structure for the error.
    pub(crate) fn take(&mut self, length: usize, what: &'static str) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .filter(|&end| end <= self.data.len())
            .ok_or(Error::Truncated(what))?;
        let bytes = &self.data[self.position..end];
        self.position = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N]> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N, what)?);
        Ok(out)
    }

    pub(crate) fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.array::<1>(what)?[0])
    }

    pub(crate) fn u16(&mut self, what: &'static str) -> Result<u16> {
        self.array(what).map(u16::from_le_bytes)
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32> {
        self.array(what).map(u32::from_le_bytes)
    }

    pub(crate) fn i32(&mut self, what: &'static str) -> Result<i32> {
        self.array(what).map(i32::from_le_bytes)
    }

    pub(crate) fn bytes32(&mut self, what: &'static str) -> Result<[u8; 32]> {
        self.array(what)
    }

    /// A length stored as a signed 32-bit integer, as KeePass writes it;
    /// negative lengths are malformed.
    pub(crate) fn length_i32(&mut self, what: &'static str) -> Result<usize> {
        let length = self.i32(what)?;
        usize::try_from(length).map_err(|_| Error::Malformed(format!("{what}: negative length")))
    }
}

/// `data` as a fixed-size array, or `None` when its length differs.
pub(crate) fn fixed<const N: usize>(data: &[u8]) -> Option<[u8; N]> {
    data.try_into().ok()
}

pub(crate) fn hex(data: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(data.len() * 2);
    for byte in data {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}
