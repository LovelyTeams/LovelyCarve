//! Whole-device FAT12 / FAT16 / FAT32 detection.
//!
//! USB flash images are often "superfloppy" volumes with no MBR/GPT wrapper.
//! Some forensic samples also have a damaged primary boot sector while the
//! FAT32 backup boot sector remains intact, so this parser accepts either the
//! primary BPB at sector 0 or the standard FAT32 backup at sector 6.

use std::io::{Cursor, SeekFrom};

use binrw::BinRead;

use crate::core::evidence::partitions::{
    DetectCtx, PartParser, Partition, PartitionKind, SectorSource,
};

const BOOT_SECTOR_LEN: usize = 512;
pub(crate) const FAT32_BACKUP_BOOT_SECTOR: u64 = 6;
const BOOT_SIGNATURE: [u8; 2] = [0x55, 0xAA];
const FAT32_FSINFO_LEAD: [u8; 4] = [0x52, 0x52, 0x61, 0x41];
const FAT32_FSINFO_STRUCT: [u8; 4] = [0x72, 0x72, 0x41, 0x61];
const FAT32_EOC_MIN: u32 = 0x0FFF_FFF8;
const FAT32_MEDIA_DESCRIPTOR: u8 = 0xF8;
const FAT32_ROOT_CLUSTER: u32 = 2;
const FAT32_SCAN_LIMIT_BYTES: u64 = 16 * 1024 * 1024;
const FAT32_MAX_RESERVED_SECTORS: u64 = u16::MAX as u64;
const FAT32_MIN_CLUSTERS: u64 = 65_525;

pub struct FatParser;

#[derive(Debug, BinRead)]
#[br(little)]
struct FatBootSector {
    #[br(seek_before = SeekFrom::Start(11))]
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    fats: u8,
    root_entries: u16,
    total_sectors_16: u16,
    #[br(seek_before = SeekFrom::Start(22))]
    fat_sectors_16: u16,
    #[br(seek_before = SeekFrom::Start(32))]
    total_sectors_32: u32,
    fat_sectors_32: u32,
}

impl PartParser for FatParser {
    fn name(&self) -> &'static str {
        "fat"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        let primary = ctx.head.get(..BOOT_SECTOR_LEN).unwrap_or(ctx.head);
        if let Some(kind) = detect_valid_fat_boot_sector(primary, ctx.size) {
            return Ok(Some(vec![whole_device_partition(
                kind,
                ctx.size,
                ctx.sector_size,
            )]));
        }

        let backup_offset = FAT32_BACKUP_BOOT_SECTOR.saturating_mul(ctx.sector_size as u64);
        if backup_offset + BOOT_SECTOR_LEN as u64 <= ctx.size {
            let backup = ctx.source.read(backup_offset, BOOT_SECTOR_LEN)?;
            if let Some(kind) = detect_valid_fat_boot_sector(&backup, ctx.size) {
                return Ok(Some(vec![whole_device_partition(
                    kind,
                    ctx.size,
                    ctx.sector_size,
                )]));
            }
        }

        if recover_fat32_boot_sector(ctx.source, ctx.size, ctx.sector_size as u64)?.is_some() {
            return Ok(Some(vec![whole_device_partition(
                PartitionKind::Fat32,
                ctx.size,
                ctx.sector_size,
            )]));
        }

        Ok(None)
    }
}

fn whole_device_partition(kind: PartitionKind, size_bytes: u64, sector_size: usize) -> Partition {
    Partition {
        id: 0,
        kind,
        start_lba: 0,
        length_lba: size_bytes / sector_size as u64,
        label: None,
        unique_guid: None,
        amiga_geometry: None,
    }
}

pub(crate) fn detect_valid_fat_boot_sector(bytes: &[u8], image_size: u64) -> Option<PartitionKind> {
    if bytes.len() < BOOT_SECTOR_LEN || bytes[510..512] != BOOT_SIGNATURE {
        return None;
    }

    let boot = FatBootSector::read(&mut Cursor::new(bytes)).ok()?;
    let bytes_per_sector = boot.bytes_per_sector;
    if !matches!(
        bytes_per_sector,
        512 | 1024 | 2048 | 4096 | 8192 | 16384 | 32768
    ) {
        return None;
    }

    let sectors_per_cluster = boot.sectors_per_cluster;
    if sectors_per_cluster == 0 || !sectors_per_cluster.is_power_of_two() {
        return None;
    }

    let reserved_sectors = u32::from(boot.reserved_sectors);
    let fats = u32::from(boot.fats);
    if reserved_sectors == 0 || !matches!(fats, 1 | 2) {
        return None;
    }

    let root_entries = u32::from(boot.root_entries);
    let total_sectors_16 = u32::from(boot.total_sectors_16);
    let total_sectors_32 = boot.total_sectors_32;
    let total_sectors = if total_sectors_16 != 0 {
        total_sectors_16
    } else {
        total_sectors_32
    };
    if total_sectors == 0 {
        return None;
    }

    let declared_bytes = u64::from(total_sectors).saturating_mul(u64::from(bytes_per_sector));
    if declared_bytes > image_size {
        return None;
    }

    let fat_sectors_16 = u32::from(boot.fat_sectors_16);
    let fat_sectors_32 = boot.fat_sectors_32;
    let is_fat32 = fat_sectors_16 == 0;
    let fat_sectors = if is_fat32 {
        fat_sectors_32
    } else {
        fat_sectors_16
    };
    if fat_sectors == 0 {
        return None;
    }

    let root_dir_sectors = (root_entries * 32).div_ceil(u32::from(bytes_per_sector));
    let first_data_sector = reserved_sectors
        .checked_add(fats.checked_mul(fat_sectors)?)?
        .checked_add(root_dir_sectors)?;
    if first_data_sector >= total_sectors {
        return None;
    }

    let cluster_count = (total_sectors - first_data_sector) / u32::from(sectors_per_cluster);
    let inferred_fat32 = cluster_count >= 65_525;
    if is_fat32 {
        if root_entries != 0 || total_sectors_16 != 0 || !inferred_fat32 {
            return None;
        }
        Some(PartitionKind::Fat32)
    } else {
        if root_entries == 0 || inferred_fat32 {
            return None;
        }
        Some(if cluster_count < 4085 {
            PartitionKind::Fat12
        } else {
            PartitionKind::Fat16
        })
    }
}

pub(crate) fn recover_fat32_boot_sector(
    source: &dyn SectorSource,
    volume_size: u64,
    sector_size: u64,
) -> anyhow::Result<Option<[u8; BOOT_SECTOR_LEN]>> {
    if sector_size != BOOT_SECTOR_LEN as u64 || volume_size < FAT32_MIN_CLUSTERS * sector_size {
        return Ok(None);
    }

    let primary = source.read(0, BOOT_SECTOR_LEN)?;
    if detect_valid_fat_boot_sector(&primary, volume_size).is_some() {
        return Ok(None);
    }

    let backup_offset = FAT32_BACKUP_BOOT_SECTOR * sector_size;
    if backup_offset + BOOT_SECTOR_LEN as u64 <= volume_size {
        let backup = source.read(backup_offset, BOOT_SECTOR_LEN)?;
        if detect_valid_fat_boot_sector(&backup, volume_size).is_some() {
            let mut boot_sector = [0u8; BOOT_SECTOR_LEN];
            boot_sector.copy_from_slice(&backup);
            return Ok(Some(boot_sector));
        }
    }

    let fsinfo = find_fat32_fsinfo_sector(source, volume_size, sector_size)?;
    let Some(fsinfo) = fsinfo else {
        return Ok(None);
    };

    let fat_offset = find_fat32_table_start(source, volume_size, sector_size)?;
    let Some(fat_offset) = fat_offset else {
        return Ok(None);
    };
    let root_offset = find_fat32_root_dir_start(source, volume_size, sector_size, fat_offset)?;
    let Some(root_offset) = root_offset else {
        return Ok(None);
    };
    if root_offset <= fat_offset || (root_offset - fat_offset) % sector_size != 0 {
        return Ok(None);
    }

    let reserved_sectors = fat_offset / sector_size;
    if reserved_sectors == 0 || reserved_sectors > FAT32_MAX_RESERVED_SECTORS {
        return Ok(None);
    }
    let fat_sectors = (root_offset - fat_offset) / sector_size;
    if fat_sectors == 0 || fat_sectors > u32::MAX as u64 {
        return Ok(None);
    }
    let total_sectors = volume_size / sector_size;
    if total_sectors == 0 || total_sectors > u32::MAX as u64 {
        return Ok(None);
    }

    let Some(sectors_per_cluster) =
        infer_fat32_sectors_per_cluster(total_sectors, reserved_sectors, fat_sectors, fsinfo)
    else {
        return Ok(None);
    };

    let mut boot_sector = [0u8; BOOT_SECTOR_LEN];
    write_recovered_fat32_boot_sector(
        &mut boot_sector,
        sector_size as u16,
        sectors_per_cluster,
        reserved_sectors as u16,
        fat_sectors as u32,
        total_sectors as u32,
    );
    Ok(Some(boot_sector))
}

#[derive(Clone, Copy)]
struct Fat32FsInfo {
    free_clusters: Option<u32>,
    next_free_cluster: Option<u32>,
}

fn find_fat32_fsinfo_sector(
    source: &dyn SectorSource,
    volume_size: u64,
    sector_size: u64,
) -> anyhow::Result<Option<Fat32FsInfo>> {
    for sector in [1, FAT32_BACKUP_BOOT_SECTOR + 1] {
        let offset = sector * sector_size;
        if offset + BOOT_SECTOR_LEN as u64 > volume_size {
            continue;
        }
        let bytes = source.read(offset, BOOT_SECTOR_LEN)?;
        if let Some(fsinfo) = parse_fat32_fsinfo(&bytes) {
            return Ok(Some(fsinfo));
        }
    }
    Ok(None)
}

fn parse_fat32_fsinfo(bytes: &[u8]) -> Option<Fat32FsInfo> {
    if bytes.len() < BOOT_SECTOR_LEN
        || bytes.get(0..4) != Some(&FAT32_FSINFO_LEAD)
        || bytes.get(484..488) != Some(&FAT32_FSINFO_STRUCT)
        || bytes[510..512] != BOOT_SIGNATURE
    {
        return None;
    }
    let free_clusters = read_u32(bytes, 488).filter(|&value| value != u32::MAX);
    let next_free_cluster = read_u32(bytes, 492).filter(|&value| value != u32::MAX);
    Some(Fat32FsInfo {
        free_clusters,
        next_free_cluster,
    })
}

fn find_fat32_table_start(
    source: &dyn SectorSource,
    volume_size: u64,
    sector_size: u64,
) -> anyhow::Result<Option<u64>> {
    let scan_end = volume_size.min(FAT32_SCAN_LIMIT_BYTES);
    let mut offset = (FAT32_BACKUP_BOOT_SECTOR + 2) * sector_size;
    while offset + BOOT_SECTOR_LEN as u64 <= scan_end {
        let bytes = source.read(offset, BOOT_SECTOR_LEN)?;
        if looks_like_fat32_table_start(&bytes) {
            return Ok(Some(offset));
        }
        offset += sector_size;
    }
    Ok(None)
}

fn looks_like_fat32_table_start(bytes: &[u8]) -> bool {
    if bytes.len() < 16 || bytes[0] != FAT32_MEDIA_DESCRIPTOR {
        return false;
    }
    let Some(entry0) = read_u32(bytes, 0) else {
        return false;
    };
    let Some(entry1) = read_u32(bytes, 4) else {
        return false;
    };
    let Some(entry2) = read_u32(bytes, 8) else {
        return false;
    };
    (entry0 & 0x0FFF_FFFF) >= FAT32_EOC_MIN
        && (entry1 & 0x0FFF_FFFF) >= FAT32_EOC_MIN
        && entry2 != 0
}

fn find_fat32_root_dir_start(
    source: &dyn SectorSource,
    volume_size: u64,
    sector_size: u64,
    fat_offset: u64,
) -> anyhow::Result<Option<u64>> {
    let scan_end = volume_size.min(fat_offset.saturating_add(FAT32_SCAN_LIMIT_BYTES));
    let mut offset = fat_offset + sector_size;
    while offset + BOOT_SECTOR_LEN as u64 <= scan_end {
        let bytes = source.read(offset, BOOT_SECTOR_LEN)?;
        if looks_like_fat_root_dir_sector(&bytes) {
            return Ok(Some(offset));
        }
        offset += sector_size;
    }
    Ok(None)
}

fn looks_like_fat_root_dir_sector(bytes: &[u8]) -> bool {
    if bytes.len() < BOOT_SECTOR_LEN {
        return false;
    }

    let mut valid_entries = 0usize;
    let mut has_volume_or_directory = false;
    for entry in bytes.as_chunks::<32>().0 {
        match entry[0] {
            0x00 => break,
            0xE5 => continue,
            0x2E => continue,
            _ => {}
        }
        let attr = entry[11];
        if attr == 0 || attr & 0xC0 != 0 {
            return false;
        }
        if attr == 0x0F {
            valid_entries += 1;
            continue;
        }
        if entry[0..11].iter().any(|&byte| byte < 0x20 && byte != 0x05) {
            return false;
        }
        if attr & 0x18 != 0 {
            has_volume_or_directory = true;
        }
        valid_entries += 1;
    }
    valid_entries >= 2 && has_volume_or_directory
}

fn infer_fat32_sectors_per_cluster(
    total_sectors: u64,
    reserved_sectors: u64,
    fat_sectors: u64,
    fsinfo: Fat32FsInfo,
) -> Option<u8> {
    let data_sectors = total_sectors.checked_sub(reserved_sectors + fat_sectors)?;
    let fat_entries = fat_sectors.checked_mul(BOOT_SECTOR_LEN as u64)? / 4;
    [1u8, 2, 4, 8, 16, 32, 64, 128]
        .into_iter()
        .find(|&sectors_per_cluster| {
            let cluster_count = data_sectors / u64::from(sectors_per_cluster);
            if cluster_count < FAT32_MIN_CLUSTERS || cluster_count + 2 > fat_entries {
                return false;
            }
            if fsinfo
                .free_clusters
                .is_some_and(|free| u64::from(free) > cluster_count)
            {
                return false;
            }
            if fsinfo
                .next_free_cluster
                .is_some_and(|next| next >= 2 && u64::from(next) > cluster_count + 1)
            {
                return false;
            }
            true
        })
}

fn write_recovered_fat32_boot_sector(
    sector: &mut [u8; BOOT_SECTOR_LEN],
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    fat_sectors: u32,
    total_sectors: u32,
) {
    sector[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    sector[3..11].copy_from_slice(b"MSDOS5.0");
    sector[11..13].copy_from_slice(&bytes_per_sector.to_le_bytes());
    sector[13] = sectors_per_cluster;
    sector[14..16].copy_from_slice(&reserved_sectors.to_le_bytes());
    sector[16] = 1;
    sector[21] = FAT32_MEDIA_DESCRIPTOR;
    sector[24..26].copy_from_slice(&63u16.to_le_bytes());
    sector[26..28].copy_from_slice(&255u16.to_le_bytes());
    sector[32..36].copy_from_slice(&total_sectors.to_le_bytes());
    sector[36..40].copy_from_slice(&fat_sectors.to_le_bytes());
    sector[44..48].copy_from_slice(&FAT32_ROOT_CLUSTER.to_le_bytes());
    sector[48..50].copy_from_slice(&1u16.to_le_bytes());
    sector[50..52].copy_from_slice(&(FAT32_BACKUP_BOOT_SECTOR as u16).to_le_bytes());
    sector[64] = 0x80;
    sector[66] = 0x29;
    sector[71..82].copy_from_slice(b"NO NAME    ");
    sector[82..90].copy_from_slice(b"FAT32   ");
    sector[510..512].copy_from_slice(&BOOT_SIGNATURE);
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::partitions::{detect_partitions, test_support::BufSource};
    use std::collections::BTreeMap;

    #[test]
    fn detects_whole_device_fat32_from_primary_boot_sector() {
        let sector = 512usize;
        let total_sectors = 100_000u32;
        let mut buf = vec![0u8; sector * total_sectors as usize];
        write_fat32_boot(&mut buf[..sector], total_sectors, 2, 100);
        let src = BufSource(buf);

        let parts = detect_partitions(&src, sector).expect("detect");

        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].id, 0);
        assert_eq!(parts[0].kind, PartitionKind::Fat32);
        assert_eq!(parts[0].start_lba, 0);
        assert_eq!(parts[0].length_lba, u64::from(total_sectors));
        assert_eq!(parts[0].label, None);
    }

    #[test]
    fn detects_whole_device_fat32_from_backup_boot_sector() {
        let sector = 512usize;
        let total_sectors = 100_000u32;
        let mut buf = vec![0u8; sector * total_sectors as usize];
        let backup = sector * FAT32_BACKUP_BOOT_SECTOR as usize;
        write_fat32_boot(&mut buf[backup..backup + sector], total_sectors, 1, 6);
        let src = BufSource(buf);

        let parts = detect_partitions(&src, sector).expect("detect");

        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Fat32);
        assert_eq!(parts[0].start_lba, 0);
    }

    #[test]
    fn rejects_fat32_that_declares_more_bytes_than_source() {
        let sector = 512usize;
        let mut buf = vec![0u8; sector * 16];
        write_fat32_boot(&mut buf[..sector], 100_000, 1, 6);
        let src = BufSource(buf);

        let parts = detect_partitions(&src, sector).expect("detect");

        assert!(parts.is_empty());
    }

    #[test]
    fn recovers_fat32_boot_from_fsinfo_table_and_root_dir() {
        let sector = 512u64;
        let total_sectors = 67_000u64;
        let reserved = 32u64;
        let fat_sectors = 520u64;
        let fat_offset = reserved * sector;
        let root_offset = (reserved + fat_sectors) * sector;
        let mut source = SparseSource::new(total_sectors * sector);
        source.insert_sector(sector, fsinfo_sector(60_000, 4));
        source.insert_sector(fat_offset, fat32_table_sector());
        source.insert_sector(root_offset, root_dir_sector());

        let recovered = recover_fat32_boot_sector(&source, source.size(), sector)
            .expect("recover")
            .expect("synthetic boot sector");

        assert_eq!(
            u16::from_le_bytes(recovered[11..13].try_into().expect("bytes per sector")),
            512
        );
        assert_eq!(recovered[13], 1);
        assert_eq!(
            u16::from_le_bytes(recovered[14..16].try_into().expect("reserved sectors")),
            reserved as u16
        );
        assert_eq!(read_u32(&recovered, 36), Some(fat_sectors as u32));
        assert_eq!(read_u32(&recovered, 44), Some(2));
        assert_eq!(
            detect_valid_fat_boot_sector(&recovered, source.size()),
            Some(PartitionKind::Fat32)
        );
    }

    fn write_fat32_boot(
        sector: &mut [u8],
        total_sectors: u32,
        fsinfo_sector: u16,
        backup_sector: u16,
    ) {
        sector[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        sector[3..11].copy_from_slice(b"MSDOS5.0");
        sector[11..13].copy_from_slice(&512u16.to_le_bytes());
        sector[13] = 1;
        sector[14..16].copy_from_slice(&32u16.to_le_bytes());
        sector[16] = 2;
        sector[21] = 0xF8;
        sector[24..26].copy_from_slice(&63u16.to_le_bytes());
        sector[26..28].copy_from_slice(&255u16.to_le_bytes());
        sector[32..36].copy_from_slice(&total_sectors.to_le_bytes());
        sector[36..40].copy_from_slice(&100u32.to_le_bytes());
        sector[44..48].copy_from_slice(&2u32.to_le_bytes());
        sector[48..50].copy_from_slice(&fsinfo_sector.to_le_bytes());
        sector[50..52].copy_from_slice(&backup_sector.to_le_bytes());
        sector[64] = 0x80;
        sector[66] = 0x29;
        sector[67..71].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        sector[71..82].copy_from_slice(b"NO NAME    ");
        sector[82..90].copy_from_slice(b"FAT32   ");
        sector[510..512].copy_from_slice(&BOOT_SIGNATURE);
    }

    struct SparseSource {
        size: u64,
        sectors: BTreeMap<u64, [u8; BOOT_SECTOR_LEN]>,
    }

    impl SparseSource {
        fn new(size: u64) -> Self {
            Self {
                size,
                sectors: BTreeMap::new(),
            }
        }

        fn insert_sector(&mut self, offset: u64, sector: [u8; BOOT_SECTOR_LEN]) {
            self.sectors.insert(offset, sector);
        }
    }

    impl SectorSource for SparseSource {
        fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
            let mut out = vec![0; len];
            for (&sector_offset, sector) in &self.sectors {
                let sector_end = sector_offset + BOOT_SECTOR_LEN as u64;
                let read_end = offset + len as u64;
                if sector_offset >= read_end || sector_end <= offset {
                    continue;
                }
                let copy_start = sector_offset.max(offset);
                let copy_end = sector_end.min(read_end);
                let dst_start = (copy_start - offset) as usize;
                let src_start = (copy_start - sector_offset) as usize;
                let copy_len = (copy_end - copy_start) as usize;
                out[dst_start..dst_start + copy_len]
                    .copy_from_slice(&sector[src_start..src_start + copy_len]);
            }
            Ok(out)
        }

        fn size(&self) -> u64 {
            self.size
        }
    }

    fn fsinfo_sector(free_clusters: u32, next_free_cluster: u32) -> [u8; BOOT_SECTOR_LEN] {
        let mut sector = [0u8; BOOT_SECTOR_LEN];
        sector[0..4].copy_from_slice(&FAT32_FSINFO_LEAD);
        sector[484..488].copy_from_slice(&FAT32_FSINFO_STRUCT);
        sector[488..492].copy_from_slice(&free_clusters.to_le_bytes());
        sector[492..496].copy_from_slice(&next_free_cluster.to_le_bytes());
        sector[510..512].copy_from_slice(&BOOT_SIGNATURE);
        sector
    }

    fn fat32_table_sector() -> [u8; BOOT_SECTOR_LEN] {
        let mut sector = [0u8; BOOT_SECTOR_LEN];
        for (index, entry) in [0x0FFF_FFF8u32, 0x0FFF_FFFF, 0x0FFF_FFFF, 0x0FFF_FFFF]
            .into_iter()
            .enumerate()
        {
            let start = index * 4;
            sector[start..start + 4].copy_from_slice(&entry.to_le_bytes());
        }
        sector
    }

    fn root_dir_sector() -> [u8; BOOT_SECTOR_LEN] {
        let mut sector = [0u8; BOOT_SECTOR_LEN];
        sector[0..11].copy_from_slice(b"NO NAME    ");
        sector[11] = 0x08;
        sector[32] = 0x41;
        sector[33..43].copy_from_slice(b"T\0e\0s\0t\0\0\0");
        sector[43] = 0x0F;
        sector[64..75].copy_from_slice(b"TEST       ");
        sector[75] = 0x10;
        sector
    }
}
