use super::*;
use crate::core::evidence::image::PartitionKind;
use crate::core::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{self, FsSession},
};
use std::io::Read;

fn exercise(bytes: &[u8], prefix: &str, kind: PartitionKind, path: &str, expected: &[u8]) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.bin");
    std::fs::write(&source, bytes).unwrap();
    let evidence = Arc::new(EvidenceSession::from(DiskImage::open(&source).unwrap()));
    assert_eq!(evidence.partitions[0].kind, kind);
    assert_eq!(evidence.sector_size, 1);
    assert_eq!(evidence.read_range(0, bytes.len()).unwrap(), bytes);
    let mut session = FsSession::new(&evidence);
    let root = session.children("part:0").unwrap();
    assert!(!root.is_empty());
    assert!(root.iter().any(|node| node.timestamps.modified.is_some()));
    let id = format!("{prefix}:part:0:{}", hex::encode(path));
    assert_eq!(
        session.read_file_limited(&id, usize::MAX).unwrap(),
        expected
    );
    for offset in [
        0,
        4090,
        expected.len() / 2,
        expected.len().saturating_sub(7),
        expected.len(),
    ] {
        if offset > expected.len() {
            continue;
        }
        assert_eq!(
            session.read_file_range(&id, offset as u64, 121).unwrap(),
            expected[offset..(offset + 121).min(expected.len())]
        );
    }
    let mut actual = Vec::new();
    let length = fs_tree::stream_file_chunks(&evidence, &id, 8191, |chunk| {
        actual.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    assert_eq!(length, expected.len() as u64);
    assert_eq!(actual, expected);
    assert!(session.file_stat(&id).unwrap().data_runs.is_empty());
    let exported = directory.path().join("exported.bin");
    fs_tree::export_file_with_fault_report(&evidence, &id, &exported).unwrap();
    assert_eq!(std::fs::read(exported).unwrap(), expected);
}

#[test]
fn flashfs_erofs_native_tree_range_stream_export_and_codecs() {
    let base = include_bytes!("../../../../vendor/lamfold-erofs/tests/fixtures/erofs.img");
    let expected: Vec<_> = (0..300000).map(|index| (index % 256) as u8).collect();
    exercise(base, "erofs", PartitionKind::Erofs, "/big.dat", &expected);
    let expected =
        include_bytes!("../../../../vendor/lamfold-erofs/tests/fixtures/comp_multi.payload");
    for source in [
        include_bytes!("../../../../vendor/lamfold-erofs/tests/fixtures/comp_lz4.erofs").as_slice(),
        include_bytes!("../../../../vendor/lamfold-erofs/tests/fixtures/comp_lzma.erofs")
            .as_slice(),
        include_bytes!("../../../../vendor/lamfold-erofs/tests/fixtures/comp_deflate.erofs")
            .as_slice(),
        include_bytes!("../../../../vendor/lamfold-erofs/tests/fixtures/comp_zstd.erofs")
            .as_slice(),
    ] {
        exercise(
            source,
            "erofs",
            PartitionKind::Erofs,
            "/payload.bin",
            expected,
        );
    }
}

#[test]
fn flashfs_f2fs_native_inline_indirect_and_reference_mkfs() {
    let mut source = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../../../../vendor/f2fs-reader/tests/fixtures/synthetic.f2fs.zlib")
            .as_slice(),
    )
    .read_to_end(&mut source)
    .unwrap();
    let expected: Vec<_> = (0..1000 * 4096 + 37)
        .map(|index| {
            if (3..5).contains(&(index / 4096)) {
                0
            } else {
                (index % 256) as u8
            }
        })
        .collect();
    exercise(&source, "f2fs", PartitionKind::F2fs, "/big.bin", &expected);
    exercise(
        &source,
        "f2fs",
        PartitionKind::F2fs,
        "/small.txt",
        b"hello F2FS!\r\n",
    );
    let mut real = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../../../../vendor/f2fs-reader/tests/fixtures/syzkaller_0.f2fs.zlib")
            .as_slice(),
    )
    .read_to_end(&mut real)
    .unwrap();
    exercise(&real, "f2fs", PartitionKind::F2fs, "/file1", b"syzkallers");
}

#[test]
fn flashfs_erofs_checksum_volume_bounds_and_invalid_geometry() {
    let image = include_bytes!("../../../../vendor/lamfold-erofs/tests/fixtures/erofs.img");
    for offset in [1032, 1060, 1036] {
        let mut bytes = image.to_vec();
        bytes[offset] ^= 0x40;
        let source = lamfold::SliceSource::new(&bytes);
        let mut cache = BlockCache::new(0);
        let mut cx = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        assert!(lamfold_erofs::Erofs::open(source, &mut cx).is_err());
    }
}

#[test]
fn qnx_native_qnx4_qnx6_mmi_tree_range_stream_export() {
    let mut qnx4 = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../../../../vendor/qnx-reader/tests/fixtures/qnx4.zlib").as_slice(),
    )
    .read_to_end(&mut qnx4)
    .unwrap();
    let expected: Vec<_> = (0..1300)
        .map(|index| ((index * 13 + 7) % 256) as u8)
        .collect();
    exercise(&qnx4, "qnx4", PartitionKind::Qnx4, "/big.bin", &expected);
    for compressed in [
        include_bytes!("../../../../vendor/qnx-reader/tests/fixtures/nfi-qnx6.zlib").as_slice(),
        include_bytes!("../../../../vendor/qnx-reader/tests/fixtures/qnx6-mmi-le.zlib").as_slice(),
        include_bytes!("../../../../vendor/qnx-reader/tests/fixtures/qnx6-4096-be.zlib").as_slice(),
    ] {
        let mut bytes = Vec::new();
        flate2::read::ZlibDecoder::new(compressed)
            .read_to_end(&mut bytes)
            .unwrap();
        let (path, expected): (&str, &[u8]) = if bytes.starts_with(&[0xeb, 0x10, 0x90, 0]) {
            (
                "/directory_1/directory_1_1/message.txt",
                b"This is a test file system!\n",
            )
        } else {
            ("/small.txt", b"hello QNX!\r\n")
        };
        exercise(&bytes, "qnx6", PartitionKind::Qnx6, path, expected);
    }
}

#[test]
fn qnx_partition_bytes_override_linux_mbr_type_and_keep_volume_identity() {
    let mut qnx4 = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../../../../vendor/qnx-reader/tests/fixtures/qnx4.zlib").as_slice(),
    )
    .read_to_end(&mut qnx4)
    .unwrap();
    let mut qnx6 = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../../../../vendor/qnx-reader/tests/fixtures/nfi-qnx6.zlib").as_slice(),
    )
    .read_to_end(&mut qnx6)
    .unwrap();
    let mut bytes = vec![0; 2 * 1024 * 1024 + qnx6.len()];
    bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
    for (slot, start, payload) in [(0, 2048u32, qnx4.as_slice()), (1, 4096u32, qnx6.as_slice())] {
        let entry = 446 + slot * 16;
        bytes[entry + 4] = 0x83;
        bytes[entry + 8..entry + 12].copy_from_slice(&start.to_le_bytes());
        bytes[entry + 12..entry + 16].copy_from_slice(&(payload.len() as u32 / 512).to_le_bytes());
        bytes[start as usize * 512..start as usize * 512 + payload.len()].copy_from_slice(payload);
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("disk.img");
    std::fs::write(&path, &bytes).unwrap();
    let evidence = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
    assert_eq!(evidence.sector_size, 512);
    let mut session = FsSession::new(&evidence);
    for (kind, prefix, path, expected) in [
        (
            PartitionKind::Qnx4,
            "qnx4",
            "/small.txt",
            b"hello QNX!\r\n".as_slice(),
        ),
        (
            PartitionKind::Qnx6,
            "qnx6",
            "/directory_1/directory_1_1/message.txt",
            b"This is a test file system!\n".as_slice(),
        ),
    ] {
        let partition = evidence
            .partitions
            .iter()
            .find(|partition| partition.kind == kind)
            .unwrap();
        assert!(!session
            .children(&format!("part:{}", partition.id))
            .unwrap()
            .is_empty());
        let id = format!("{prefix}:part:{}:{}", partition.id, hex::encode(path));
        assert_eq!(
            session.read_file_limited(&id, usize::MAX).unwrap(),
            expected
        );
    }
    assert_eq!(evidence.read_range(0, bytes.len()).unwrap(), bytes);
}
