//! Sector-aligned boot discovery. Geometry and full volume bounds must validate.
use super::*;
use crate::core::evidence::partitions::fat::detect_valid_fat_boot_sector;
use crate::core::filesystem::volumes::exfat_recover::ExfatGeom;

pub(super) fn discover(
    image: &DiskImage,
    options: &RecoveryOptions,
    ranges: &[SectorRange],
    cancel: &CancellationToken,
    progress: &mut dyn FnMut(u64, Option<u64>, &str),
) -> Result<(Vec<RecoveryVolume>, bool)> {
    let sector = image.sector_size as u64;
    let total = image.len() / sector;
    if ranges.is_empty() {
        bail!("Scan range is empty");
    }
    let mut bounds = Vec::with_capacity(ranges.len());
    for range in ranges {
        if range.sector_count == 0 {
            bail!("Sector count must be positive");
        }
        let end = range
            .start_sector
            .checked_add(range.sector_count)
            .context("Scan range overflow")?;
        if end > total {
            bail!("Scan range is outside the evidence");
        }
        bounds.push((range.start_sector, end));
    }
    let scanned_total = bounds
        .iter()
        .map(|(start, end)| (end - start) * sector)
        .sum::<u64>();
    let mut volumes = Vec::new();
    for partition in &image.partitions {
        if options.accepts(partition.kind)
            && bounds
                .iter()
                .any(|(start, end)| (*start..*end).contains(&partition.start_lba))
        {
            if volumes.len() >= MAX_VOLUMES {
                return Ok((volumes, true));
            }
            volumes.push(RecoveryVolume {
                partition: partition.clone(),
                lost: false,
                reconstruction: None,
                error: None,
            });
        }
    }
    let mut scanned = 0u64;
    for bound in bounds {
        let at_limit =
            scan_boot_sectors(image, options, bound, &mut volumes, cancel, &mut |bytes| {
                scanned += bytes;
                progress(
                    scanned,
                    Some(scanned_total),
                    "Scanning filesystem boot sectors",
                );
            })?;
        if at_limit {
            return Ok((volumes, true));
        }
    }
    Ok((volumes, false))
}

/// Scans one sector range for boot sectors and residual MFTs; returns `true` at the volume limit.
fn scan_boot_sectors(
    image: &DiskImage,
    options: &RecoveryOptions,
    (start_sector, end_sector): (u64, u64),
    volumes: &mut Vec<RecoveryVolume>,
    cancel: &CancellationToken,
    advance: &mut dyn FnMut(u64),
) -> Result<bool> {
    let sector = image.sector_size as u64;
    let mut offset = start_sector * sector;
    let end_bytes = end_sector * sector;
    while offset < end_bytes {
        check_cancel(cancel)?;
        let length = (end_bytes - offset).min(4 * 1024 * 1024) as usize;
        let bytes = image.read_range(offset, length)?;
        if bytes.len() != length {
            bail!("Short read during filesystem discovery at {offset}");
        }
        for (index, boot) in bytes.chunks_exact(sector as usize).enumerate() {
            let start = offset + index as u64 * sector;
            let lba = start / sector;
            if options.ntfs
                && boot.get(..4) == Some(b"FILE")
                && boot.get(44..48) == Some(&[0, 0, 0, 0])
            {
                match crate::core::filesystem::volumes::ntfs::discover_mft(image, start) {
                    Ok((partition, geometry)) => {
                        if !volumes.iter().any(|volume| {
                            volume.partition.kind == PartitionKind::Ntfs
                                && volume.partition.start_lba == partition.start_lba
                        }) {
                            if volumes.len() >= MAX_VOLUMES {
                                return Ok(true);
                            }
                            volumes.push(RecoveryVolume {
                                partition,
                                lost: true,
                                reconstruction: Some(geometry),
                                error: None,
                            });
                        }
                    }
                    Err(error) => tracing::trace!("MFT candidate at {start} rejected: {error:#}"),
                }
            }

            if volumes
                .iter()
                .any(|volume| volume.partition.start_lba == lba)
            {
                continue;
            }
            let Some((kind, length_bytes)) = boot_geometry(boot, image.len() - start) else {
                continue;
            };
            if !options.accepts(kind) || !length_bytes.is_multiple_of(sector) {
                continue;
            }
            // Backup FAT/exFAT boots describe the primary volume, not another volume.
            if volumes.iter().any(|volume| {
                let primary = volume.partition.start_lba * sector;
                volume.partition.kind == kind
                    && ((kind == PartitionKind::Fat32
                        && start
                            == primary + 6 * u64::from(u16::from_le_bytes([boot[11], boot[12]])))
                        || (kind == PartitionKind::ExFat
                            && start == primary + 12 * (1u64 << boot[108])))
            }) {
                continue;
            }
            if volumes.len() >= MAX_VOLUMES {
                return Ok(true);
            }
            volumes.push(RecoveryVolume {
                partition: Partition {
                    id: 0,
                    kind,
                    start_lba: lba,
                    length_lba: length_bytes / sector,
                    label: None,
                    unique_guid: None,
                },
                // A boot inside an existing partition can still be an old filesystem.
                lost: true,
                reconstruction: None,
                error: None,
            });
        }
        offset += length as u64;
        advance(length as u64);
    }
    Ok(false)
}

fn boot_geometry(boot: &[u8], remaining: u64) -> Option<(PartitionKind, u64)> {
    if boot.len() < 512 || boot[510..512] != [0x55, 0xaa] {
        return None;
    }
    if &boot[3..11] == b"EXFAT   " {
        ExfatGeom::parse(boot, 0).ok()?;
        let size =
            u64::from_le_bytes(boot[72..80].try_into().ok()?).checked_mul(1u64 << boot[108])?;
        return (size <= remaining).then_some((PartitionKind::ExFat, size));
    }
    let sector = u64::from(u16::from_le_bytes([boot[11], boot[12]]));
    if &boot[3..11] == b"NTFS    " {
        if !matches!(sector, 512 | 1024 | 2048 | 4096)
            || boot[13] == 0
            || !boot[13].is_power_of_two()
        {
            return None;
        }
        let size = u64::from_le_bytes(boot[40..48].try_into().ok()?).checked_mul(sector)?;
        return (size >= sector && size <= remaining).then_some((PartitionKind::Ntfs, size));
    }
    let kind = detect_valid_fat_boot_sector(boot, remaining)?;
    let short = u16::from_le_bytes([boot[19], boot[20]]);
    let sectors = if short != 0 {
        u64::from(short)
    } else {
        u64::from(u32::from_le_bytes(boot[32..36].try_into().ok()?))
    };
    Some((kind, sectors.checked_mul(sector)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_boot_rejects_short_or_out_of_bounds_ntfs() {
        assert!(boot_geometry(&[0; 511], u64::MAX).is_none());
        let mut boot = [0; 512];
        boot[3..11].copy_from_slice(b"NTFS    ");
        boot[11..13].copy_from_slice(&512u16.to_le_bytes());
        boot[13] = 8;
        boot[40..48].copy_from_slice(&100u64.to_le_bytes());
        boot[510..512].copy_from_slice(&[0x55, 0xaa]);
        assert_eq!(
            boot_geometry(&boot, 51200),
            Some((PartitionKind::Ntfs, 51200))
        );
        assert!(boot_geometry(&boot, 51199).is_none());
        boot[13] = 3;
        assert!(boot_geometry(&boot, 51200).is_none());
    }
}
