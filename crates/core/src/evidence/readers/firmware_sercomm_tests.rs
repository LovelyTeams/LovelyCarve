use super::*;
use crate::{
    evidence::image::ImageFormat,
    filesystem::{
        evidence_session::{EvidenceSession, ImageExportMode},
        tree::fs_tree::{stream_file_chunks, FsSession, NodeKind},
    },
};
use sha2::{Digest, Sha256};
use std::{io::Read, sync::Arc};

fn stored_fixture() -> Vec<u8> {
    let bytes = include_bytes!("../../../test_data/firmware/synthetic-sercomm-stored.img.zlib");
    let mut output = Vec::new();
    flate2::read::ZlibDecoder::new(&bytes[..])
        .read_to_end(&mut output)
        .unwrap();
    output
}

#[test]
fn firmware_sercomm_native_tree_preserves_source_identity_geometry_hash_and_export() {
    let deflate = include_bytes!("../../../test_data/firmware/synthetic-sercomm-deflate.img");
    let stored = stored_fixture();
    let metadata: serde_json::Value = serde_json::from_str(include_str!(
        "../../../test_data/firmware/sercomm-fixtures.json"
    ))
    .unwrap();
    for original in [&deflate[..], stored.as_slice()] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("firmware.img");
        std::fs::write(&path, original).unwrap();
        let image = Arc::new(EvidenceSession::open(&path).unwrap());
        let info = image.info();
        assert_eq!(info.source_format, ImageFormat::Firmware);
        assert_eq!(info.size_bytes, original.len() as u64);
        assert!(info.unlocked_partitions.is_empty());
        assert_eq!(info.sector_size, 1);
        assert_eq!(info.partitions[0].kind, PartitionKind::Ubi);
        assert!(info.partitions[0].start_lba + info.partitions[0].length_lba <= info.size_bytes);
        let native = image.filesystem_session("part:0");
        let native_image = Arc::downgrade(native.disk_image());
        assert!(Arc::ptr_eq(image.filesystem_session("root"), &image));
        assert!(Arc::ptr_eq(
            image.filesystem_session("unalloc:0:64"),
            &image
        ));
        assert_eq!(native.id, image.id);
        assert_eq!(native.partitions[0].start_lba, 0);
        assert_eq!(
            native.partitions[0].length_lba,
            metadata["rootfsSize"].as_u64().unwrap()
        );
        assert_eq!(image.native_volume_size("part:0"), Some(native.len()));
        assert_eq!(
            crate::catalog::snapshot::volume_snapshot::volume_size_for_key(&image, "part:0"),
            native.len()
        );
        let decoded = native.read_range(0, native.len() as usize).unwrap();
        assert_eq!(
            hex::encode(Sha256::digest(&decoded)),
            metadata["rootfsSha256"].as_str().unwrap()
        );
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        assert_eq!(
            image.disk_image().info().partitions[0].length_lba,
            info.partitions[0].length_lba
        );
        for mode in [ImageExportMode::Source, ImageExportMode::UnlockedPartitions] {
            let export = image.raw_export_reader(mode).unwrap();
            let mut bytes = vec![0; original.len()];
            assert_eq!(export.read_into(0, &mut bytes).unwrap(), bytes.len());
            assert_eq!(bytes, original);
        }
        let expected = b"Independent DD-WRT fixture.\n".repeat(64);
        let mut session = FsSession::new(&image);
        let roots = session.children("part:0").unwrap();
        let root = roots
            .iter()
            .find(|entry| entry.kind == NodeKind::Dir)
            .unwrap();
        assert_eq!(root.id, "squashfs:ubi:part:0:2:");
        let files = session.children(&root.id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "hello");
        let id = &files[0].id;
        assert_eq!(session.read_file_limited(id, usize::MAX).unwrap(), expected);
        assert_eq!(
            session.read_file_range(id, 11, 93).unwrap(),
            expected[11..104]
        );
        let stat = session.file_stat(id).unwrap();
        assert_eq!(stat.size, expected.len() as u64);
        assert!(stat.data_runs.is_empty());
        let mut streamed = Vec::new();
        stream_file_chunks(&image, id, 53, |bytes| {
            streamed.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
        assert_eq!(streamed, expected);
        assert_eq!(std::fs::read(path).unwrap(), original);
        drop(session);
        drop(image);
        assert!(native_image.upgrade().is_none());
    }
}

#[test]
fn firmware_sercomm_kernel_crc_is_checked_after_valid_zip_and_seal() {
    let mut bytes = stored_fixture();
    let decoded = sercomm_firmware::open(&bytes, &CancellationToken::new()).unwrap();
    let extent = decoded.source_extent();
    let start = extent.start as usize;
    let end = extent.end as usize;
    bytes[start + 2 * 1024 * 1024 + 64] ^= 1;
    let eocd = bytes.len() - 22;
    let central =
        512 + u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
    let crc = crc32fast::hash(&bytes[start..end]);
    bytes[512 + 14..512 + 18].copy_from_slice(&crc.to_le_bytes());
    bytes[central + 16..central + 20].copy_from_slice(&crc.to_le_bytes());
    bytes[511] = 0;
    bytes[511] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    let error = FirmwareImage::open(&bytes, &CancellationToken::new()).unwrap_err();
    assert!(error.to_string().contains("uImage data CRC"), "{error:#}");
}

#[test]
#[ignore = "set LOVELYCARVE_SERCOMM_FIRMWARE to semicolon-separated public Sercomm factory images"]
fn firmware_sercomm_external_all_files_stream_range_and_original_source() {
    for path in std::env::var("LOVELYCARVE_SERCOMM_FIRMWARE")
        .unwrap()
        .split(';')
    {
        let original = std::fs::read(path).unwrap();
        let image = Arc::new(EvidenceSession::open(path).unwrap());
        assert_eq!(image.info().source_format, ImageFormat::Firmware);
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        let mut session = FsSession::new(&image);
        let mut pending = vec!["part:0".to_owned()];
        let mut count = 0;
        let mut special_count = 0;
        while let Some(parent) = pending.pop() {
            for entry in session.children(&parent).unwrap() {
                if entry.kind == NodeKind::Dir {
                    pending.push(entry.id);
                } else if entry.kind == NodeKind::File {
                    let bytes = match session.read_file_limited(&entry.id, 128 * 1024 * 1024) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            // The tree's generic file bucket also contains devices,
                            // FIFOs and sockets. Only the precise metadata rejection
                            // is allowed here; ordinary file failures remain fatal.
                            let (_, path) =
                                crate::filesystem::volumes::squashfs::parse_squashfs_id(&entry.id)
                                    .unwrap();
                            assert_eq!(entry.size, 0);
                            assert_eq!(
                                error.to_string(),
                                format!("SquashFS entry {path} is not a regular file")
                            );
                            assert!(session.read_file_range(&entry.id, 0, 1).is_err());
                            assert!(stream_file_chunks(&image, &entry.id, 65536, |_| {
                                panic!("special node produced content")
                            })
                            .is_err());
                            special_count += 1;
                            continue;
                        }
                    };
                    assert_eq!(bytes.len() as u64, entry.size);
                    let mut streamed = Vec::new();
                    stream_file_chunks(&image, &entry.id, 65536, |chunk| {
                        streamed.extend_from_slice(chunk);
                        Ok(())
                    })
                    .unwrap();
                    assert_eq!(streamed, bytes);
                    for offset in [
                        0,
                        4090,
                        65530,
                        bytes.len() / 2,
                        bytes.len(),
                        bytes.len() + 1,
                    ] {
                        let start = offset.min(bytes.len());
                        assert_eq!(
                            session
                                .read_file_range(&entry.id, offset as u64, 8193)
                                .unwrap(),
                            bytes[start..(start + 8193).min(bytes.len())]
                        );
                    }
                    count += 1;
                }
            }
        }
        assert!(count > 100, "{path}: too few files {count}");
        assert!(special_count > 0, "{path}: expected device metadata");
        eprintln!("Sercomm external verification: {path}: {count} regular files and raw volumes, {special_count} special nodes");
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
}
