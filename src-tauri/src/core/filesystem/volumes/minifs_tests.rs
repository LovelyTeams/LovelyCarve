use super::*;
use crate::core::evidence::image::{ImageFormat, PartitionKind};
use crate::core::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{self, FsSession},
};
use sha2::{Digest, Sha256};

const FIXTURE: &[u8] = include_bytes!("../../../../vendor/minifs/tests/fixtures/synthetic.minifs");

fn wrap(bytes: &[u8]) -> Vec<u8> {
    let mut header = vec![0; 148];
    header[0x18..0x1c].copy_from_slice(&((128 + bytes.len()) as u32).to_be_bytes());
    header[0x20..0x24].copy_from_slice(&1u32.to_be_bytes());
    header[0x54..0x58].copy_from_slice(&128u32.to_be_bytes());
    header[0x58..0x5c].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
    header.extend_from_slice(bytes);
    header
}

#[test]
fn minifs_dispatch_bare_and_firmware_content_and_source_identity() {
    for source in [FIXTURE.to_vec(), wrap(FIXTURE)] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("minifs.bin");
        std::fs::write(&path, &source).unwrap();
        let image = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
        assert_eq!(image.partitions.len(), 1);
        assert_eq!(image.partitions[0].kind, PartitionKind::Minifs);
        assert_eq!(image.sector_size, 1);
        assert_eq!(image.read_range(0, source.len()).unwrap(), source);
        assert_eq!(
            image.partitions[0].start_lba,
            if source.len() == FIXTURE.len() {
                0
            } else {
                148
            }
        );
        let mut session = FsSession::new(&image);
        let root = session.children("part:0").unwrap();
        assert_eq!(root.len(), 3);
        let hello = root.iter().find(|entry| entry.name == "hello.txt").unwrap();
        assert!(hello.timestamps.modified.is_none());
        assert_eq!(
            session.read_file_limited(&hello.id, 100).unwrap(),
            b"hello\n"
        );
        let node = format!("minifs:part:0:{}", hex::encode("/web/js/app.js"));
        let bytes = session.read_file_limited(&node, usize::MAX).unwrap();
        assert_eq!(bytes.len(), 256 * 289 + 12);
        assert_eq!(
            session.read_file_range(&node, 65530, 50).unwrap(),
            bytes[65530..65580]
        );
        let mut streamed = Vec::new();
        assert_eq!(
            fs_tree::stream_file_chunks(&image, &node, 4099, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap(),
            bytes.len() as u64
        );
        assert_eq!(streamed, bytes);
        assert!(
            fs_tree::stream_file_chunks(&image, &node, 4096, |_| anyhow::bail!("cancelled"))
                .is_err()
        );
        let stat = session.file_stat(&node).unwrap();
        assert_eq!(stat.size, bytes.len() as u64);
        assert!(stat.mode.is_none());
        assert!(stat.data_runs.is_empty());
        for path in ["/../bad", "//bad", "/a\\b", "/a/", "/a\0b"] {
            assert!(parse_minifs_id(&format!("minifs:part:0:{}", hex::encode(path))).is_none());
        }
        assert!(parse_minifs_id("minifs:part:256:").is_none());
    }
}

#[test]
fn minifs_firmware_rejects_invalid_envelope_and_metadata() {
    for (offset, value) in [(0x18, 1), (0x58, u32::MAX), (148 + 16, 2)] {
        let mut bytes = wrap(FIXTURE);
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("broken.bin");
        std::fs::write(&path, bytes).unwrap();
        assert!(DiskImage::open(path).is_err());
    }
}

#[test]
#[ignore = "requires LOVELYCARVE_MINIFS_FIRMWARE and MINIFS_ORACLE"]
fn minifs_external_firmware_all_files_python_oracle() {
    let path =
        std::env::var_os("LOVELYCARVE_MINIFS_FIRMWARE").expect("LOVELYCARVE_MINIFS_FIRMWARE");
    let source = std::fs::read(&path).unwrap();
    let manifest =
        std::fs::read_to_string(std::env::var_os("MINIFS_ORACLE").expect("MINIFS_ORACLE")).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
    assert_eq!(image.info().source_format, ImageFormat::Firmware);
    assert_eq!(image.read_range(0, source.len()).unwrap(), source);
    assert_eq!(image.partitions[0].kind, PartitionKind::Minifs);
    let mut session = FsSession::new(&image);
    let mut pending = vec!["part:0".to_owned()];
    let mut actual = std::collections::BTreeMap::new();
    while let Some(parent) = pending.pop() {
        for child in session.children(&parent).unwrap() {
            if child.kind == NodeKind::Dir {
                pending.push(child.id);
            } else {
                actual.insert(parse_minifs_id(&child.id).unwrap().1, child);
            }
        }
    }
    assert_eq!(actual.len(), manifest.lines().count());
    for line in manifest.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        let path = fields[0];
        let node = &actual[path];
        let bytes = session.read_file_limited(&node.id, usize::MAX).unwrap();
        assert_eq!(node.size, fields[1].parse::<u64>().unwrap());
        assert_eq!(bytes.len() as u64, node.size);
        assert_eq!(hex::encode(Sha256::digest(&bytes)), fields[2], "{path}");
        for offset in [
            0,
            bytes.len() / 2,
            bytes.len().saturating_sub(9),
            bytes.len(),
        ] {
            assert_eq!(
                session
                    .read_file_range(&node.id, offset as u64, 113)
                    .unwrap(),
                bytes[offset..(offset + 113).min(bytes.len())],
                "{path}"
            );
        }
        let mut stream_hash = Sha256::new();
        let count = fs_tree::stream_file_chunks(&image, &node.id, 8191, |chunk| {
            stream_hash.update(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(count, node.size);
        assert_eq!(hex::encode(stream_hash.finalize()), fields[2], "{path}");
        assert!(session.file_stat(&node.id).unwrap().data_runs.is_empty());
    }
    println!(
        "MINIFS firmware oracle: {} files; full, stream, ranges and original source verified",
        actual.len()
    );
}
