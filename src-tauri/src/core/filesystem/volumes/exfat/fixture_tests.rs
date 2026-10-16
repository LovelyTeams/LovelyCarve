use std::{io::Write, sync::Arc};

use super::*;
use crate::core::evidence::partitions::PartitionKind;

const NAME: &str = "ABCDEFGHIJKLMN馃榾.bin";
const HEAP: usize = 26;

fn fixture(contiguous: bool, valid_length: u64) -> Vec<u8> {
    let mut bytes = vec![0; (HEAP + 128) * 512];
    let boot = &mut bytes[..512];
    boot[..3].copy_from_slice(&[0xeb, 0x76, 0x90]);
    boot[3..11].copy_from_slice(b"EXFAT   ");
    boot[72..80].copy_from_slice(&((HEAP + 128) as u64).to_le_bytes());
    boot[80..84].copy_from_slice(&24u32.to_le_bytes());
    boot[84..88].copy_from_slice(&2u32.to_le_bytes());
    boot[88..92].copy_from_slice(&(HEAP as u32).to_le_bytes());
    boot[92..96].copy_from_slice(&128u32.to_le_bytes());
    boot[96..100].copy_from_slice(&2u32.to_le_bytes());
    boot[100..104].copy_from_slice(&0x12345678u32.to_le_bytes());
    boot[104..106].copy_from_slice(&0x100u16.to_le_bytes());
    boot[108] = 9;
    boot[110] = 1;
    boot[510..512].copy_from_slice(&[0x55, 0xaa]);
    for (cluster, next) in [
        (0, 0xfffffff8u32),
        (1, 0xffffffff),
        (2, 0xffffffff),
        (3, 0xffffffff),
        (4, 0xffffffff),
        (5, 8),
        (8, 6),
        (6, 0xffffffff),
    ] {
        let offset = 24 * 512 + cluster * 4;
        bytes[offset..offset + 4].copy_from_slice(&next.to_le_bytes());
    }
    let root = &mut bytes[HEAP * 512..(HEAP + 1) * 512];
    // Required bitmap and upcase records.
    for (slot, kind, cluster, length) in [(0, 0x81, 3u32, 16u64), (1, 0x82, 4, 2)] {
        let entry = &mut root[slot * 32..slot * 32 + 32];
        entry[0] = kind;
        entry[20..24].copy_from_slice(&cluster.to_le_bytes());
        entry[24..32].copy_from_slice(&length.to_le_bytes());
    }
    root[64] = 0x83;
    root[65] = 2;
    root[66..70].copy_from_slice(&[b'L', 0, b'C', 0]);
    let file = &mut root[96..128];
    file[0] = 0x85;
    file[1] = 3;
    file[4..6].copy_from_slice(&0x23u16.to_le_bytes());
    let time = (44u32 << 25) | (1 << 21) | (2 << 16) | (3 << 11) | (4 << 5) | 3;
    file[8..12].copy_from_slice(&time.to_le_bytes());
    file[20] = 123;
    file[22] = 0xec; // valid UTC-5
    let name: Vec<u16> = NAME.encode_utf16().collect();
    let stream = &mut root[128..160];
    stream[0] = 0xc0;
    stream[1] = if contiguous { 3 } else { 1 };
    stream[3] = name.len() as u8;
    stream[8..16].copy_from_slice(&valid_length.to_le_bytes());
    stream[20..24].copy_from_slice(&5u32.to_le_bytes());
    stream[24..32].copy_from_slice(&1100u64.to_le_bytes());
    for (index, chunk) in name.chunks(15).enumerate() {
        let slot = &mut root[160 + index * 32..192 + index * 32];
        slot[0] = 0xc1;
        for (index, unit) in chunk.iter().enumerate() {
            slot[2 + index * 2..4 + index * 2].copy_from_slice(&unit.to_le_bytes());
        }
    }
    for (cluster, value) in if contiguous {
        [(5, 0x11), (6, 0x22), (7, 0x33)]
    } else {
        [(5, 0x11), (8, 0x22), (6, 0x33)]
    } {
        bytes[(HEAP + cluster - 2) * 512..(HEAP + cluster - 1) * 512].fill(value);
    }
    bytes
}

fn open(bytes: &[u8]) -> (tempfile::NamedTempFile, Result<ExfatVolume>) {
    let mut temporary = tempfile::NamedTempFile::new().unwrap();
    temporary.write_all(bytes).unwrap();
    temporary.flush().unwrap();
    let image = Arc::new(DiskImage::open(temporary.path()).unwrap());
    let partition = Partition {
        id: 0,
        kind: PartitionKind::ExFat,
        start_lba: 0,
        length_lba: bytes.len() as u64 / image.sector_size as u64,
        label: None,
        unique_guid: None,
    };
    (temporary, ExfatVolume::open(image, &partition))
}

#[test]
fn exfat_fragmented_contiguous_and_uninitialized_reads_match() {
    for contiguous in [false, true] {
        for valid_length in [0, 700, 1100] {
            let bytes = fixture(contiguous, valid_length);
            let (temporary, volume) = open(&bytes);
            let volume = volume.unwrap();
            let mut expected = [vec![0x11; 512], vec![0x22; 512], vec![0x33; 76]].concat();
            expected[valid_length as usize..].fill(0);
            let path = format!("/{NAME}");
            assert_eq!(volume.read_file(&path, usize::MAX).unwrap(), expected);
            for (offset, length) in [
                (0, 0),
                (510, 40),
                (690, 50),
                (1020, 300),
                (1100, 1),
                (u64::MAX, 5),
            ] {
                let start = (offset as usize).min(expected.len());
                let end = start.saturating_add(length).min(expected.len());
                assert_eq!(
                    volume.read_file_range(&path, offset, length).unwrap(),
                    expected[start..end]
                );
            }
            let mut stream = Vec::new();
            assert_eq!(
                volume
                    .stream_file(&path, 512, |chunk| {
                        stream.extend_from_slice(chunk);
                        Ok(())
                    })
                    .unwrap(),
                1100
            );
            assert_eq!(stream, expected);
            let metadata = volume.file_metadata(&path).unwrap();
            assert_eq!(
                (
                    metadata.attributes,
                    metadata.entry_offset,
                    metadata.size,
                    metadata.valid_data_length
                ),
                (0x23, (HEAP * 512 + 96) as u64, 1100, valid_length)
            );
            assert_eq!(
                metadata.timestamps.created.as_deref(),
                Some("2024-01-02T08:04:07.230+00:00")
            );
            assert_eq!(
                volume.volume_metadata().unwrap(),
                ExfatVolumeMetadata {
                    label: Some("LC".into()),
                    serial_number: 0x12345678,
                    cluster_size: 512
                }
            );
            let stat = volume.file_stat(&path, "test").unwrap();
            assert_eq!(stat.data_runs.len(), if contiguous { 1 } else { 3 });
            assert_eq!(
                stat.data_runs.iter().map(|run| run.length).sum::<u64>(),
                1100
            );
            assert_eq!(volume.children("").unwrap()[0].inode, stat.inode);
            assert_eq!(std::fs::read(temporary.path()).unwrap(), bytes);
        }
    }
}

#[test]
fn exfat_rejects_cyclic_bad_short_and_out_of_bounds_chains() {
    for next in [5, 0xffffffffu32, 0xfffffff7, 0, 5000] {
        let mut bytes = fixture(false, 1100);
        bytes[24 * 512 + 8 * 4..24 * 512 + 8 * 4 + 4].copy_from_slice(&next.to_le_bytes());
        let (_temporary, volume) = open(&bytes);
        assert!(volume.is_err());
    }
    let mut bytes = fixture(true, 1100);
    bytes[HEAP * 512 + 148..HEAP * 512 + 152].copy_from_slice(&129u32.to_le_bytes());
    let (_temporary, volume) = open(&bytes);
    assert!(volume.is_err());
}

#[test]
fn exfat_deleted_records_keep_live_names_and_timestamp_precision() {
    let mut bytes = fixture(true, 1100);
    let start = HEAP * 512;
    let mut deleted = bytes[start + 96..start + 224].to_vec();
    for entry in deleted.chunks_exact_mut(32) {
        entry[0] &= 0x7f;
    }
    bytes[start + 224..start + 352].copy_from_slice(&deleted);
    bytes[start + 352..start + 480].copy_from_slice(&deleted);
    let (_temporary, volume) = open(&bytes);
    let volume = volume.unwrap();
    let children = volume.children_with_deleted("").unwrap();
    assert_eq!(children.len(), 3);
    assert!(children[1].deleted && children[2].deleted);
    assert_eq!(
        children[0].timestamps.created,
        children[1].timestamps.created
    );
    assert_ne!(children[1].id, children[2].id);
}

#[test]
fn exfat_reads_backup_boot_without_modifying_primary() {
    let mut bytes = fixture(true, 1100);
    let boot: [u8; 512] = bytes[..512].try_into().unwrap();
    bytes[12 * 512..13 * 512].copy_from_slice(&boot);
    bytes[..512].fill(0);
    let (temporary, volume) = open(&bytes);
    let volume = volume.unwrap();
    assert_eq!(
        volume.read_file(&format!("/{NAME}"), 1).unwrap(),
        vec![0x11]
    );
    assert_eq!(volume.volume_metadata().unwrap().serial_number, 0x12345678);
    assert_eq!(std::fs::read(temporary.path()).unwrap(), bytes);
}

#[test]
fn exfat_no_fat_chain_directory_keeps_deleted_children() {
    let mut bytes = fixture(true, 1100);
    let root = HEAP * 512;
    let mut directory = bytes[root + 96..root + 224].to_vec();
    directory[4..6].copy_from_slice(&0x10u16.to_le_bytes());
    directory[40..48].copy_from_slice(&512u64.to_le_bytes());
    directory[52..56].copy_from_slice(&12u32.to_le_bytes());
    directory[56..64].copy_from_slice(&512u64.to_le_bytes());
    let mut deleted = bytes[root + 96..root + 224].to_vec();
    for entry in deleted.chunks_exact_mut(32) {
        entry[0] &= 0x7f;
    }
    bytes[root + 96..root + 224].copy_from_slice(&directory);
    let child_directory = (HEAP + 10) * 512;
    bytes[child_directory..child_directory + 128].copy_from_slice(&deleted);
    // FAT[12] is zero: traversal must use NoFatChain and directory DataLength.
    let (_temporary, volume) = open(&bytes);
    let volume = volume.unwrap();
    let nodes = volume.children_with_deleted(&format!("/{NAME}")).unwrap();
    assert_eq!(nodes.len(), 1);
    assert!(nodes[0].deleted);
    assert_eq!(nodes[0].name, NAME);
}

#[test]
fn recovery_exfat_deleted_contiguous_volume_exports_logical_bytes() {
    use crate::core::filesystem::{
        evidence_session::EvidenceSession,
        recovery::{self, RecoveryOptions, SectorRange},
    };
    use tokio_util::sync::CancellationToken;
    for valid_length in [0, 700, 1100] {
        let mut bytes = fixture(true, valid_length);
        for slot in [96, 128, 160, 192] {
            bytes[HEAP * 512 + slot] &= 0x7f;
        }
        let mut temporary = tempfile::NamedTempFile::new().unwrap();
        temporary.write_all(&bytes).unwrap();
        temporary.flush().unwrap();
        let image = Arc::new(EvidenceSession::open(temporary.path()).unwrap());
        let token = CancellationToken::new();
        let options = RecoveryOptions {
            fat: false,
            ntfs: false,
            exfat: true,
            include_existing: false,
        };
        let ranges = vec![SectorRange {
            start_sector: 0,
            sector_count: image.len() / image.sector_size as u64,
        }];
        let scanned = recovery::scan(
            &image,
            "scan".to_string(),
            options,
            ranges,
            &token,
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(scanned.candidates.len(), 1, "{scanned:#?}");
        assert!(scanned.candidates[0].readable);
        let destination = tempfile::tempdir().unwrap();
        let exported = recovery::export::recover(
            &image,
            &scanned,
            &[scanned.candidates[0].id.clone()],
            destination.path(),
            &token,
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(exported.recovered, 1);
        assert_eq!(exported.failed, 0);
        let outputs = recovery::tests::exported_files(&exported.directory);
        let folder = scanned.volumes[0]
            .partition
            .label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or("exFAT")
            .to_string();
        assert_eq!(
            outputs
                .iter()
                .map(|(relative, _)| relative.as_str())
                .collect::<Vec<_>>(),
            vec![format!("v0-{folder}/{NAME}")]
        );
        let mut expected = [vec![0x11; 512], vec![0x22; 512], vec![0x33; 76]].concat();
        expected[valid_length as usize..].fill(0);
        assert_eq!(std::fs::read(&outputs[0].1).unwrap(), expected);
    }
}
