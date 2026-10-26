//! One filesystem-relative device window for reads, preflight and writes.

use std::ffi::OsString;
use std::sync::Arc;

use fs_core::cli::CliError;
use fs_core::{BlockRead, FileDevice, OwnedRwSlice};

pub type Device = OwnedRwSlice;

pub struct Target<'a> {
    path: &'a OsString,
    offset: u64,
}

impl<'a> Target<'a> {
    pub fn new(path: &'a OsString, offset: u64) -> Self {
        Self { path, offset }
    }

    pub fn open(&self) -> Result<Device, CliError> {
        self.window(false)
    }

    pub fn open_rw(&self) -> Result<Device, CliError> {
        self.window(true)
    }

    fn window(&self, writable: bool) -> Result<Device, CliError> {
        let dev = if writable {
            FileDevice::open_rw(self.path)
        } else {
            FileDevice::open(self.path)
        }
        .map_err(|e| CliError::failed(e.to_string()))?;
        let size = dev.size_bytes();
        if self.offset != 0 && self.offset >= size {
            return Err(CliError::failed(format!(
                "--offset {} is past the end of {} ({size} bytes)",
                self.offset,
                std::path::Path::new(self.path).display()
            )));
        }
        // The read-only parent still refuses writes through this slice.
        // Subtraction follows the range check, so even u64::MAX cannot wrap.
        Ok(OwnedRwSlice::new(
            Arc::new(dev),
            self.offset,
            size - self.offset,
        ))
    }
}
