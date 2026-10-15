//! Independent byte-built FAT fixtures, including deliberately broken chains.

use std::{io::Write, sync::Arc};

use super::*;
use crate::core::evidence::partitions::PartitionKind;

#[derive(Clone, Copy)]
enum Format {
    Fat12,
    Fat16,
    Fat32,
}

struct Fixture {
    bytes: Vec<u8>,
    format: Format,
    sector_size: usize,
    fat_sector: usize,
    root_sector: usize,
    data_sector: usize,
}

impl Fixture {
    fn new(format: Format, sector_size: usize) -> Self {
        let (clusters, reserved, fat_sectors, root_sectors) = match format {
            Format::Fat12 => (128, 1, 1, 1),
            Format::Fat16 => (5000, 1, 20, 1),
            Format::Fat32 => (65530, 32, 513, 0),
        };
        let fats = 2;
        let data_sector = reserved + fats * fat_sectors + root_sectors;
        let total_sectors = data_sector + clusters;
        let mut bytes = vec![0; total_sectors * sector_size];
        let boot = &mut bytes[..512];
        boot[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
        boot[3..11].copy_from_slice(b"MSDOS5.0");
        boot[11..13].copy_from_slice(&(sector_size as u16).to_le_bytes());
        boot[13] = 1;
        boot[14..16].copy_from_slice(&(reserved as u16).to_le_bytes());
        boot[16] = fats as u8;
        boot[21] = 0xf8;
        boot[32..36].copy_from_slice(&(total_sectors as u32).to_le_bytes());
        let root_sector = if matches!(format, Format::Fat32) {
            boot[36..40].copy_from_slice(&(fat_sectors as u32).to_le_bytes());
            boot[40..42].copy_from_slice(&0x81u16.to_le_bytes()); // only FAT #1 is authoritative
            boot[44..48].copy_from_slice(&2u32.to_le_bytes());
            boot[48..50].copy_from_slice(&1u16.to_le_bytes());
            boot[50..52].copy_from_slice(&6u16.to_le_bytes());
            boot[66] = 0x29;
            boot[67..71].copy_from_slice(&0x12345678u32.to_le_bytes());
            boot[71..82].copy_from_slice(b"AUDIT      ");
            boot[82..90].copy_from_slice(b"FAT32   ");
            data_sector
        } else {
            boot[17..19].copy_from_slice(&((sector_size / 32) as u16).to_le_bytes());
            boot[22..24].copy_from_slice(&(fat_sectors as u16).to_le_bytes());
            boot[38] = 0x29;
            boot[39..43].copy_from_slice(&0x12345678u32.to_le_bytes());
            boot[43..54].copy_from_slice(b"AUDIT      ");
            // The hint is deliberately misleading: classification must use cluster count.
            boot[54..62].copy_from_slice(b"FAT32   ");
            reserved + fats * fat_sectors
        };
        boot[510..512].copy_from_slice(&[0x55, 0xaa]);
        if matches!(format, Format::Fat32) {
            let fsinfo = &mut bytes[sector_size..2 * sector_size];
            fsinfo[..4].copy_from_slice(&0x41615252u32.to_le_bytes());
            fsinfo[484..488].copy_from_slice(&0x61417272u32.to_le_bytes());
            fsinfo[488..496].fill(0xff);
            fsinfo[508..512].copy_from_slice(&0xaa550000u32.to_le_bytes());
        }
        let mut fixture = Self {
            bytes,
            format,
            sector_size,
            fat_sector: reserved
                + if matches!(format, Format::Fat32) {
                    fat_sectors
                } else {
                    0
                },
            root_sector,
            data_sector,
        };
        fixture.set_link(0, 0xfffffff8);
        fixture.set_link(1, 0xffffffff);
        fixture.set_link(2, 0xffffffff);
        fixture.set_link(5, 8);
        fixture.set_link(8, 6);
        fixture.set_link(6, 0xffffffff);
        let entry = &mut fixture.bytes[root_sector * sector_size..root_sector * sector_size + 32];
        entry[..11].copy_from_slice(b"PAYLOAD BIN");
        entry[11] = 0x23;
        entry[13] = 123;
        entry[14..16].copy_from_slice(&((3u16 << 11) | (4 << 5) | 3).to_le_bytes());
        entry[16..18].copy_from_slice(&((44u16 << 9) | (1 << 5) | 2).to_le_bytes());
        entry[26..28].copy_from_slice(&5u16.to_le_bytes());
        entry[28..32].copy_from_slice(&1100u32.to_le_bytes());
        for (cluster, value) in [(5, 0x11), (8, 0x22), (6, 0x33)] {
            let offset = fixture.cluster_offset(cluster);
            fixture.bytes[offset..offset + sector_size].fill(value);
        }
        fixture
    }

    fn cluster_offset(&self, cluster: usize) -> usize {
        (self.data_sector + cluster - 2) * self.sector_size
    }
    fn set_link(&mut self, cluster: usize, value: u32) {
        let fat = self.fat_sector * self.sector_size;
        match self.format {
            Format::Fat32 => self.bytes[fat + cluster * 4..fat + cluster * 4 + 4]
                .copy_from_slice(&value.to_le_bytes()),
            Format::Fat16 => self.bytes[fat + cluster * 2..fat + cluster * 2 + 2]
                .copy_from_slice(&(value as u16).to_le_bytes()),
            Format::Fat12 => {
                let offset = fat + cluster * 3 / 2;
                let previous =
                    u16::from_le_bytes(self.bytes[offset..offset + 2].try_into().unwrap());
                let next = if cluster & 1 == 0 {
                    (previous & 0xf000) | (value as u16 & 0xfff)
                } else {
                    (previous & 0xf) | ((value as u16 & 0xfff) << 4)
                };
                self.bytes[offset..offset + 2].copy_from_slice(&next.to_le_bytes());
            }
        }
    }

    fn open(&self) -> (tempfile::NamedTempFile, FatVolume) {
        self.open_at(0)
    }

    fn open_at(&self, base_offset: usize) -> (tempfile::NamedTempFile, FatVolume) {
        assert!(base_offset.is_multiple_of(512));
        let mut temporary = tempfile::NamedTempFile::new().unwrap();
        temporary.write_all(&vec![0; base_offset]).unwrap();
        temporary.write_all(&self.bytes).unwrap();
        temporary.flush().unwrap();
        let image = Arc::new(DiskImage::open(temporary.path()).unwrap());
        let partition = Partition {
            id: 0,
            kind: PartitionKind::Fat12,
            start_lba: base_offset as u64 / image.sector_size as u64,
            length_lba: self.bytes.len() as u64 / image.sector_size as u64,
            label: None,
            unique_guid: None,
        };
        let volume = FatVolume::open(image, &partition).unwrap();
        (temporary, volume)
    }
}

#[test]
fn fat_variants_share_full_stream_range_and_metadata() {
    for (format, expected_format) in [
        (Format::Fat12, "FAT12"),
        (Format::Fat16, "FAT16"),
        (Format::Fat32, "FAT32"),
    ] {
        let fixture = Fixture::new(format, 512);
        let (temporary, volume) = fixture.open();
        let expected = [vec![0x11; 512], vec![0x22; 512], vec![0x33; 76]].concat();
        assert_eq!(
            volume.read_file("/payload.bin", usize::MAX).unwrap(),
            expected
        );
        for (offset, length) in [
            (0, 0),
            (0, 1),
            (510, 40),
            (1020, 300),
            (1100, 8),
            (u64::MAX, 3),
        ] {
            let start = (offset as usize).min(expected.len());
            let end = start.saturating_add(length).min(expected.len());
            assert_eq!(
                volume
                    .read_file_range("/PAYLOAD.BIN", offset, length)
                    .unwrap(),
                expected[start..end]
            );
        }
        let mut stream = Vec::new();
        assert_eq!(
            volume
                .stream_file("/PAYLOAD.BIN", 17, |bytes| {
                    stream.extend_from_slice(bytes);
                    Ok(())
                })
                .unwrap(),
            1100
        );
        assert_eq!(stream, expected);
        assert_eq!(volume.volume_metadata().unwrap().format, expected_format);
        let metadata = volume.file_metadata("/pAyLoAd.BiN").unwrap();
        assert_eq!(
            (metadata.attributes, metadata.size, metadata.entry_offset),
            (0x23, 1100, (fixture.root_sector * 512) as u64)
        );
        assert_eq!(
            metadata.timestamps.created.as_deref(),
            Some("2024-01-02T03:04:07.230")
        );
        let stat = volume.file_stat("/payload.bin", "test").unwrap();
        assert_eq!(stat.data_runs.len(), 3);
        assert_eq!(
            stat.data_runs.iter().map(|run| run.length).sum::<u64>(),
            1100
        );
        assert_eq!(volume.children("").unwrap()[0].inode, stat.inode);
        assert_eq!(
            std::fs::read(temporary.path()).unwrap(),
            fixture.bytes,
            "read-only access must preserve every byte"
        );
    }
}

#[test]
fn fat_rejects_cycles_short_chains_and_bad_cluster_numbers() {
    for next in [5, 0xffff, 0xfff7, 1, 9000] {
        let mut fixture = Fixture::new(Format::Fat16, 512);
        fixture.set_link(8, next);
        let (_temporary, volume) = fixture.open();
        assert!(volume.read_file("/PAYLOAD.BIN", 4096).is_err());
        assert!(volume.read_file_range("/PAYLOAD.BIN", 513, 1).is_err());
        assert!(volume.stream_file("/PAYLOAD.BIN", 512, |_| Ok(())).is_err());
    }
}

#[test]
fn fat12_reads_nonstandard_large_logical_sectors() {
    let fixture = Fixture::new(Format::Fat12, 8192);
    let (_temporary, volume) = fixture.open();
    assert_eq!(
        volume.read_file("/PAYLOAD.BIN", usize::MAX).unwrap(),
        vec![0x11; 1100]
    );
    assert_eq!(volume.volume_metadata().unwrap().cluster_size, 8192);
}

#[test]
fn fat_keeps_same_name_deleted_records_with_distinct_ids() {
    let mut fixture = Fixture::new(Format::Fat12, 512);
    let start = fixture.root_sector * 512;
    let mut short: [u8; 32] = fixture.bytes[start..start + 32].try_into().unwrap();
    short[0] = 0xe5;
    let mut long = [0u8; 32];
    long[0] = 0xe5;
    long[11] = 0x0f;
    let units: Vec<u16> = "PAYLOAD.BIN".encode_utf16().collect();
    for (index, offset) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
        .into_iter()
        .enumerate()
    {
        long[offset..offset + 2]
            .copy_from_slice(&units.get(index).copied().unwrap_or(0).to_le_bytes());
    }
    for (index, entry) in [long, short, long, short].into_iter().enumerate() {
        fixture.bytes[start + 32 + index * 32..start + 64 + index * 32].copy_from_slice(&entry);
    }
    let (_temporary, volume) = fixture.open();
    let nodes = volume.children_with_deleted("").unwrap();
    assert_eq!(
        nodes
            .iter()
            .map(|node| (node.name.as_str(), node.deleted))
            .collect::<Vec<_>>(),
        vec![
            ("PAYLOAD.BIN", false),
            ("PAYLOAD.BIN", true),
            ("PAYLOAD.BIN", true)
        ]
    );
    assert_ne!(nodes[1].id, nodes[2].id);
    assert_eq!(
        crate::core::filesystem::deleted::parse_run_id(&nodes[1].id),
        Some((fixture.cluster_offset(5) as u64, 1100))
    );
}

#[test]
fn fat_deleted_metadata_uses_absolute_fixed_root_offsets() {
    for format in [Format::Fat12, Format::Fat16] {
        for sector_size in [512, 8192] {
            let mut fixture = Fixture::new(format, sector_size);
            let root_offset = fixture.root_sector * sector_size;
            let mut entry: [u8; 32] = fixture.bytes[root_offset..root_offset + 32]
                .try_into()
                .unwrap();
            entry[0] = 0xe5;
            fixture.bytes[root_offset + 32..root_offset + 64].copy_from_slice(&entry);
            // A zero-length deleted directory still has a metadata address.
            entry[11] = 0x10;
            entry[26..32].fill(0);
            fixture.bytes[root_offset + 64..root_offset + 96].copy_from_slice(&entry);
            let base_offset = 4 * 512;
            let (temporary, volume) = fixture.open_at(base_offset);
            let nodes = volume.children_with_deleted("").unwrap();
            assert_eq!(
                nodes
                    .iter()
                    .map(|node| (node.deleted, node.inode))
                    .collect::<Vec<_>>(),
                vec![
                    (false, Some((base_offset + root_offset) as u64)),
                    (true, Some((base_offset + root_offset + 32) as u64)),
                    (true, Some((base_offset + root_offset + 64) as u64)),
                ]
            );
            let root_id = hex::encode("part:0");
            assert_eq!(nodes[2].id, format!("deleted:fat:part:0:{root_id}:64"));
            assert_eq!(
                std::fs::read(temporary.path()).unwrap(),
                [vec![0; base_offset], fixture.bytes].concat()
            );
        }
    }
}

#[test]
fn fat_deleted_metadata_follows_fragmented_directory_clusters() {
    for format in [Format::Fat12, Format::Fat16, Format::Fat32] {
        let mut fixture = Fixture::new(format, 512);
        let root_offset = fixture.root_sector * 512;
        let mut entry: [u8; 32] = fixture.bytes[root_offset..root_offset + 32]
            .try_into()
            .unwrap();
        // The directory traverses a later physical cluster before an earlier one.
        fixture.set_link(9, 3);
        fixture.set_link(3, 0xffffffff);
        let path = if matches!(format, Format::Fat32) {
            fixture.set_link(2, 9);
            ""
        } else {
            let mut directory = [0u8; 32];
            directory[..11].copy_from_slice(b"FOLDER     ");
            directory[11] = 0x10;
            directory[26..28].copy_from_slice(&9u16.to_le_bytes());
            fixture.bytes[root_offset + 32..root_offset + 64].copy_from_slice(&directory);
            "/FOLDER"
        };
        entry[0] = 0xe5;
        let first_offset = fixture.cluster_offset(9) + 32;
        let second_offset = fixture.cluster_offset(3) + 64;
        fixture.bytes[first_offset..first_offset + 32].copy_from_slice(&entry);
        fixture.bytes[second_offset..second_offset + 32].copy_from_slice(&entry);
        let base_offset = 4 * 512;
        let (_temporary, volume) = fixture.open_at(base_offset);
        let nodes: Vec<_> = volume
            .children_with_deleted(path)
            .unwrap()
            .into_iter()
            .filter(|node| node.deleted)
            .collect();
        assert_eq!(
            nodes
                .iter()
                .map(|node| (node.name.as_str(), node.inode))
                .collect::<Vec<_>>(),
            vec![
                ("_AYLOAD.BIN", Some((base_offset + first_offset) as u64)),
                ("_AYLOAD.BIN", Some((base_offset + second_offset) as u64)),
            ]
        );
        let preceding_bytes = if matches!(format, Format::Fat32) {
            512
        } else {
            0
        };
        let parent_id = hex::encode(volume.node_id(path));
        let data_offset = base_offset + fixture.cluster_offset(5);
        assert_eq!(
            nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>(),
            [preceding_bytes + 32, preceding_bytes + 512 + 64].map(|slot_offset| {
                format!("deleted:run:part:0:fat:{parent_id}:{slot_offset}:0x{data_offset:x}:0x44c")
            })
        );
    }
}
