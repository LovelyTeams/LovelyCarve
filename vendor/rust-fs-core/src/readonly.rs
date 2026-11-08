//! Read-only safety wrapper.
//!
//! [`ReadOnlyDevice`] wraps any `BlockRead` and presents it as a
//! [`BlockDevice`] whose write path is unconditionally
//! [`crate::Error::ReadOnly`] and whose `is_writable()` is always `false` —
//! regardless of what the underlying device supports.
//!
//! Useful when the caller wants type-level certainty that no writes can
//! land on a particular device, even if the underlying type *could*
//! accept them. Examples: "snapshot view" of a writable image, an
//! inspection tool that must never mutate, a slice handed across an FFI
//! boundary that the consumer should not be able to write through.

use crate::block::{BlockDevice, BlockRead};
use crate::error::Result;

/// Wraps any `T: BlockRead` and makes it read-only at the type level.
pub struct ReadOnlyDevice<T> {
    inner: T,
}

impl<T> ReadOnlyDevice<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    /// Borrow the wrapped device.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Consume the wrapper and return the inner device unchanged.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: BlockRead> BlockRead for ReadOnlyDevice<T> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.inner.read_at(offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.inner.size_bytes()
    }
}

/// `BlockDevice` impl uses the trait's default (`Err(ReadOnly)` for
/// `write_at`, no-op `flush`, `is_writable() -> false`). Even if `T`
/// implements `BlockDevice` with full writes, the wrapper hides that.
impl<T: BlockRead> BlockDevice for ReadOnlyDevice<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::test_device::RwBytes as WritableBytes;
    use std::sync::Mutex;

    #[test]
    fn read_through_works() {
        let mut v = vec![0u8; 16];
        v[4..8].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        let wrapped = ReadOnlyDevice::new(WritableBytes(Mutex::new(v)));

        let mut buf = [0u8; 4];
        wrapped.read_at(4, &mut buf).unwrap();
        assert_eq!(buf, [0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn writes_rejected_even_though_inner_is_writable() {
        let wrapped = ReadOnlyDevice::new(WritableBytes(Mutex::new(vec![0u8; 16])));
        assert!(!BlockDevice::is_writable(&wrapped));

        match BlockDevice::write_at(&wrapped, 0, &[0xFFu8; 4]) {
            Err(Error::ReadOnly) => {}
            other => panic!("expected ReadOnly, got {other:?}"),
        }
    }

    #[test]
    fn into_inner_returns_unchanged() {
        let inner = WritableBytes(Mutex::new(vec![0u8; 8]));
        let wrapped = ReadOnlyDevice::new(inner);
        let back = wrapped.into_inner();
        // Inner should still be writable when accessed directly.
        assert!(BlockDevice::is_writable(&back));
        // The recovered inner still accepts a direct write + read-back.
        back.write_at(0, &[0x42; 4]).unwrap();
        let mut buf = [0u8; 4];
        back.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0x42; 4]);
    }

    #[test]
    fn inner_borrows_without_consuming() {
        let mut v = vec![0u8; 8];
        v[0..2].copy_from_slice(&[0x9A, 0xBC]);
        let wrapped = ReadOnlyDevice::new(WritableBytes(Mutex::new(v)));

        // `inner()` exposes the underlying device by reference; the inner
        // type's own (writable) behaviour is visible through it.
        assert!(BlockDevice::is_writable(wrapped.inner()));
        assert_eq!(wrapped.inner().size_bytes(), 8);

        // Wrapper still usable after borrowing the inner.
        let mut buf = [0u8; 2];
        wrapped.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0x9A, 0xBC]);
    }
}
