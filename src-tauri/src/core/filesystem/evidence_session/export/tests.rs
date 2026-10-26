use super::*;
use crate::core::evidence::crypto::encrypted_image::{self, EncryptionScheme, UnlockOptions};
use crate::core::evidence::image::{DiskImage, Partition, PartitionKind};
use crate::core::evidence::partitions::SectorSource;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/encrypted")
        .join(name)
}

#[test]
fn raw_export_projects_multiple_unlocked_partitions_and_preserves_disk_layout() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("disk.raw");
    let plaintext = std::fs::read(fixture("plain.img")).unwrap();
    let first = std::fs::read(fixture("luks1.img")).unwrap();
    let second = std::fs::read(fixture("luks2.img")).unwrap();
    let second_start = 512 + first.len() + 512;
    let mut original = vec![0x5a; 512];
    original.extend_from_slice(&first);
    original.extend_from_slice(&[0x77; 512]);
    original.extend_from_slice(&second);
    original.extend_from_slice(&[0x33; 512]);
    std::fs::write(&path, &original).unwrap();
    let mut disk = DiskImage::open(&path).unwrap();
    disk.partitions = [(2, 512, first.len()), (7, second_start, second.len())]
        .into_iter()
        .map(|(id, start, length)| Partition {
            id,
            kind: PartitionKind::Luks,
            start_lba: start as u64 / 512,
            length_lba: length as u64 / 512,
            label: None,
            unique_guid: None,
            amiga_geometry: None,
        })
        .collect();
    let source = Arc::new(EvidenceSession::from(disk));
    let before_unlock = source
        .raw_export_reader(ImageExportMode::UnlockedPartitions)
        .unwrap();
    for id in [2, 7] {
        let (plain, result) = encrypted_image::unlock(
            source.disk_image(),
            UnlockOptions {
                target: Default::default(),
                scheme: EncryptionScheme::Luks,
                password: "lovelycarve-test".into(),
                master_key_hex: None,
                logical_volume_index: None,
                encrypted_root_plist_path: None,
                partition_id: Some(id),
                offset: None,
                length: None,
                hidden: false,
                pim: 0,
            },
            CancellationToken::new(),
        )
        .unwrap();
        source
            .attach_decrypted_partition(id, plain, result.read_cancellation)
            .unwrap();
    }
    let reader = source
        .raw_export_reader(ImageExportMode::UnlockedPartitions)
        .unwrap();
    let mut expected = original.clone();
    for (start, length) in [(512, first.len()), (second_start, second.len())] {
        expected[start..start + length].fill(0);
        expected[start..start + plaintext.len()].copy_from_slice(&plaintext);
    }
    let mut exported = vec![0; original.len()];
    for (index, chunk) in exported.chunks_mut(997).enumerate() {
        assert_eq!(
            reader.read_into((index * 997) as u64, chunk).unwrap(),
            chunk.len()
        );
    }
    assert_eq!(exported, expected);
    // Random, crossing-boundary reads match sequential output.
    for offset in [
        0,
        507,
        512 + plaintext.len() - 3,
        second_start - 4,
        original.len() - 3,
    ] {
        let mut chunk = vec![0; 1031];
        let count = reader.read_into(offset as u64, &mut chunk).unwrap();
        assert_eq!(&chunk[..count], &expected[offset..offset + count]);
    }
    let raw_reader = source.raw_export_reader(ImageExportMode::Source).unwrap();
    assert_eq!(
        raw_reader.read_into(0, &mut exported).unwrap(),
        original.len()
    );
    assert_eq!(exported, original);
    assert_eq!(
        before_unlock.read_into(0, &mut exported).unwrap(),
        original.len()
    );
    assert_eq!(
        exported, original,
        "unlock during export cannot change its byte projection"
    );
    assert_eq!(source.read_range(0, original.len()).unwrap(), original);
    source.revoke_decrypted_partitions();
    assert!(reader.read_into(512, &mut [0; 512]).is_err());
    assert!(source
        .raw_export_reader(ImageExportMode::UnlockedPartitions)
        .is_err());
    assert_eq!(
        raw_reader.read_into(0, &mut exported).unwrap(),
        original.len()
    );
}

#[test]
fn raw_export_handles_whole_containers_without_partition_metadata() {
    let mut disk = DiskImage::open(fixture("veracrypt.hc")).unwrap();
    disk.partitions.clear();
    let source = Arc::new(EvidenceSession::from(disk));
    let plain = DiskImage::open(fixture("plain.img")).unwrap();
    let expected = plain.read_range(0, plain.len() as usize).unwrap();
    source
        .attach_decrypted_partition(0, plain, CancellationToken::new())
        .unwrap();
    let reader = source
        .raw_export_reader(ImageExportMode::UnlockedPartitions)
        .unwrap();
    let mut output = vec![0x55; source.len() as usize];
    assert_eq!(reader.read_into(0, &mut output).unwrap(), output.len());
    assert_eq!(&output[..expected.len()], expected);
    assert!(output[expected.len()..].iter().all(|byte| *byte == 0));
}

#[test]
fn raw_export_rejects_invalid_plaintext_geometry() {
    for (start_lba, length_lba) in [(u64::MAX, 1), (0, u64::MAX), (1, 1), (0, 1)] {
        let mut disk = DiskImage::open(fixture("plain.img")).unwrap();
        disk.partitions[0].start_lba = start_lba;
        disk.partitions[0].length_lba = length_lba;
        let source = Arc::new(EvidenceSession::from(disk));
        source
            .attach_decrypted_partition(
                0,
                DiskImage::open(fixture("plain.img")).unwrap(),
                CancellationToken::new(),
            )
            .unwrap();
        assert!(source
            .raw_export_reader(ImageExportMode::UnlockedPartitions)
            .is_err());
    }
    let mut disk = DiskImage::open(fixture("plain.img")).unwrap();
    let mut duplicate = disk.partitions[0].clone();
    duplicate.id = 1;
    disk.partitions.push(duplicate);
    let source = Arc::new(EvidenceSession::from(disk));
    for id in [0, 1] {
        source
            .attach_decrypted_partition(
                id,
                DiskImage::open(fixture("plain.img")).unwrap(),
                CancellationToken::new(),
            )
            .unwrap();
    }
    assert!(source
        .raw_export_reader(ImageExportMode::UnlockedPartitions)
        .is_err());
}

struct ShortSource;
impl SectorSource for ShortSource {
    fn size(&self) -> u64 {
        4096
    }
    fn read(&self, _offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        Ok(vec![0x55; length.saturating_sub(1)])
    }
}

#[test]
fn raw_export_never_completes_a_short_source_read() {
    let disk =
        DiskImage::from_assembled("short".into(), Arc::new(ShortSource), Vec::new(), 512).unwrap();
    let source = Arc::new(EvidenceSession::from(disk));
    for mode in [ImageExportMode::Source, ImageExportMode::UnlockedPartitions] {
        let reader = source.raw_export_reader(mode).unwrap();
        assert!(reader.read_into(0, &mut [0; 4096]).is_err());
    }
}
