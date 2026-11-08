use super::*;
use crate::{
    evidence::{image::PartitionKind, partitions::TableScheme},
    filesystem::{
        evidence_session::EvidenceSession,
        tree::fs_tree::{self, FsSession},
    },
};
use std::io::Read;
fn inflate(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .read_to_end(&mut output)
        .unwrap();
    output
}
fn embed_mbr(volume: &[u8], type_byte: u8) -> Vec<u8> {
    let mut bytes = vec![0; 1024 * 1024 + volume.len()];
    bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
    bytes[450] = type_byte;
    bytes[454..458].copy_from_slice(&2048u32.to_le_bytes());
    bytes[458..462].copy_from_slice(&((volume.len() / 512) as u32).to_le_bytes());
    bytes[1024 * 1024..].copy_from_slice(volume);
    bytes
}
#[test]
fn amiga_rdb_native_keeps_filesystem_geometry_and_exact_bytes() {
    let rdb = inflate(include_bytes!(
        "../../../../../vendor/amiga-reader/tests/fixtures/rdb-multiblock.zlib"
    ));
    let bytes = rdb;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("amiga.hdf");
    std::fs::write(&path, &bytes).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
    assert_eq!(image.table_scheme, Some(TableScheme::Rdb));
    assert_eq!(image.partitions.len(), 4);
    assert_eq!(image.partitions[2].kind, PartitionKind::Unknown);
    let mut session = FsSession::new(&image);
    for (index, variant, block_size, blocks, start) in [
        (0, 3, 1024, 2418, 126),
        (1, 7, 4096, 434, 5040),
        (3, 0, 512, 2394, 10206),
    ] {
        let partition = &image.partitions[index];
        assert_eq!(partition.kind, PartitionKind::Amiga);
        assert_eq!(partition.start_lba, start);
        let geometry = partition.amiga_geometry.unwrap();
        assert_eq!(
            (geometry.block_size, geometry.blocks, geometry.dos_type),
            (block_size, blocks, 0x444f5300 + variant)
        );
        if index != 3 {
            assert!(u64::from(block_size) * blocks < partition.length_lba * 512);
        }
        let root = session.children(&format!("part:{index}")).unwrap();
        let big = root.iter().find(|node| node.name == "Café.bin").unwrap();
        assert_eq!(parse_amiga_id(&big.id).unwrap().0, index as u8);
        let expected: Vec<_> = (0..200013).map(|index| (index * 17 + 5) as u8).collect();
        assert_eq!(
            session.read_file_limited(&big.id, usize::MAX).unwrap(),
            expected
        );
        for offset in [487, 511, 36861, 73720, 200010, 200013] {
            assert_eq!(
                session.read_file_range(&big.id, offset, 73).unwrap(),
                expected[(offset as usize).min(expected.len())
                    ..(offset as usize + 73).min(expected.len())]
            );
        }
        let mut streamed = Vec::new();
        fs_tree::stream_file_chunks(&image, &big.id, 4091, |chunk| {
            streamed.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(streamed, expected);
        let output = directory.path().join(format!("export_{index}.bin"));
        fs_tree::export_file_with_fault_report(&image, &big.id, &output).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), expected);
        assert!(fs_tree::export_file_with_fault_report(&image, &big.id, &output).is_err());
        let sub = root.iter().find(|node| node.name == "sub").unwrap();
        assert_eq!(session.children(&sub.id).unwrap().len(), 101);
        let named = root.iter().find(|node| node.name == "back\\slash").unwrap();
        assert_eq!(
            session.read_file_limited(&named.id, 100).unwrap(),
            b"Unix-safe evidence name"
        );
        let modified = big.timestamps.modified.as_deref().unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(modified).is_err());
        assert!(chrono::NaiveDateTime::parse_from_str(modified, "%Y-%m-%dT%H:%M:%S%.f").is_ok());
        assert!(big.timestamps.created.is_none());
    }
    assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
}
#[test]
fn amiga_native_bare_floppy_variants_are_browsable_and_preserve_local_dates() {
    for fixture in [
        include_bytes!("../../../../../vendor/amiga-reader/tests/fixtures/dos0-512-1760.zlib")
            .as_slice(),
        include_bytes!("../../../../../vendor/amiga-reader/tests/fixtures/dos7-512-1760.zlib")
            .as_slice(),
    ] {
        for embedded in [false, true] {
            let volume = inflate(fixture);
            let bytes = if embedded {
                embed_mbr(&volume, 0x30)
            } else {
                volume
            };
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("floppy.adf");
            std::fs::write(&path, &bytes).unwrap();
            let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
            assert_eq!(image.partitions[0].kind, PartitionKind::Amiga);
            let mut session = FsSession::new(&image);
            let root = session.children("part:0").unwrap();
            assert!(root.iter().any(|node| node.name == "Café.bin"));
            assert!(
                parse_amiga_id(&format!("amiga:part:0:{}", hex::encode("/../outside"))).is_none()
            );
            assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
        }
    }
}

#[test]
fn amiga_internal_recursive_table_scan_keeps_rdb_geometry() {
    use crate::evidence::partitions;
    let rdb = inflate(include_bytes!(
        "../../../../../vendor/amiga-reader/tests/fixtures/rdb-multiblock.zlib"
    ));
    let bytes = embed_mbr(&rdb, 0x76);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested.img");
    std::fs::write(&path, bytes).unwrap();
    let image = DiskImage::open(path).unwrap();
    // The recursive tree is a separate Rust-internal API, rather than the
    // main window's flat, top-level partition list.
    let tree = partitions::detect_partitions_tree(&image, 512, 4).unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].children.len(), 4);
    assert_eq!(
        tree[0].children[1].partition.amiga_geometry.unwrap().blocks,
        434
    );
    assert!(tree[0].children[1].children.is_empty());
}

#[test]
fn amiga_native_hardlinks_directories_and_latin1_softlinks_keep_separate_names() {
    let bytes = inflate(include_bytes!(
        "../../../../../vendor/amiga-reader/tests/fixtures/links-dos3.zlib"
    ));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("links.adf");
    std::fs::write(&path, bytes).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
    let mut session = FsSession::new(&image);
    let root = session.children("part:0").unwrap();
    let original = root.iter().find(|node| node.name == "Café.bin").unwrap();
    let alias = root.iter().find(|node| node.name == "alias.bin").unwrap();
    assert_eq!(original.inode, alias.inode);
    assert_ne!(original.id, alias.id);
    assert_ne!(original.timestamps.modified, alias.timestamps.modified);
    assert_eq!(
        session.read_file_limited(&original.id, usize::MAX).unwrap(),
        session.read_file_limited(&alias.id, usize::MAX).unwrap()
    );
    let sub = root.iter().find(|node| node.name == "sub").unwrap();
    let linked = root.iter().find(|node| node.name == "dirlink").unwrap();
    assert_eq!(sub.inode, linked.inode);
    assert_eq!(
        session.children(&sub.id).unwrap().len(),
        session.children(&linked.id).unwrap().len()
    );
    let soft = root.iter().find(|node| node.name == "softlink").unwrap();
    assert_eq!(soft.link_target.as_deref(), Some("Work:Café.bin"));
    assert_eq!(
        session.read_file_limited(&soft.id, 100).unwrap(),
        b"Work:Caf\xe9.bin"
    );
    let stat = session.file_stat(&alias.id).unwrap();
    assert_eq!(stat.inode, original.inode);
    assert!(stat.mode.is_none());
}

#[test]
fn amiga_dirty_bitmap_is_recognized_but_refused_without_source_changes() {
    let mut bytes = inflate(include_bytes!(
        "../../../../../vendor/amiga-reader/tests/fixtures/dos1-512-1760.zlib"
    ));
    let root = &mut bytes[880 * 512..881 * 512];
    root[312..316].fill(0);
    root[20..24].fill(0);
    let sum = root.chunks_exact(4).fold(0u32, |sum, word| {
        sum.wrapping_add(u32::from_be_bytes(word.try_into().unwrap()))
    });
    root[20..24].copy_from_slice(&sum.wrapping_neg().to_be_bytes());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dirty.adf");
    std::fs::write(&path, &bytes).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
    assert_eq!(image.partitions[0].kind, PartitionKind::Amiga);
    let error = FsSession::new(&image).children("part:0").unwrap_err();
    assert!(format!("{error:#}").contains("dirty"));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
