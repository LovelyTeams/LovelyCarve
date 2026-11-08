use super::*;

fn trx(payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; TRX_HEADER];
    bytes[..4].copy_from_slice(b"HDR0");
    bytes[12..16].copy_from_slice(&0x10000u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&(TRX_HEADER as u32).to_le_bytes());
    bytes.extend_from_slice(payload);
    let length = bytes.len() as u32;
    bytes[4..8].copy_from_slice(&length.to_le_bytes());
    let crc = !crc32fast::hash(&bytes[12..]);
    bytes[8..12].copy_from_slice(&crc.to_le_bytes());
    bytes
}

#[test]
fn firmware_trx_validates_crc_offsets_and_truncation() {
    let payload = b"hsqs synthetic filesystem";
    let bytes = trx(payload);
    let cancel = CancellationToken::new();
    let image = FirmwareImage::open(&bytes, &cancel).unwrap();
    assert_eq!(
        image.partitions,
        vec![Partition {
            id: 0,
            kind: PartitionKind::Squashfs,
            start_lba: 28,
            length_lba: payload.len() as u64,
            label: Some("TRX rootfs (segment 0)".into()),
            unique_guid: None,
            amiga_geometry: None,
        }]
    );
    for end in 0..bytes.len() {
        assert!(FirmwareImage::open(&bytes[..end], &cancel).is_err());
    }
    let mut corrupt = bytes.clone();
    corrupt[28] ^= 1;
    assert!(FirmwareImage::open(&corrupt, &cancel)
        .unwrap_err()
        .to_string()
        .contains("checksum"));
    corrupt = bytes.clone();
    corrupt[20..24].copy_from_slice(&28u32.to_le_bytes());
    let crc = !crc32fast::hash(&corrupt[12..]);
    corrupt[8..12].copy_from_slice(&crc.to_le_bytes());
    assert!(FirmwareImage::open(&corrupt, &cancel).is_err());
    cancel.cancel();
    assert!(FirmwareImage::open(&bytes, &cancel).is_err());
}

#[test]
fn firmware_trx_browses_and_preserves_source_bytes() {
    use crate::evidence::image::{DiskImage, ImageFormat};
    use crate::filesystem::volumes::squashfs::SquashfsVolume;
    use std::io::Cursor;
    let content = b"synthetic firmware file content";
    let mut writer = backhand::FilesystemWriter::default();
    writer
        .push_file(
            Cursor::new(content),
            "hello",
            backhand::NodeHeader::default(),
        )
        .unwrap();
    let mut filesystem = Cursor::new(Vec::new());
    writer.write(&mut filesystem).unwrap();
    let bytes = trx(filesystem.get_ref());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.trx");
    std::fs::write(&path, &bytes).unwrap();
    let image = Arc::new(DiskImage::open(&path).unwrap());
    assert_eq!(image.info().source_format, ImageFormat::Firmware);
    assert_eq!(image.sector_size, 1);
    assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
    let volume = SquashfsVolume::open(Arc::clone(&image), &image.partitions[0]).unwrap();
    assert_eq!(volume.root_children().unwrap()[0].name, "hello");
    assert_eq!(volume.read_file("/hello", usize::MAX).unwrap(), content);
    assert_eq!(
        volume.read_file_range("/hello", 3, 8).unwrap(),
        content[3..11]
    );
    let mut streamed = Vec::new();
    assert_eq!(
        volume
            .stream_file("/hello", 8192, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap(),
        content.len() as u64
    );
    assert_eq!(streamed, content);
}

#[test]
fn firmware_independent_enc_and_bgn_fixtures_browse_through_cached_dispatch() {
    use crate::evidence::image::{DiskImage, ImageFormat};
    use crate::filesystem::evidence_session::EvidenceSession;
    use crate::filesystem::tree::fs_tree::FsSession;
    let fixtures: [&[u8]; 2] = [
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test_data/firmware/synthetic.enc"
        )),
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test_data/firmware/synthetic-bgn.enc"
        )),
    ];
    for fixture in fixtures {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("router.enc");
        std::fs::write(&path, fixture).unwrap();
        let image = Arc::new(DiskImage::open(&path).unwrap());
        assert_eq!(image.info().source_format, ImageFormat::Firmware);
        assert_eq!(image.read_range(0, fixture.len()).unwrap(), fixture);
        let session = Arc::new(EvidenceSession::from(image));
        let mut filesystem = FsSession::new(&session);
        let root = filesystem.children("part:0").unwrap();
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].name, "hello");
        let expected = b"Independent DD-WRT fixture.\n".repeat(64);
        assert_eq!(
            filesystem
                .read_file_limited(&root[0].id, usize::MAX)
                .unwrap(),
            expected
        );
        assert_eq!(
            filesystem.read_file_range(&root[0].id, 11, 93).unwrap(),
            expected[11..104]
        );
        let mut streamed = Vec::new();
        filesystem
            .stream_file(&root[0].id, 4096, |bytes| {
                streamed.extend_from_slice(bytes);
                Ok(())
            })
            .unwrap();
        assert_eq!(streamed, expected);
        let mut corrupt = fixture.to_vec();
        let index = corrupt.len() - 5;
        corrupt[index] ^= 1;
        assert!(FirmwareImage::open(&corrupt, &CancellationToken::new()).is_err());
        for end in 0..fixture.len() - 4 {
            assert!(
                FirmwareImage::open(&fixture[..end], &CancellationToken::new()).is_err(),
                "truncation at {end}"
            );
        }
    }
}

#[test]
#[ignore = "requires LOVELYCARVE_FIRMWARE_IMAGE pointing to an external Buffalo ENC/TRX image"]
fn firmware_external_image_lists_reads_streams_and_ranges() {
    use crate::filesystem::evidence_session::EvidenceSession;
    use crate::filesystem::tree::fs_tree::{FsSession, NodeKind};
    use crate::filesystem::volumes::squashfs::{parse_squashfs_id, SquashfsVolume};
    let path = std::env::var_os("LOVELYCARVE_FIRMWARE_IMAGE").expect("LOVELYCARVE_FIRMWARE_IMAGE");
    let original = std::fs::read(&path).unwrap();
    let session = Arc::new(EvidenceSession::open(path).unwrap());
    assert_eq!(session.read_range(0, original.len()).unwrap(), original);
    let partition = &session.partitions[0];
    let volume = SquashfsVolume::open(Arc::clone(session.disk_image()), partition).unwrap();
    let mut filesystem = FsSession::new(&session);
    let mut queue = filesystem
        .children(&format!("part:{}", partition.id))
        .unwrap();
    assert!(!queue.is_empty());
    let mut file_count = 0;
    let mut total_bytes = 0;
    while let Some(node) = queue.pop() {
        match node.kind {
            NodeKind::Dir => queue.extend(filesystem.children(&node.id).unwrap()),
            NodeKind::File => {
                let (_, path) = parse_squashfs_id(&node.id).unwrap();
                let bytes = filesystem
                    .read_file_limited(&node.id, 128 * 1024 * 1024)
                    .unwrap();
                assert_eq!(bytes.len() as u64, node.size, "{path}");
                let mut streamed = Vec::new();
                volume
                    .stream_file(&path, 65536, |chunk| {
                        streamed.extend_from_slice(chunk);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(streamed, bytes, "{path}");
                for offset in [0, 1, 4095, 131071, node.size / 2, node.size, node.size + 1] {
                    let start = (offset as usize).min(bytes.len());
                    assert_eq!(
                        volume.read_file_range(&path, offset, 8193).unwrap(),
                        bytes[start..start.saturating_add(8193).min(bytes.len())],
                        "{path} @ {offset}"
                    );
                }
                file_count += 1;
                total_bytes += node.size;
            }
            _ => {}
        }
    }
    assert!(file_count > 0);
    eprintln!("firmware external verification: {file_count} files, {total_bytes} logical bytes");
}
