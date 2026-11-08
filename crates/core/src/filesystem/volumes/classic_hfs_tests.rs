use super::*;
use crate::evidence::image::PartitionKind;
use crate::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{self, FsSession},
};
use std::io::Read;

fn fixture() -> Vec<u8> {
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../../../../../vendor/classic-hfs/tests/fixtures/fragmented-1024.zlib")
            .as_slice(),
    )
    .read_to_end(&mut bytes)
    .unwrap();
    bytes
}
fn check(image: &Arc<EvidenceSession>, partition: u8) {
    let mut session = FsSession::new(image);
    let children = session.children(&format!("part:{partition}")).unwrap();
    assert_eq!(children.len(), 8);
    let file = children
        .iter()
        .find(|node| node.name == "Café.bin")
        .unwrap();
    let expected: Vec<u8> = (0..3 * 1024 + 37)
        .map(|index| (index * 17 + 3) as u8)
        .collect();
    assert_eq!(
        session.read_file_limited(&file.id, usize::MAX).unwrap(),
        expected
    );
    assert_eq!(
        session.read_file_range(&file.id, 3065, 121).unwrap(),
        expected[3065..]
    );
    assert_eq!(
        session.file_stat(&file.id).unwrap().size,
        expected.len() as u64
    );
    let mut streamed = Vec::new();
    fs_tree::stream_file_chunks(image, &file.id, 77, |chunk| {
        streamed.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    assert_eq!(streamed, expected);
    let time = file.timestamps.modified.as_deref().unwrap();
    assert_eq!(
        chrono::NaiveDateTime::parse_from_str(time, "%Y-%m-%dT%H:%M:%S")
            .unwrap()
            .and_utc()
            .timestamp(),
        3_800_000_048 - 2_082_844_800
    );
    assert!(chrono::DateTime::parse_from_rfc3339(time).is_err());
    let sub = children.iter().find(|node| node.name == "sub").unwrap();
    let nested = session.children(&sub.id).unwrap();
    assert_eq!(
        session.read_file_limited(&nested[0].id, 100).unwrap(),
        b"nested bytes!"
    );
    for name in ["a:b", "a＼b", "．", "．．"] {
        let node = children.iter().find(|node| node.name == name).unwrap();
        assert!(session.read_file_limited(&node.id, 100).is_ok());
    }
    let streams = fs_tree::list_data_streams(image, &file.id).unwrap();
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[1].name, "com.apple.ResourceFork");
    let resource: Vec<u8> = (0..3 * 1024 + 19)
        .map(|index| (index * 23 + 5) as u8)
        .collect();
    assert_eq!(streams[1].size, resource.len() as u64);
    assert_eq!(
        fs_tree::read_named_stream(image, &file.id, &streams[1].name, 100).unwrap(),
        resource[..100]
    );
    assert!(fs_tree::read_named_stream(image, &file.id, "bogus", 100).is_err());
    let temp = tempfile::tempdir().unwrap();
    let export = temp.path().join("fork.bin");
    fs_tree::export_named_stream(image, &file.id, &streams[1].name, &export).unwrap();
    assert_eq!(std::fs::read(&export).unwrap(), resource);
    assert!(fs_tree::export_named_stream(image, &file.id, &streams[1].name, &export).is_err());
    let data_export = temp.path().join("data.bin");
    fs_tree::export_file_with_fault_report(image, &file.id, &data_export).unwrap();
    assert_eq!(std::fs::read(data_export).unwrap(), expected);
    let only = children
        .iter()
        .find(|node| node.name == "only-rsrc")
        .unwrap();
    assert!(session.read_file_limited(&only.id, 100).unwrap().is_empty());
    assert_eq!(
        fs_tree::read_named_stream(image, &only.id, &streams[1].name, 100).unwrap(),
        b"RSRC!"
    );
}
#[test]
fn classic_hfs_bare_volume_native_read_export_and_resource_fork() {
    let bytes = fixture();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("hfs.img");
    std::fs::write(&path, &bytes).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
    assert_eq!(image.partitions[0].kind, PartitionKind::Hfs);
    assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
    check(&image, 0);
}
#[test]
fn classic_hfs_apm_partition_refinement_preserves_source_identity() {
    let volume = fixture();
    let start = 64 * 512;
    let mut bytes = vec![0; start + volume.len()];
    bytes[0..2].copy_from_slice(b"ER");
    bytes[2..4].copy_from_slice(&512u16.to_be_bytes());
    let count = (bytes.len() / 512) as u32;
    bytes[4..8].copy_from_slice(&count.to_be_bytes());
    for (slot, block_start, blocks, name, kind) in [
        (1, 1u32, 2u32, "Map", "Apple_partition_map"),
        (2, 64, (volume.len() / 512) as u32, "Classic", "Apple_HFS"),
    ] {
        let record = &mut bytes[slot * 512..(slot + 1) * 512];
        record[..2].copy_from_slice(b"PM");
        record[4..8].copy_from_slice(&2u32.to_be_bytes());
        record[8..12].copy_from_slice(&block_start.to_be_bytes());
        record[12..16].copy_from_slice(&blocks.to_be_bytes());
        record[16..16 + name.len()].copy_from_slice(name.as_bytes());
        record[48..48 + kind.len()].copy_from_slice(kind.as_bytes());
        record[84..88].copy_from_slice(&blocks.to_be_bytes());
        record[88..92].copy_from_slice(&0x33u32.to_be_bytes());
    }
    bytes[start..].copy_from_slice(&volume);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("apm.img");
    std::fs::write(&path, &bytes).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
    let partition = image
        .partitions
        .iter()
        .find(|partition| partition.kind == PartitionKind::Hfs)
        .unwrap();
    assert_eq!(partition.start_lba, 64);
    assert_eq!(
        image.read_range(start as u64, volume.len()).unwrap(),
        volume
    );
    check(&image, partition.id);
}
