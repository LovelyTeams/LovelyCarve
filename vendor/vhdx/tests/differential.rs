#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::io::{Read, Seek, SeekFrom};
use vhdx::VhdxReader;

fn data(name: &str) -> Vec<u8> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|_| panic!("test data missing: {}", path.display()))
}

#[test]
fn fat_differential_with_parent_opens() {
    let parent = VhdxReader::from_bytes(data("fat-parent.vhdx")).expect("parent opens");
    VhdxReader::from_bytes_with_parent(data("fat-differential.vhdx"), parent)
        .expect("differential opens with parent");
}

#[test]
fn fat_differential_sector_0_readable() {
    let parent = VhdxReader::from_bytes(data("fat-parent.vhdx")).expect("parent opens");
    let mut diff = VhdxReader::from_bytes_with_parent(data("fat-differential.vhdx"), parent)
        .expect("differential opens");
    let mut buf = [0u8; 512];
    diff.read_exact(&mut buf)
        .expect("sector 0 must be readable");
}

#[test]
fn partially_present_block_routes_each_sector_by_bitmap() {
    const SECTOR_SIZE: usize = 512;
    const START: u64 = 133 * SECTOR_SIZE as u64;

    let parent_bytes = data("fat-parent.vhdx");
    let mut parent = VhdxReader::from_bytes(parent_bytes.clone()).expect("parent opens");
    parent.seek(SeekFrom::Start(START)).expect("parent seek");
    let mut parent_sectors = [0u8; 3 * SECTOR_SIZE];
    parent.read_exact(&mut parent_sectors).expect("parent read");

    let parent_for_child = VhdxReader::from_bytes(parent_bytes).expect("parent re-opens");
    let mut child =
        VhdxReader::from_bytes_with_parent(data("fat-differential.vhdx"), parent_for_child)
            .expect("differential opens");
    child.seek(SeekFrom::Start(START)).expect("child seek");
    let mut resolved = [0u8; 3 * SECTOR_SIZE];
    child.read_exact(&mut resolved).expect("child read");

    assert_eq!(&resolved[..SECTOR_SIZE], &parent_sectors[..SECTOR_SIZE]);
    assert_ne!(
        &resolved[SECTOR_SIZE..2 * SECTOR_SIZE],
        &parent_sectors[SECTOR_SIZE..2 * SECTOR_SIZE]
    );
    assert_eq!(resolved[SECTOR_SIZE + 6], 0xff);
    assert_eq!(
        &resolved[2 * SECTOR_SIZE..],
        &parent_sectors[2 * SECTOR_SIZE..]
    );
}

/// The reader serves a whole run of same-owner sectors per sector-bitmap byte.
/// A bulk read spanning many runs must be byte-identical to resolving one
/// sector at a time, which is the unbatched reference behaviour.
#[test]
fn bulk_read_of_partial_block_matches_sector_at_a_time() {
    const SECTOR_SIZE: usize = 512;
    const SECTORS: usize = 96;
    const START: u64 = 100 * SECTOR_SIZE as u64;

    fn child() -> VhdxReader {
        let parent = VhdxReader::from_bytes(data("fat-parent.vhdx")).expect("parent opens");
        VhdxReader::from_bytes_with_parent(data("fat-differential.vhdx"), parent)
            .expect("differential opens")
    }

    let mut bulk = vec![0u8; SECTORS * SECTOR_SIZE];
    let mut reader = child();
    reader.seek(SeekFrom::Start(START)).expect("bulk seek");
    reader.read_exact(&mut bulk).expect("bulk read");

    let mut reference = vec![0u8; SECTORS * SECTOR_SIZE];
    let mut reader = child();
    for (i, sector) in reference.chunks_mut(SECTOR_SIZE).enumerate() {
        reader
            .seek(SeekFrom::Start(START + (i * SECTOR_SIZE) as u64))
            .expect("sector seek");
        reader.read_exact(sector).expect("sector read");
    }

    assert_eq!(bulk, reference);
}

#[test]
fn fat_differential_virtual_disk_size_matches_parent() {
    let parent_bytes = data("fat-parent.vhdx");
    let parent_size = VhdxReader::from_bytes(parent_bytes.clone())
        .expect("parent opens")
        .virtual_disk_size();
    let parent2 = VhdxReader::from_bytes(parent_bytes).expect("parent re-opens");
    let diff = VhdxReader::from_bytes_with_parent(data("fat-differential.vhdx"), parent2)
        .expect("differential opens");
    assert_eq!(
        diff.virtual_disk_size(),
        parent_size,
        "child and parent virtual disk sizes must match"
    );
}

#[test]
fn from_bytes_still_refuses_differencing_disk() {
    let result = VhdxReader::from_bytes(data("fat-differential.vhdx"));
    assert!(
        result.is_err(),
        "from_bytes must still refuse a differencing disk without parent"
    );
}
