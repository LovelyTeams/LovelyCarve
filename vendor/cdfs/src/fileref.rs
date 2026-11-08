// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::{
    cell::RefCell,
    io::{Read, Result, Seek, SeekFrom},
    rc::Rc,
};

use crate::BLOCK_SIZE;

/// A trait for objects which can be read by logical block addresses.
pub trait ISO9660Reader {
    /// Read the block(s) at a given LBA (logical block address)
    fn read_at(&mut self, buf: &mut [u8], lba: u64) -> Result<usize>;
}

impl<T: Read + Seek> ISO9660Reader for T {
    fn read_at(&mut self, buf: &mut [u8], lba: u64) -> Result<usize> {
        self.seek(SeekFrom::Start(lba * u64::from(BLOCK_SIZE)))?;
        self.read(buf)
    }
}

// TODO: Figure out if sane API possible without Rc/RefCell
pub(crate) struct FileRef<T: ISO9660Reader>(Rc<RefCell<T>>);

impl<T: ISO9660Reader> Clone for FileRef<T> {
    fn clone(&self) -> FileRef<T> {
        FileRef(self.0.clone())
    }
}

impl<T: ISO9660Reader> FileRef<T> {
    pub fn new(reader: T) -> FileRef<T> {
        FileRef(Rc::new(RefCell::new(reader)))
    }

    /// Read the block(s) at a given LBA (logical block address)
    pub fn read_at(&self, buf: &mut [u8], lba: u64) -> Result<usize> {
        (*self.0).borrow_mut().read_at(buf, lba)
    }
}
