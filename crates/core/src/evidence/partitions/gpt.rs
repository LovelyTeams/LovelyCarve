//! Read-only GPT analysis and detection. See `docs/formats/GPT.md` for recovery policy.

mod metadata;
mod reader;
mod table;

pub use metadata::{has_gpt_signature, GptEntry, GptHeader};
pub use reader::GptPartitionReader;
#[cfg(test)]
pub(crate) use table::tests::fixture;
pub use table::{infer_sector_size, GptTable};

use crate::evidence::partitions::{
    DetectCtx, DetectedTable, PartParser, Partition, PartitionKind, TableScheme,
};
use anyhow::Result;
use uuid::Uuid;

const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
#[cfg(test)]
const GPT_ENTRY_SIZE: u32 = 128;
const GPT_HEADER_BYTES: usize = 92;
const GPT_FALLBACK_LBAS: [u64; 4] = [1, 0, 7, 8];
const GPT_ENTRY_ARRAY_CAP: usize = 1024 * 1024;

/// BitLocker partition GUID (`AEAFCA3B-61C1-40E0-87A3-47D1AF93A9FC`). Not
/// present in `gpt::partition_types` so we keep our own constant.
const BITLOCKER_GUID: Uuid = Uuid::from_bytes([
    0xAE, 0xAF, 0xCA, 0x3B, 0x61, 0xC1, 0x40, 0xE0, 0x87, 0xA3, 0x47, 0xD1, 0xAF, 0x93, 0xA9, 0xFC,
]);

/// FreeBSD UFS partition GUID (`516E7CB6-6ECF-11D6-8FF8-00022D09712B`). FreeBSD
/// installer images put the rootfs on a partition with this type.
const FREEBSD_UFS_GUID: Uuid = Uuid::from_bytes([
    0x51, 0x6E, 0x7C, 0xB6, 0x6E, 0xCF, 0x11, 0xD6, 0x8F, 0xF8, 0x00, 0x02, 0x2D, 0x09, 0x71, 0x2B,
]);

/// FreeBSD swap partition GUID (`516E7CB5-6ECF-11D6-8FF8-00022D09712B`).
const FREEBSD_SWAP_GUID: Uuid = Uuid::from_bytes([
    0x51, 0x6E, 0x7C, 0xB5, 0x6E, 0xCF, 0x11, 0xD6, 0x8F, 0xF8, 0x00, 0x02, 0x2D, 0x09, 0x71, 0x2B,
]);

/// FreeBSD ZFS partition GUID (`516E7CBA-6ECF-11D6-8FF8-00022D09712B`). FreeBSD
/// installer images put the rootfs ZFS pool on a partition with this type.
const FREEBSD_ZFS_GUID: Uuid = Uuid::from_bytes([
    0x51, 0x6E, 0x7C, 0xBA, 0x6E, 0xCF, 0x11, 0xD6, 0x8F, 0xF8, 0x00, 0x02, 0x2D, 0x09, 0x71, 0x2B,
]);

/// Solaris / illumos / Linux ZFS partition GUID
/// (`6A898CC3-1DD2-11B2-99A6-080020736631`). Originally registered by Solaris
/// for the `/usr` slice, this same GUID was adopted by OpenZFS and is what
/// ZFS-on-Linux writes when `zpool create` partitions a whole disk. Apple's
/// historical macOS ZFS port also used it.
const SOLARIS_ZFS_GUID: Uuid = Uuid::from_bytes([
    0x6A, 0x89, 0x8C, 0xC3, 0x1D, 0xD2, 0x11, 0xB2, 0x99, 0xA6, 0x08, 0x00, 0x20, 0x73, 0x66, 0x31,
]);

/// MidnightBSD ZFS partition GUID (`85D5E45D-237C-11E1-B4B3-E89A8F7FC3A7`).
const MIDNIGHTBSD_ZFS_GUID: Uuid = Uuid::from_bytes([
    0x85, 0xD5, 0xE4, 0x5D, 0x23, 0x7C, 0x11, 0xE1, 0xB4, 0xB3, 0xE8, 0x9A, 0x8F, 0x7F, 0xC3, 0xA7,
]);

/// Apple HFS+ partition GUID (`48465300-0000-11AA-AA11-00306543ECAC`). Used
/// for both HFS+ (case-insensitive) and HFSX (case-sensitive) on GPT-labeled
/// Apple disks; the case-sensitivity bit lives in the volume header at
/// offset 0x400, so we always emit `HfsPlus` here and let
/// `refine_partition_kinds` upgrade to `HfsX` when it probes the boot region.
const APPLE_HFS_GUID: Uuid = Uuid::from_bytes([
    0x48, 0x46, 0x53, 0x00, 0x00, 0x00, 0x11, 0xAA, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC,
]);

pub struct GptParser;

impl PartParser for GptParser {
    fn name(&self) -> &'static str {
        "gpt"
    }
    fn scheme(&self) -> TableScheme {
        TableScheme::Gpt
    }
    fn try_parse(&self, ctx: &DetectCtx<'_>) -> Result<Option<Vec<Partition>>> {
        Ok(self.try_parse_with_meta(ctx)?.map(|table| table.partitions))
    }

    fn try_parse_with_meta(&self, ctx: &DetectCtx<'_>) -> Result<Option<DetectedTable>> {
        if !matches!(ctx.sector_size, 512 | 1024 | 2048 | 4096) {
            return Ok(None);
        }
        let Some(table) = GptTable::read(ctx.source, ctx.sector_size)? else {
            return Ok(None);
        };
        for warning in &table.warnings {
            tracing::warn!("{warning}");
        }
        Ok(Some(DetectedTable {
            partitions: table.project()?,
            scheme: Some(TableScheme::Gpt),
            disk_id: table.disk_id(),
        }))
    }
}

fn guid_to_kind(guid: Uuid) -> PartitionKind {
    use gpt::partition_types as t;
    if guid == t::LINUX_LVM.guid {
        PartitionKind::Lvm
    } else if guid == t::LINUX_RAID.guid {
        PartitionKind::Mdadm
    } else if guid == t::LINUX_LUKS.guid {
        PartitionKind::Luks
    } else if guid == t::LINUX_SWAP.guid {
        PartitionKind::Swap
    } else if guid == t::BIOS.guid {
        PartitionKind::BiosBoot
    } else if guid == t::MACOS_APFS.guid {
        PartitionKind::ApfsContainer
    } else if guid
        == Uuid::from_bytes([
            0x53, 0x74, 0x6f, 0x72, 0x61, 0x67, 0x11, 0xaa, 0xaa, 0x11, 0x00, 0x30, 0x65, 0x43,
            0xec, 0xac,
        ])
    {
        PartitionKind::Filevault
    } else if guid == APPLE_HFS_GUID {
        PartitionKind::HfsPlus
    } else if guid == BITLOCKER_GUID {
        PartitionKind::Bitlocker
    } else if guid == FREEBSD_UFS_GUID {
        PartitionKind::Ufs
    } else if guid == FREEBSD_SWAP_GUID {
        PartitionKind::Swap
    } else if guid == FREEBSD_ZFS_GUID || guid == SOLARIS_ZFS_GUID || guid == MIDNIGHTBSD_ZFS_GUID {
        PartitionKind::Zfs
    } else {
        // Includes BASIC (`MS_BASIC_DATA`), LINUX_FS, MICROSOFT_RESERVED, EFI,
        // anything else: leave Unknown so `refine_partition_kinds` can probe
        // the boot region for an actual filesystem signature.
        PartitionKind::Unknown
    }
}

/// Decode the on-disk GPT type-GUID byte order (3 little-endian fields + an
/// 8-byte big-endian tail) into a canonical [`Uuid`]. `pub(crate)` so the
/// drive-letter recovery (`core::analysis::identify::win_drive_letters`) can decode the GUID that
/// Windows stores in a `MountedDevices` `DMIO:ID:` value using the identical
/// byte convention.
pub(crate) fn uuid_from_disk_bytes(bytes: &[u8]) -> Option<Uuid> {
    let arr: [u8; 16] = bytes.try_into().ok()?;
    let d1 = u32::from_le_bytes([arr[0], arr[1], arr[2], arr[3]]);
    let d2 = u16::from_le_bytes([arr[4], arr[5]]);
    let d3 = u16::from_le_bytes([arr[6], arr[7]]);
    let d4 = [
        arr[8], arr[9], arr[10], arr[11], arr[12], arr[13], arr[14], arr[15],
    ];
    Some(Uuid::from_fields(d1, d2, d3, &d4))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::partitions::{detect_partitions, test_support::BufSource};

    #[test]
    fn rejects_gpt_without_128_byte_entries() {
        let sector = 512usize;
        let mut buf = vec![0u8; sector * 34];
        buf[sector..sector + 8].copy_from_slice(GPT_SIGNATURE);
        buf[sector + 72..sector + 80].copy_from_slice(&2u64.to_le_bytes());
        buf[sector + 80..sector + 84].copy_from_slice(&128u32.to_le_bytes());
        buf[sector + 84..sector + 88].copy_from_slice(&64u32.to_le_bytes());
        buf[510] = 0x55;
        buf[511] = 0xAA;
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts
            .iter()
            .all(|p| p.kind != PartitionKind::Unknown || p.start_lba == 0 && p.length_lba == 0));
    }

    /// CRC-less fixture: the library refuses on header-CRC mismatch so this
    /// exercises the manual fallback at LBA 1. The disk is large enough that
    /// the partition stays in bounds (P1-11).
    #[test]
    fn parses_gpt_with_entry_at_lba_2_via_manual_fallback() {
        let sector = 512usize;
        let mut buf = vec![0u8; sector * 256];
        buf[sector..sector + 8].copy_from_slice(GPT_SIGNATURE);
        buf[sector + 72..sector + 80].copy_from_slice(&2u64.to_le_bytes());
        buf[sector + 80..sector + 84].copy_from_slice(&4u32.to_le_bytes());
        buf[sector + 84..sector + 88].copy_from_slice(&GPT_ENTRY_SIZE.to_le_bytes());
        let entries_off = sector * 2;
        buf[entries_off..entries_off + 16].copy_from_slice(&[1u8; 16]);
        buf[entries_off + 32..entries_off + 40].copy_from_slice(&100u64.to_le_bytes());
        buf[entries_off + 40..entries_off + 48].copy_from_slice(&199u64.to_le_bytes());
        let name = "data".encode_utf16().collect::<Vec<_>>();
        for (i, u) in name.iter().enumerate() {
            buf[entries_off + 56 + i * 2..entries_off + 58 + i * 2]
                .copy_from_slice(&u.to_le_bytes());
        }
        let src = BufSource(buf);
        let parts = detect_partitions(&src, sector).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].start_lba, 100);
        assert_eq!(parts[0].length_lba, 100);
        assert_eq!(parts[0].label.as_deref(), Some("data"));
    }

    /// P1-10: the manual walker must read the entry array from the source, not
    /// the 16-sector (0x2000) head probe. A 128-entry table at LBA 2 has its
    /// later entries well past byte 0x2000; an entry at index 100 (byte offset
    /// 0x2000 + …) must still be found.
    #[test]
    fn manual_fallback_reads_full_entry_array_past_head_probe() {
        let sector = 512usize;
        let mut buf = vec![0u8; sector * 512];
        buf[sector..sector + 8].copy_from_slice(GPT_SIGNATURE);
        buf[sector + 72..sector + 80].copy_from_slice(&2u64.to_le_bytes());
        buf[sector + 80..sector + 84].copy_from_slice(&128u32.to_le_bytes());
        buf[sector + 84..sector + 88].copy_from_slice(&GPT_ENTRY_SIZE.to_le_bytes());
        // First entry keeps the leading-entry bounds check happy.
        let entries_off = sector * 2;
        buf[entries_off..entries_off + 16].copy_from_slice(&[1u8; 16]);
        buf[entries_off + 32..entries_off + 40].copy_from_slice(&34u64.to_le_bytes());
        buf[entries_off + 40..entries_off + 48].copy_from_slice(&99u64.to_le_bytes());
        // Entry index 100 lands at byte 2*512 + 100*128 = 0x3C00, past 0x2000.
        let far = entries_off + 100 * GPT_ENTRY_SIZE as usize;
        assert!(far > super::super::HEADER_PROBE_BYTES);
        buf[far..far + 16].copy_from_slice(&[2u8; 16]);
        buf[far + 32..far + 40].copy_from_slice(&200u64.to_le_bytes());
        buf[far + 40..far + 48].copy_from_slice(&299u64.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, sector).expect("detect");
        assert_eq!(parts.len(), 2, "the entry past 0x2000 must be parsed");
        assert_eq!(parts[1].start_lba, 200);
    }

    /// P1-11: a lone `EFI PART` signature in a random sector with an entry that
    /// starts past the image end must be rejected, not minted into a garbage
    /// partition.
    #[test]
    fn rejects_manual_table_with_out_of_bounds_leading_entry() {
        let sector = 512usize;
        let mut buf = vec![0u8; sector * 64];
        buf[sector..sector + 8].copy_from_slice(GPT_SIGNATURE);
        buf[sector + 72..sector + 80].copy_from_slice(&2u64.to_le_bytes());
        buf[sector + 80..sector + 84].copy_from_slice(&4u32.to_le_bytes());
        buf[sector + 84..sector + 88].copy_from_slice(&GPT_ENTRY_SIZE.to_le_bytes());
        let entries_off = sector * 2;
        buf[entries_off..entries_off + 16].copy_from_slice(&[1u8; 16]);
        // start_lba 9_000 is far past the 64-sector image → reject the table.
        buf[entries_off + 32..entries_off + 40].copy_from_slice(&9_000u64.to_le_bytes());
        buf[entries_off + 40..entries_off + 48].copy_from_slice(&9_100u64.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, sector).expect("detect");
        assert!(
            parts.is_empty(),
            "out-of-bounds GPT must not yield partitions"
        );
    }

    /// P1-9: a wiped primary header with an intact backup header mirrored at the
    /// last LBA must still recover the table from the backup entry array.
    #[test]
    fn recovers_table_from_backup_header() {
        let sector = 512usize;
        let total = 256usize;
        let mut buf = vec![0u8; sector * total];
        // No primary `EFI PART` at LBA 1. Backup header sits at the last LBA.
        let backup_lba = total - 1;
        let backup_off = backup_lba * sector;
        buf[backup_off..backup_off + 8].copy_from_slice(GPT_SIGNATURE);
        // Backup entry array starts a few sectors before the backup header.
        let entries_lba = backup_lba - 32;
        buf[backup_off + 72..backup_off + 80].copy_from_slice(&(entries_lba as u64).to_le_bytes());
        buf[backup_off + 80..backup_off + 84].copy_from_slice(&4u32.to_le_bytes());
        buf[backup_off + 84..backup_off + 88].copy_from_slice(&GPT_ENTRY_SIZE.to_le_bytes());
        let entries_off = entries_lba * sector;
        buf[entries_off..entries_off + 16].copy_from_slice(&[1u8; 16]);
        buf[entries_off + 32..entries_off + 40].copy_from_slice(&34u64.to_le_bytes());
        buf[entries_off + 40..entries_off + 48].copy_from_slice(&99u64.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, sector).expect("detect");
        assert_eq!(parts.len(), 1, "backup header must recover the table");
        assert_eq!(parts[0].start_lba, 34);
        assert_eq!(parts[0].length_lba, 66);
    }

    /// Regression: an entry whose EndingLBA is u64::MAX must not overflow the
    /// `last_lba - first_lba + 1` length math (debug panic / release wrap).
    #[test]
    fn entry_with_max_ending_lba_does_not_overflow() {
        let sector = 512usize;
        let mut buf = vec![0u8; sector * 256];
        buf[sector..sector + 8].copy_from_slice(GPT_SIGNATURE);
        buf[sector + 72..sector + 80].copy_from_slice(&2u64.to_le_bytes());
        buf[sector + 80..sector + 84].copy_from_slice(&4u32.to_le_bytes());
        buf[sector + 84..sector + 88].copy_from_slice(&GPT_ENTRY_SIZE.to_le_bytes());
        let entries_off = sector * 2;
        buf[entries_off..entries_off + 16].copy_from_slice(&[1u8; 16]);
        buf[entries_off + 32..entries_off + 40].copy_from_slice(&0u64.to_le_bytes());
        buf[entries_off + 40..entries_off + 48].copy_from_slice(&u64::MAX.to_le_bytes());
        let src = BufSource(buf);
        // Must not panic; the overflowing entry is skipped, and its huge start
        // is in bounds (0) but length overflows → dropped, leaving no parts.
        let parts = detect_partitions(&src, sector).expect("detect");
        assert!(parts.is_empty());
    }

    /// P1-14 + P1-9/10: a 4Kn image whose only `EFI PART` sits at LBA 7
    /// (byte 28672, past the 0x2000 head probe) must still be recovered via
    /// the source-backed signature probe.
    #[test]
    fn manual_fallback_finds_signature_past_head_probe_at_4kn() {
        let sector = 4096usize;
        let mut buf = vec![0u8; sector * 64];
        let header_off = 7 * sector;
        buf[header_off..header_off + 8].copy_from_slice(GPT_SIGNATURE);
        buf[header_off + 72..header_off + 80].copy_from_slice(&8u64.to_le_bytes());
        buf[header_off + 80..header_off + 84].copy_from_slice(&4u32.to_le_bytes());
        buf[header_off + 84..header_off + 88].copy_from_slice(&GPT_ENTRY_SIZE.to_le_bytes());
        let entries_off = 8 * sector;
        buf[entries_off..entries_off + 16].copy_from_slice(&[1u8; 16]);
        buf[entries_off + 32..entries_off + 40].copy_from_slice(&10u64.to_le_bytes());
        buf[entries_off + 40..entries_off + 48].copy_from_slice(&20u64.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, sector).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].start_lba, 10);
    }

    #[test]
    fn maps_known_type_guids() {
        use gpt::partition_types as t;
        assert_eq!(guid_to_kind(t::LINUX_LVM.guid), PartitionKind::Lvm);
        assert_eq!(guid_to_kind(t::LINUX_RAID.guid), PartitionKind::Mdadm);
        assert_eq!(guid_to_kind(t::LINUX_LUKS.guid), PartitionKind::Luks);
        assert_eq!(guid_to_kind(t::LINUX_SWAP.guid), PartitionKind::Swap);
        assert_eq!(guid_to_kind(t::BIOS.guid), PartitionKind::BiosBoot);
        assert_eq!(
            guid_to_kind(t::MACOS_APFS.guid),
            PartitionKind::ApfsContainer
        );
        assert_eq!(guid_to_kind(BITLOCKER_GUID), PartitionKind::Bitlocker);
        assert_eq!(guid_to_kind(FREEBSD_UFS_GUID), PartitionKind::Ufs);
        assert_eq!(guid_to_kind(FREEBSD_SWAP_GUID), PartitionKind::Swap);
        assert_eq!(guid_to_kind(FREEBSD_ZFS_GUID), PartitionKind::Zfs);
        assert_eq!(guid_to_kind(SOLARIS_ZFS_GUID), PartitionKind::Zfs);
        assert_eq!(guid_to_kind(MIDNIGHTBSD_ZFS_GUID), PartitionKind::Zfs);
        assert_eq!(guid_to_kind(t::BASIC.guid), PartitionKind::Unknown);
        assert_eq!(guid_to_kind(t::LINUX_FS.guid), PartitionKind::Unknown);
    }

    #[test]
    fn parse_entries_captures_unique_partition_guid() {
        // 单条 128 字节 GPT 条目：类型 GUID 在偏移 0，唯一 GUID 在偏移 16。
        let mut entry = vec![0u8; 128];
        entry[0..16].copy_from_slice(&[1u8; 16]); // 非零类型 GUID，否则条目被跳过
        let on_disk_unique = [
            0xDD, 0xCC, 0xBB, 0xAA, 0x22, 0x11, 0x44, 0x33, 0x55, 0x66, 0x77, 0x88, 0x99, 0x00,
            0x12, 0x34,
        ];
        entry[16..32].copy_from_slice(&on_disk_unique);
        entry[32..40].copy_from_slice(&100u64.to_le_bytes());
        entry[40..48].copy_from_slice(&199u64.to_le_bytes());
        let raw = GptEntry::decode(&entry, 0, 0).expect("entry");
        let partition = raw.project(0).expect("partition");
        let expected = uuid_from_disk_bytes(&on_disk_unique).unwrap();
        assert_eq!(partition.unique_guid, Some(*expected.as_bytes()));
    }

    #[test]
    fn uuid_from_disk_bytes_round_trips_through_uuid_string() {
        // BitLocker GUID on disk: data1/data2/data3 are LE, data4 is BE.
        let on_disk = [
            0x3B, 0xCA, 0xAF, 0xAE, 0xC1, 0x61, 0xE0, 0x40, 0x87, 0xA3, 0x47, 0xD1, 0xAF, 0x93,
            0xA9, 0xFC,
        ];
        let parsed = uuid_from_disk_bytes(&on_disk).expect("parse");
        assert_eq!(parsed, BITLOCKER_GUID);
    }
}
