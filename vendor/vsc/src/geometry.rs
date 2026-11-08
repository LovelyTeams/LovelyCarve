//! NTFS primary/backup geometry fallback for corrupt catalog volume sizes.

use std::io::{Read, Seek, SeekFrom};

use crate::bytes::{le_u16, le_u64};
use crate::VssError;

fn decode_boot(boot: &[u8], offset: u64, source_size: u64) -> Result<(u64, u64), VssError> {
    let corrupt = |reason| VssError::CorruptVolumeMetadata { offset, reason };
    if boot.get(3..11) != Some(b"NTFS    ") || boot.get(510..512) != Some(&[0x55, 0xaa]) {
        return Err(corrupt(
            "snapshot size fallback requires an NTFS boot sector",
        ));
    }
    let sector = u64::from(le_u16(boot, 11));
    if !matches!(sector, 256 | 512 | 1024 | 2048 | 4096) {
        return Err(corrupt("invalid NTFS sector size"));
    }
    // NTFS's sector count excludes the final backup boot sector.
    let size = le_u64(boot, 40)
        .checked_add(1)
        .and_then(|sectors| sectors.checked_mul(sector))
        .filter(|size| *size >= 2 * sector && *size <= source_size)
        .ok_or_else(|| corrupt("NTFS volume size overflows or exceeds the source"))?;
    Ok((size, sector))
}

pub(crate) fn ntfs_volume_size<R: Read + Seek>(
    reader: &mut R,
    source_size: u64,
) -> Result<u64, VssError> {
    reader.seek(SeekFrom::Start(0))?;
    let mut boot = [0; 512];
    reader.read_exact(&mut boot)?;
    let (size, sector) = decode_boot(&boot, 0, source_size)?;
    let backup_offset = size - sector;
    reader.seek(SeekFrom::Start(backup_offset))?;
    reader.read_exact(&mut boot)?;
    let backup = decode_boot(&boot, backup_offset, source_size)?;
    if backup != (size, sector) {
        return Err(VssError::CorruptVolumeMetadata {
            offset: backup_offset,
            reason: "primary and backup NTFS geometry disagree",
        });
    }
    Ok(size)
}
