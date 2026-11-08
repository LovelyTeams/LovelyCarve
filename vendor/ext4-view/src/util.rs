// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use core::mem::size_of;

/// Convert a `u32` to a `usize`.
///
/// Rust allows `usize` to be as small as `u16`, but on platforms
/// supported by this crate, this conversion is infallible.
///
/// # Panics
///
/// Panics if `val` does not fit in this platform's `usize`.
#[inline]
#[must_use]
pub(crate) const fn usize_from_u32(val: u32) -> usize {
    assert!(size_of::<usize>() >= size_of::<u32>());

    // Cannot use `usize::try_from` in a `const fn`.
    #[expect(clippy::as_conversions)]
    {
        val as usize
    }
}

/// Create a `u64` from two `u32` values.
#[inline]
#[must_use]
pub(crate) fn u64_from_hilo(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

/// Create a `u32` from two `u16` values.
#[inline]
#[must_use]
pub(crate) fn u32_from_hilo(hi: u16, lo: u16) -> u32 {
    (u32::from(hi) << 16) | u32::from(lo)
}

/// Read a little-endian [`u16`] from `bytes` at `offset`.
///
/// # Panics
///
/// Panics if `bytes` is not large enough to read two bytes at `offset`.
#[inline]
#[must_use]
#[track_caller]
pub(crate) fn read_u16le(bytes: &[u8], offset: usize) -> u16 {
    // OK to unwrap: these panics are described in the docstring.
    let end = offset.checked_add(size_of::<u16>()).unwrap();
    let bytes = bytes.get(offset..end).unwrap();
    u16::from_le_bytes(bytes.try_into().unwrap())
}

/// Read a little-endian [`u32`] from `bytes` at `offset`.
///
/// # Panics
///
/// Panics if `bytes` is not large enough to read four bytes at `offset`.
#[inline]
#[must_use]
#[track_caller]
pub(crate) fn read_u32le(bytes: &[u8], offset: usize) -> u32 {
    // OK to unwrap: these panics are described in the docstring.
    let end = offset.checked_add(size_of::<u32>()).unwrap();
    let bytes = bytes.get(offset..end).unwrap();
    u32::from_le_bytes(bytes.try_into().unwrap())
}

/// Read a big-endian [`u32`] from `bytes` at `offset`.
///
/// # Panics
///
/// Panics if `bytes` is not large enough to read four bytes at `offset`.
#[inline]
#[must_use]
#[track_caller]
pub(crate) fn read_u32be(bytes: &[u8], offset: usize) -> u32 {
    // OK to unwrap: these panics are described in the docstring.
    let end = offset.checked_add(size_of::<u32>()).unwrap();
    let bytes = bytes.get(offset..end).unwrap();
    u32::from_be_bytes(bytes.try_into().unwrap())
}
