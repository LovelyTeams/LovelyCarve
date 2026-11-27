use super::*;
use crate::core::evidence::image::PartitionKind;
use crate::core::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{self, FsSession},
};
use sha2::{Digest, Sha256};

const CRAM_LE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test_data/rom-images/synthetic-le.cramfs"
));
const CRAM_BE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test_data/rom-images/synthetic-be.cramfs"
));
const CRAM_EXT_LE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test_data/rom-images/native-extended-le.cramfs"
));
const CRAM_EXT_BE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test_data/rom-images/native-extended-be.cramfs"
));
const CRAM_V1: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test_data/rom-images/native-v1.cramfs"
));
const ROM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test_data/rom-images/synthetic.romfs"
));
const PAYLOAD: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test_data/rom-images/payload.bin"
));

fn image(bytes: &[u8]) -> (tempfile::TempDir, Arc<EvidenceSession>) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.bin");
    std::fs::write(&path, bytes).unwrap();
    (
        directory,
        Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap())),
    )
}

fn refresh_cram_crc(bytes: &mut [u8]) {
    let offset = lamfold_cramfs::superblock_offset(bytes).unwrap();
    bytes[offset + 32..offset + 36].fill(0);
    let crc = crc32fast::hash(&bytes[offset..]);
    bytes[offset + 32..offset + 36].copy_from_slice(&crc.to_le_bytes());
}

fn refresh_rom_checksum(bytes: &mut [u8], offset: usize, length: usize) {
    bytes[offset + 12..offset + 16].fill(0);
    let sum = bytes[offset..offset + length]
        .chunks_exact(4)
        .fold(0u32, |sum, word| {
            sum.wrapping_add(u32::from_be_bytes(word.try_into().unwrap()))
        });
    bytes[offset + 12..offset + 16].copy_from_slice(&sum.wrapping_neg().to_be_bytes());
}

#[test]
fn rom_images_dispatch_preserves_content_ranges_metadata_and_source() {
    for (fixture, kind, prefix) in [
        (CRAM_LE, PartitionKind::Cramfs, "cramfs"),
        (CRAM_BE, PartitionKind::Cramfs, "cramfs"),
        (ROM, PartitionKind::Romfs, "romfs"),
    ] {
        let (_directory, evidence) = image(fixture);
        assert_eq!(evidence.partitions[0].kind, kind);
        assert_eq!(evidence.sector_size, 1);
        assert_eq!(evidence.read_range(0, fixture.len()).unwrap(), fixture);
        let mut session = FsSession::new(&evidence);
        let root = session.children("part:0").unwrap();
        let file = root.iter().find(|node| node.name == "payload.bin").unwrap();
        assert_eq!(file.size, PAYLOAD.len() as u64);
        assert!(file.id.starts_with(prefix));
        assert_eq!(
            session.read_file_limited(&file.id, usize::MAX).unwrap(),
            PAYLOAD
        );
        assert_eq!(
            session.read_file_limited(&file.id, 19).unwrap(),
            PAYLOAD[..19]
        );
        for (offset, length) in [
            (0, 0),
            (4090, 40),
            (8180, 100),
            (PAYLOAD.len() - 3, 200),
            (PAYLOAD.len(), 30),
        ] {
            assert_eq!(
                session
                    .read_file_range(&file.id, offset as u64, length)
                    .unwrap(),
                PAYLOAD[offset..offset.saturating_add(length).min(PAYLOAD.len())]
            );
        }
        assert!(session.read_file_range(&file.id, u64::MAX, 9).is_err());
        let stat = session.file_stat(&file.id).unwrap();
        assert_eq!(stat.size, file.size);
        assert!(stat.mode.is_some());
        assert!(stat.data_runs.is_empty());
        assert!(file.timestamps.modified.is_none());
        let mut streamed = Vec::new();
        assert_eq!(
            fs_tree::stream_file_chunks(&evidence, &file.id, 4099, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap(),
            PAYLOAD.len() as u64
        );
        assert_eq!(streamed, PAYLOAD);
        assert!(
            fs_tree::stream_file_chunks(&evidence, &file.id, 4096, |_| anyhow::bail!(
                "cancel test"
            ))
            .is_err()
        );
        let nested = root.iter().find(|node| node.name == "nested").unwrap();
        let note = session.children(&nested.id).unwrap();
        assert_eq!(
            session.read_file_limited(&note[0].id, 100).unwrap(),
            b"LovelyCarve ROM image fixture.\n"
        );
        let link = root.iter().find(|node| node.name == "link").unwrap();
        assert_eq!(link.kind, NodeKind::Symlink);
        assert_eq!(link.link_target.as_deref(), Some("nested/note.txt"));
        assert_eq!(session.read_file_range(&link.id, 2, 4).unwrap(), b"sted");
        let empty = root.iter().find(|node| node.name == "empty").unwrap();
        assert!(session
            .read_file_limited(&empty.id, 100)
            .unwrap()
            .is_empty());
        let pipe = root.iter().find(|node| node.name == "pipe").unwrap();
        assert!(session.read_file_limited(&pipe.id, 1).is_err());
        if kind == PartitionKind::Romfs {
            let alias = root.iter().find(|node| node.name == "alias.bin").unwrap();
            assert_ne!(alias.id, file.id);
            assert_eq!(
                session.read_file_limited(&alias.id, usize::MAX).unwrap(),
                PAYLOAD
            );
        }
    }
}

#[test]
fn rom_images_reject_crc_truncation_outside_extents_and_partial_decodes() {
    for fixture in [CRAM_LE, CRAM_BE, ROM] {
        for end in [fixture.len() - 1, 64] {
            let (_directory, evidence) = image(&fixture[..end]);
            assert!(FsSession::new(&evidence).children("part:0").is_err());
        }
        let mut bytes = fixture.to_vec();
        bytes[12] ^= 1;
        let (_directory, evidence) = image(&bytes);
        assert!(FsSession::new(&evidence).children("part:0").is_err());
    }
    // Root's payload.bin inode at 96; its data table is relative to the FS.
    for mutation in 0..4 {
        let mut bytes = CRAM_LE.to_vec();
        match mutation {
            0 => {
                let data =
                    (u32::from_le_bytes(bytes[104..108].try_into().unwrap()) >> 6) as usize * 4;
                bytes[data..data + 4].copy_from_slice(&0u32.to_le_bytes());
            }
            1 => {
                let size = u32::from_le_bytes(bytes[100..104].try_into().unwrap());
                bytes[100..104].copy_from_slice(&(size + 1).to_le_bytes());
            }
            2 => bytes[8..12].copy_from_slice(&1u32.to_le_bytes()),
            3 => bytes[8..12].copy_from_slice(&0x201u32.to_le_bytes()),
            _ => unreachable!(),
        }
        refresh_cram_crc(&mut bytes);
        let (_directory, evidence) = image(&bytes);
        let mut session = FsSession::new(&evidence);
        assert!(session
            .read_file_limited(
                &format!("cramfs:part:0:{}", hex::encode("/payload.bin")),
                usize::MAX
            )
            .is_err());
    }
}

#[test]
fn rom_images_cramfs_native_extended_pointers_and_boot_padding() {
    for fixture in [CRAM_EXT_LE, CRAM_EXT_BE] {
        for base in [0, 512] {
            let mut disk = vec![0; base];
            if base != 0 {
                disk[450] = 0x83;
                disk[454..458].copy_from_slice(&1u32.to_le_bytes());
                disk[458..462].copy_from_slice(&((fixture.len() / 512) as u32).to_le_bytes());
                disk[510..512].copy_from_slice(&[0x55, 0xaa]);
            }
            disk.extend_from_slice(fixture);
            // Boot bytes have no effect on the filesystem checksum.
            disk[base + 17] = 0x7a;
            let (_directory, evidence) = image(&disk);
            assert_eq!(evidence.partitions[0].kind, PartitionKind::Cramfs);
            assert_eq!(
                evidence.partitions[0].start_lba * evidence.sector_size as u64,
                base as u64
            );
            assert_eq!(evidence.read_range(0, disk.len()).unwrap(), disk);
            let mut session = FsSession::new(&evidence);
            let root = session.children("part:0").unwrap();
            for (name, hash) in [
                (
                    "payload.bin",
                    "d3e976619a6b7499471669dc3c587da9a4685b350a13bcd8ff871ad227fd68c6",
                ),
                (
                    "program",
                    "4777fda08bd4c1b7ec65be151cfb1ba562bcdde8e696303a91d59342b88e28dc",
                ),
            ] {
                let file = root.iter().find(|node| node.name == name).unwrap();
                let whole = session.read_file_limited(&file.id, usize::MAX).unwrap();
                assert_eq!(hex::encode(Sha256::digest(&whole)), hash);
                for offset in [4091, 8185, 8197, 12_280, 12_289] {
                    assert_eq!(
                        session
                            .read_file_range(&file.id, offset as u64, 45)
                            .unwrap(),
                        whole[offset..(offset + 45).min(whole.len())]
                    );
                }
            }
            let link = root.iter().find(|node| node.name == "link").unwrap();
            assert_eq!(link.link_target.as_deref(), Some("nested/note"));
        }
    }
}

#[test]
fn rom_images_cramfs_fsid_v1_uses_source_boundary() {
    let historical_size = u32::from_le_bytes(CRAM_V1[4..8].try_into().unwrap());
    assert_eq!(historical_size, 65_536);
    assert!(CRAM_V1.len() < historical_size as usize);
    let file = format!("cramfs:part:0:{}", hex::encode("/payload.bin"));
    let (_directory, evidence) = image(CRAM_V1);
    assert_eq!(evidence.partitions[0].kind, PartitionKind::Cramfs);
    let mut session = FsSession::new(&evidence);
    let full = session.read_file_limited(&file, usize::MAX).unwrap();
    assert_eq!(
        hex::encode(Sha256::digest(&full)),
        "d3e976619a6b7499471669dc3c587da9a4685b350a13bcd8ff871ad227fd68c6"
    );
    assert_eq!(
        session.read_file_range(&file, 8192, 4096).unwrap(),
        vec![0; 4096]
    );
    assert_eq!(
        session.read_file_range(&file, 4093, 29).unwrap(),
        full[4093..4122]
    );

    let (_directory, truncated) = image(&CRAM_V1[..4096]);
    assert!(FsSession::new(&truncated)
        .read_file_limited(&file, usize::MAX)
        .is_err());
    assert!(FsSession::new(&truncated)
        .read_file_range(&file, 4100, 1)
        .is_err());
}

#[test]
fn rom_images_cramfs_direct_zlib_before_sequential_raw() {
    let mut bytes = CRAM_EXT_LE.to_vec();
    let inode = bytes.windows(11).position(|s| s == b"payload.bin").unwrap() - 12;
    let word = u32::from_le_bytes(bytes[inode + 8..inode + 12].try_into().unwrap());
    let table = ((word >> 6) * 4) as usize;
    let compressed_end = u32::from_le_bytes(bytes[table..table + 4].try_into().unwrap()) as usize;
    let old_end =
        u32::from_le_bytes(bytes[table + 12..table + 16].try_into().unwrap()) & 0x3fff_ffff;

    // Move the same stream away from the table and give its zlib page a
    // length-prefixed direct pointer. The next raw page uses a regular end pointer.
    let start = bytes.len();
    let body = bytes[table + 16..old_end as usize].to_vec();
    let compressed_len = compressed_end - (table + 16);
    bytes.extend_from_slice(&(compressed_len as u16).to_le_bytes());
    bytes.extend_from_slice(&body);
    let raw_end = (start + 2 + compressed_len + 4096) as u32;
    for (index, pointer) in [
        0x4000_0000 | (start / 4) as u32,
        0x8000_0000 | raw_end,
        raw_end,
        0x8000_0000 | (raw_end + 6),
    ]
    .into_iter()
    .enumerate()
    {
        let at = table + index * 4;
        bytes[at..at + 4].copy_from_slice(&pointer.to_le_bytes());
    }
    bytes.resize(bytes.len().next_multiple_of(4096), 0);
    let len = bytes.len() as u32;
    bytes[516..520].copy_from_slice(&len.to_le_bytes());
    refresh_cram_crc(&mut bytes);
    let (_directory, evidence) = image(&bytes);
    let file = format!("cramfs:part:0:{}", hex::encode("/payload.bin"));
    let mut session = FsSession::new(&evidence);
    let full = session.read_file_limited(&file, usize::MAX).unwrap();
    assert_eq!(
        hex::encode(Sha256::digest(&full)),
        "d3e976619a6b7499471669dc3c587da9a4685b350a13bcd8ff871ad227fd68c6"
    );
    assert_eq!(
        session.read_file_range(&file, 4100, 37).unwrap(),
        full[4100..4137]
    );

    for (offset, value) in [(start, 0), (start, u16::MAX), (table + 4, 0)] {
        let mut corrupt = bytes.clone();
        corrupt[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        refresh_cram_crc(&mut corrupt);
        let (_directory, evidence) = image(&corrupt);
        assert!(FsSession::new(&evidence)
            .read_file_range(&file, 4100, 37)
            .is_err());
    }
}

#[test]
fn rom_images_reject_directory_cycles_and_malformed_ids() {
    // A valid global checksum must not conceal a corrupt individual header.
    let mut bytes = ROM.to_vec();
    bytes[76] ^= 1;
    refresh_rom_checksum(&mut bytes, 0, 512);
    let (_directory, evidence) = image(&bytes);
    assert!(FsSession::new(&evidence)
        .children("part:0")
        .unwrap_err()
        .to_string()
        .contains("header checksum"));

    // A checksummed hardlink to itself must terminate with an error.
    let mut bytes = ROM.to_vec();
    let alias = bytes
        .windows(b"alias.bin\0".len())
        .position(|window| window == b"alias.bin\0")
        .unwrap()
        - 16;
    bytes[alias + 4..alias + 8].copy_from_slice(&(alias as u32).to_be_bytes());
    refresh_rom_checksum(&mut bytes, alias, 32);
    refresh_rom_checksum(&mut bytes, 0, 512);
    let (_directory, evidence) = image(&bytes);
    assert!(FsSession::new(&evidence)
        .children("part:0")
        .unwrap_err()
        .to_string()
        .contains("hard link"));

    // A next-entry cycle with otherwise valid superblock and header checksums.
    let mut bytes = ROM.to_vec();
    bytes[64..68].copy_from_slice(&73u32.to_be_bytes());
    refresh_rom_checksum(&mut bytes, 64, 32);
    refresh_rom_checksum(&mut bytes, 0, 512);
    let (_directory, evidence) = image(&bytes);
    assert!(FsSession::new(&evidence)
        .children("part:0")
        .unwrap_err()
        .to_string()
        .contains("cycle"));
    let mut bytes = CRAM_LE.to_vec();
    // nested points to its own directory entry; detect ancestor reuse.
    let word = (76u32 / 4) << 6 | 2;
    bytes[84..88].copy_from_slice(&word.to_le_bytes());
    refresh_cram_crc(&mut bytes);
    let (_directory, evidence) = image(&bytes);
    let mut session = FsSession::new(&evidence);
    assert!(session
        .children(&format!("cramfs:part:0:{}", hex::encode("/nested")))
        .unwrap_err()
        .to_string()
        .contains("cycle"));
    for path in [
        "/../payload.bin",
        "/nested//note.txt",
        "/nested/./note.txt",
        "relative",
        "/nested\\note.txt",
    ] {
        assert!(parse_cramfs_id(&format!("cramfs:part:0:{}", hex::encode(path))).is_none());
    }
    assert!(parse_romfs_id("romfs:part:256:").is_none());
    assert!(parse_romfs_id("romfs:part:0:00").is_none());
    assert!(parse_romfs_id("romfs:part:0:ff").is_none());
}

#[test]
fn rom_images_embedded_trx_and_mbr_keep_native_geometry() {
    for fixture in [CRAM_LE, ROM] {
        let mut trx = vec![0u8; 28];
        trx[..4].copy_from_slice(b"HDR0");
        trx[12..16].copy_from_slice(&0x10000u32.to_le_bytes());
        trx[16..20].copy_from_slice(&28u32.to_le_bytes());
        trx.extend_from_slice(fixture);
        let length = trx.len() as u32;
        trx[4..8].copy_from_slice(&length.to_le_bytes());
        let crc = !crc32fast::hash(&trx[12..]);
        trx[8..12].copy_from_slice(&crc.to_le_bytes());
        let (_directory, evidence) = image(&trx);
        assert_eq!(evidence.sector_size, 1);
        assert_eq!(evidence.partitions[0].start_lba, 28);
        assert_eq!(
            FsSession::new(&evidence).children("part:0").unwrap().len(),
            if fixture == ROM { 6 } else { 5 }
        );
        assert_eq!(evidence.read_range(0, trx.len()).unwrap(), trx);

        let sectors = fixture.len().div_ceil(512);
        let mut disk = vec![0; (sectors + 2) * 512];
        disk[510..512].copy_from_slice(&[0x55, 0xaa]);
        disk[450] = 0x83;
        disk[454..458].copy_from_slice(&1u32.to_le_bytes());
        disk[458..462].copy_from_slice(&(sectors as u32).to_le_bytes());
        disk[512..512 + fixture.len()].copy_from_slice(fixture);
        let (_directory, evidence) = image(&disk);
        assert_eq!(evidence.sector_size, 512);
        assert_eq!(evidence.partitions[0].start_lba, 1);
        let mut session = FsSession::new(&evidence);
        let root = session.children("part:0").unwrap();
        let file = root.iter().find(|node| node.name == "payload.bin").unwrap();
        assert_eq!(
            session.read_file_limited(&file.id, usize::MAX).unwrap(),
            PAYLOAD
        );
    }
}

#[test]
#[ignore = "set LOVELYCARVE_ROM_IMAGES to semicolon-separated external CramFS/ROMFS images"]
fn rom_images_external_samples() {
    let paths = std::env::var("LOVELYCARVE_ROM_IMAGES").unwrap();
    for path in paths.split(';') {
        let evidence = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
        let mut session = FsSession::new(&evidence);
        let entries = session.children("part:0").unwrap();
        let file = entries
            .iter()
            .find(|entry| entry.name == "lsb-release")
            .unwrap();
        let bytes = session.read_file_limited(&file.id, 4096).unwrap();
        assert!(bytes.starts_with(b"DISTRIB_ID=Ubuntu\n"));
        assert_eq!(
            session.read_file_range(&file.id, 7, 23).unwrap(),
            bytes[7..30]
        );
    }
}
