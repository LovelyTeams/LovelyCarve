use std::io::Cursor;
use std::sync::Arc;

use super::*;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{self, FsSession};

fn seal(bytes: &mut [u8]) {
    let end = bytes.len() - 4;
    let crc = ubidump::utils::crc::crc32_jamcrc(&bytes[..end]);
    bytes[end..].copy_from_slice(&crc.to_be_bytes());
}

fn ubi_fixture(payload: &[u8]) -> Vec<u8> {
    const PEB: usize = 32768;
    const DATA: usize = 4096;
    const LEB: usize = PEB - DATA;
    let count = payload.len().div_ceil(LEB).max(1);
    let mut table = vec![0; 128 * 172];
    for (index, record) in table.chunks_exact_mut(172).enumerate() {
        if index == 2 {
            record[..4].copy_from_slice(&(count as u32).to_be_bytes());
            record[4..8].copy_from_slice(&1u32.to_be_bytes());
            record[12] = 1;
            record[14..16].copy_from_slice(&4u16.to_be_bytes());
            record[16..20].copy_from_slice(b"root");
        }
        seal(record);
    }
    let mut bytes = vec![0xff; (count + 2) * PEB];
    for (physical, block) in bytes.chunks_exact_mut(PEB).enumerate() {
        let ec = &mut block[..64];
        ec.fill(0);
        ec[..4].copy_from_slice(b"UBI#");
        ec[4] = 1;
        ec[16..20].copy_from_slice(&2048u32.to_be_bytes());
        ec[20..24].copy_from_slice(&(DATA as u32).to_be_bytes());
        seal(ec);
        let vid = &mut block[2048..2112];
        vid.fill(0);
        vid[..4].copy_from_slice(b"UBI!");
        vid[4] = 1;
        vid[5] = 1;
        let volume = if physical < 2 { 0x7fffefffu32 } else { 2 };
        // Reverse the payload PEB order to exercise actual logical mapping.
        let number = if physical < 2 {
            physical
        } else {
            count + 1 - physical
        };
        vid[8..12].copy_from_slice(&volume.to_be_bytes());
        vid[12..16].copy_from_slice(&(number as u32).to_be_bytes());
        seal(vid);
        let data = if physical < 2 {
            &table[..]
        } else {
            &payload[number * LEB..payload.len().min((number + 1) * LEB)]
        };
        block[DATA..DATA + data.len()].copy_from_slice(data);
    }
    bytes
}

#[test]
fn ubi_nested_squashfs_reads_and_streams_across_reordered_lebs() {
    let expected: Vec<u8> = (0..100_000u32)
        .flat_map(|number| number.wrapping_mul(2654435761).to_le_bytes())
        .collect();
    let mut writer = backhand::FilesystemWriter::default();
    writer
        .push_dir("etc", backhand::NodeHeader::default())
        .unwrap();
    writer
        .push_file(
            Cursor::new(expected.clone()),
            "etc/data.bin",
            backhand::NodeHeader::default(),
        )
        .unwrap();
    let mut squashfs = Cursor::new(Vec::new());
    writer.write(&mut squashfs).unwrap();
    assert!(squashfs.get_ref().len() > 32768);
    let bytes = ubi_fixture(squashfs.get_ref());
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fixture.ubi");
    std::fs::write(&path, &bytes).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
    assert_eq!(image.partitions[0].kind, PartitionKind::Ubi);
    assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
    let mut session = FsSession::new(&image);
    let roots = session.children("part:0").unwrap();
    let root = roots
        .iter()
        .find(|node| node.kind == NodeKind::Dir)
        .unwrap();
    assert_eq!(root.id, "squashfs:ubi:part:0:2:");
    let entries = session.children(&root.id).unwrap();
    assert_eq!(entries[0].parent_id.as_deref(), Some(root.id.as_str()));
    let files = session.children(&entries[0].id).unwrap();
    let id = &files[0].id;
    assert_eq!(
        session.read_file_limited(id, expected.len()).unwrap(),
        expected
    );
    assert_eq!(
        session.read_file_range(id, 28_665, 50).unwrap(),
        expected[28_665..28_715]
    );
    assert_eq!(session.file_stat(id).unwrap().size, expected.len() as u64);
    let mut streamed = Vec::new();
    fs_tree::stream_file_chunks(&image, id, 8191, |chunk| {
        streamed.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    assert_eq!(streamed, expected);
    let raw = roots
        .iter()
        .find(|node| node.kind == NodeKind::File)
        .unwrap();
    assert_eq!(
        session.read_file_range(&raw.id, 28_665, 50).unwrap(),
        squashfs.get_ref()[28_665..28_715]
    );
    assert_ne!(
        crate::core::filesystem::fs_dispatch::volume_root_key(id),
        crate::core::filesystem::fs_dispatch::volume_root_key("squashfs:ubi:part:0:3:")
    );
}

#[test]
#[ignore = "requires LOVELYCARVE_UBI_IMAGE pointing to an external UBI/HDR1 image"]
fn ubi_real_firmware_lists_and_reads_files() {
    let path = std::env::var_os("LOVELYCARVE_UBI_IMAGE").expect("LOVELYCARVE_UBI_IMAGE");
    let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
    let partition = image
        .partitions
        .iter()
        .find(|part| part.kind == PartitionKind::Ubi)
        .unwrap();
    let mut session = FsSession::new(&image);
    let roots = session.children(&format!("part:{}", partition.id)).unwrap();
    assert!(roots.iter().any(|node| node.kind == NodeKind::File));
    let root = roots
        .iter()
        .find(|node| node.kind == NodeKind::Dir)
        .expect("SquashFS logical volume");
    let children = session.children(&root.id).unwrap();
    let etc = children
        .iter()
        .find(|node| node.name == "etc")
        .expect("etc directory");
    let files = session.children(&etc.id).unwrap();
    let release = files
        .iter()
        .find(|node| node.name == "openwrt_release")
        .expect("OpenWrt release file");
    let bytes = session.read_file_limited(&release.id, 65536).unwrap();
    assert!(!bytes.is_empty());
    assert!(String::from_utf8_lossy(&bytes).contains("DISTRIB_ID"));
    let mut streamed = Vec::new();
    fs_tree::stream_file_chunks(&image, &release.id, 37, |chunk| {
        streamed.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    assert_eq!(streamed, bytes);
    assert_eq!(
        session.read_file_range(&release.id, 3, 17).unwrap(),
        bytes[3..20]
    );
}
