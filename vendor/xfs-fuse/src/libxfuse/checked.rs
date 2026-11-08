//! Checked big-endian metadata access and CRC32C validation.
use super::compat::EIO;
use crc::{Crc, CRC_32_ISCSI};

pub(crate) fn bytes(data: &[u8], offset: usize, length: usize) -> Result<&[u8], i32> {
    data.get(offset..offset.checked_add(length).ok_or(EIO)?)
        .ok_or(EIO)
}
pub(crate) fn u16be(data: &[u8], offset: usize) -> Result<u16, i32> {
    Ok(u16::from_be_bytes(
        bytes(data, offset, 2)?.try_into().map_err(|_| EIO)?,
    ))
}
pub(crate) fn u32be(data: &[u8], offset: usize) -> Result<u32, i32> {
    Ok(u32::from_be_bytes(
        bytes(data, offset, 4)?.try_into().map_err(|_| EIO)?,
    ))
}
pub(crate) fn u64be(data: &[u8], offset: usize) -> Result<u64, i32> {
    Ok(u64::from_be_bytes(
        bytes(data, offset, 8)?.try_into().map_err(|_| EIO)?,
    ))
}
pub(crate) fn crc(data: &[u8], offset: usize) -> Result<(), i32> {
    let expected = u32::from_le_bytes(bytes(data, offset, 4)?.try_into().map_err(|_| EIO)?);
    let algorithm = Crc::<u32>::new(&CRC_32_ISCSI);
    let mut digest = algorithm.digest();
    digest.update(bytes(data, 0, offset)?);
    digest.update(&[0; 4]);
    digest.update(data.get(offset + 4..).ok_or(EIO)?);
    if digest.finalize() != expected {
        return Err(EIO);
    }
    Ok(())
}
