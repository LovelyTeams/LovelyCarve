//! `BlockDevice` backed by host-process-owned callbacks. Used when the
//! caller already holds the underlying resource (an FSKit
//! `FSBlockDeviceResource`, a Go file handle, a C-side fd) and the Rust
//! side just needs a trait surface to feed into a driver.

use crate::block::{BlockDevice, BlockRead};
use crate::error::{Error, Result};

pub type ReadCb = Box<dyn Fn(u64, &mut [u8]) -> std::io::Result<()> + Send + Sync>;
pub type WriteCb = Box<dyn Fn(u64, &[u8]) -> std::io::Result<()> + Send + Sync>;
pub type FlushCb = Box<dyn Fn() -> std::io::Result<()> + Send + Sync>;

pub struct CallbackDevice {
    pub size: u64,
    pub read: ReadCb,
    pub write: Option<WriteCb>,
    pub flush: Option<FlushCb>,
}

impl BlockRead for CallbackDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (self.read)(offset, buf)?;
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        self.size
    }
}

impl BlockDevice for CallbackDevice {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        match &self.write {
            Some(f) => {
                f(offset, buf)?;
                Ok(())
            }
            None => Err(Error::ReadOnly),
        }
    }
    fn flush(&self) -> Result<()> {
        match &self.flush {
            Some(f) => {
                f()?;
                Ok(())
            }
            None => Ok(()),
        }
    }
    fn is_writable(&self) -> bool {
        self.write.is_some()
    }
}
