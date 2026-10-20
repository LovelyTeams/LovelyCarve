use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::core::evidence::image::{DerivedImage, DiskImage, ImageFormat};
use crate::core::evidence::partitions::{Partition, PartitionKind, SectorSource};
use crate::core::evidence::vss_image::{
    list, open, session_path_prefix, ShadowCopyOrigin, SESSION_PREFIX,
};
use crate::core::filesystem::tree::fs_tree::children_for_image;

const BLOCK: usize = 16 * 1024;
const VOLUME_BLOCKS: usize = 32;
/// The NTFS partition starts 1 MiB into the disk.
const PARTITION_START: usize = 1024 * 1024;
const STORE_ID: [u8; 16] = [
    0x44, 0x33, 0x22, 0x11, 0x66, 0x55, 0x88, 0x77, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00,
];
/// 2021-05-01T17:40:03.223Z as a FILETIME.
const CREATED: u64 = 132_643_644_032_230_304;

struct MemorySource(Vec<u8>);

impl SectorSource for MemorySource {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        let start = (offset as usize).min(self.0.len());
        let end = start.saturating_add(len).min(self.0.len());
        Ok(self.0[start..end].to_vec())
    }

    fn size(&self) -> u64 {
        self.0.len() as u64
    }
}

fn put(bytes: &mut [u8], offset: usize, value: &[u8]) {
    bytes[offset..offset + value.len()].copy_from_slice(value);
}

fn store_block(volume: &mut [u8], block: usize, record_type: u32) {
    let at = block * BLOCK;
    put(volume, at, &vsc::VSS_IDENTIFIER);
    put(volume, at + 16, &1u32.to_le_bytes());
    put(volume, at + 20, &record_type.to_le_bytes());
    put(volume, at + 32, &((at as u64).to_le_bytes()));
}

/// An NTFS volume with one shadow copy that saved block 10 (live 0xAA,
/// snapshot 0x55) into block 20. Block 11 (0xBB) never changed.
fn shadowed_volume() -> Vec<u8> {
    let mut volume = vec![0u8; VOLUME_BLOCKS * BLOCK];
    put(&mut volume, 3, b"NTFS    ");
    put(&mut volume, 0x1E00, &vsc::VSS_IDENTIFIER);
    put(&mut volume, 0x1E00 + 16, &1u32.to_le_bytes());
    put(&mut volume, 0x1E00 + 20, &1u32.to_le_bytes());
    put(&mut volume, 0x1E00 + 24, &0x1E00u64.to_le_bytes());
    put(&mut volume, 0x1E00 + 48, &(BLOCK as u64).to_le_bytes());

    let catalog = BLOCK;
    put(&mut volume, catalog, &vsc::VSS_IDENTIFIER);
    put(&mut volume, catalog + 16, &1u32.to_le_bytes());
    put(&mut volume, catalog + 20, &2u32.to_le_bytes());
    let snapshot = catalog + 128;
    let volume_size = volume.len() as u64;
    put(&mut volume, snapshot, &2u64.to_le_bytes());
    put(&mut volume, snapshot + 8, &volume_size.to_le_bytes());
    put(&mut volume, snapshot + 16, &STORE_ID);
    put(&mut volume, snapshot + 32, &1u64.to_le_bytes());
    put(&mut volume, snapshot + 48, &CREATED.to_le_bytes());
    let pointer = catalog + 256;
    put(&mut volume, pointer, &3u64.to_le_bytes());
    put(
        &mut volume,
        pointer + 8,
        &((3 * BLOCK) as u64).to_le_bytes(),
    );
    put(&mut volume, pointer + 16, &STORE_ID);
    put(
        &mut volume,
        pointer + 32,
        &((2 * BLOCK) as u64).to_le_bytes(),
    );
    put(
        &mut volume,
        pointer + 48,
        &((4 * BLOCK) as u64).to_le_bytes(),
    );

    store_block(&mut volume, 2, 4);
    store_block(&mut volume, 3, 3);
    let descriptor = 3 * BLOCK + 128;
    put(
        &mut volume,
        descriptor,
        &((10 * BLOCK) as u64).to_le_bytes(),
    );
    put(
        &mut volume,
        descriptor + 16,
        &((20 * BLOCK) as u64).to_le_bytes(),
    );
    store_block(&mut volume, 4, 6);

    volume[10 * BLOCK..11 * BLOCK].fill(0xAA);
    volume[11 * BLOCK..12 * BLOCK].fill(0xBB);
    volume[20 * BLOCK..21 * BLOCK].fill(0x55);
    volume
}

/// A disk with an empty FAT partition and the shadowed NTFS volume.
fn disk_with_shadow_copy() -> Arc<DiskImage> {
    let mut disk = vec![0u8; PARTITION_START];
    disk.extend_from_slice(&shadowed_volume());
    let partitions = vec![
        Partition {
            id: 1,
            kind: PartitionKind::Fat32,
            start_lba: 1,
            length_lba: 8,
            label: None,
            unique_guid: None,
        },
        Partition {
            id: 2,
            kind: PartitionKind::Ntfs,
            start_lba: (PARTITION_START / 512) as u64,
            length_lba: (VOLUME_BLOCKS * BLOCK / 512) as u64,
            label: None,
            unique_guid: None,
        },
    ];
    let image = DiskImage::from_assembled(
        "vss-test".into(),
        Arc::new(MemorySource(disk)),
        partitions,
        512,
    )
    .unwrap();
    Arc::new(image)
}

#[test]
fn lists_shadow_copies_of_ntfs_partitions_only() {
    let image = disk_with_shadow_copy();
    let volumes = list(&image);
    assert_eq!(volumes.len(), 1);
    let volume = &volumes[0];
    assert_eq!(
        (
            volume.partition_id,
            volume.volume_size,
            volume.error.as_deref()
        ),
        (2, (VOLUME_BLOCKS * BLOCK) as u64, None)
    );
    assert_eq!(volume.shadow_copies.len(), 1);
    let copy = &volume.shadow_copies[0];
    assert_eq!(copy.index, 0);
    assert_eq!(copy.store_id, "11223344-5566-7788-99aa-bbccddeeff00");
    assert_eq!(copy.creation_filetime, CREATED);
    assert_eq!(copy.created_unix_ms, Some(1_619_890_803_223));
    assert_eq!(copy.unavailable_reason, None);
}

#[test]
fn opens_a_shadow_copy_as_a_reconstructed_session_image() {
    let image = disk_with_shadow_copy();
    let snapshot = open(&image, 2, 0, &CancellationToken::new()).unwrap();

    assert_eq!(snapshot.len(), (VOLUME_BLOCKS * BLOCK) as u64);
    assert_eq!(
        snapshot.read_range((10 * BLOCK) as u64, BLOCK).unwrap(),
        vec![0x55; BLOCK]
    );
    assert_eq!(
        snapshot.read_range((11 * BLOCK) as u64 + 100, 300).unwrap(),
        vec![0xBB; 300]
    );
    // The live volume still holds the newer bytes.
    assert_eq!(
        image
            .read_range((PARTITION_START + 10 * BLOCK) as u64, 4)
            .unwrap(),
        vec![0xAA; 4]
    );

    let origin = ShadowCopyOrigin {
        parent_image_id: image.id.clone(),
        partition_id: 2,
        index: 0,
        store_id: "11223344-5566-7788-99aa-bbccddeeff00".into(),
        shadow_copy_id: Some("00000000-0000-0000-0000-000000000000".into()),
        created_unix_ms: Some(1_619_890_803_223),
    };
    let path = snapshot.path.to_string_lossy().into_owned();
    assert!(path.starts_with(&session_path_prefix(&image.id, 2, &origin.store_id)));
    assert!(path.starts_with(SESSION_PREFIX));
    assert!(path.ends_with("分区2_卷影副本1_2021-05-01_17-40-03_UTC"));
    match &snapshot.derived {
        Some(DerivedImage::ShadowCopy {
            parent,
            origin: actual,
        }) => {
            assert_eq!((parent, actual), (&image.id, &origin));
        }
        other => panic!("expected a shadow-copy image, got {other:?}"),
    }
    let info = snapshot.info();
    assert_eq!(info.source_format, ImageFormat::ShadowCopy);
    assert_eq!(info.shadow_copy, Some(origin));
}

#[test]
fn closed_source_never_yields_snapshot_bytes() {
    let image = disk_with_shadow_copy();
    let snapshot = open(&image, 2, 0, &CancellationToken::new()).unwrap();
    drop(image);
    assert!(snapshot.read_range((11 * BLOCK) as u64, 512).is_err());
}

fn image_from_volume(volume: Vec<u8>) -> Arc<DiskImage> {
    let length = volume.len() as u64;
    Arc::new(
        DiskImage::from_assembled(
            "vss-integrity".into(),
            Arc::new(MemorySource(volume)),
            vec![Partition {
                id: 0,
                kind: PartitionKind::Ntfs,
                start_lba: 0,
                length_lba: length / 512,
                label: None,
                unique_guid: None,
            }],
            512,
        )
        .unwrap(),
    )
}

#[test]
fn corrupt_catalog_is_reported_instead_of_an_empty_shadow_list() {
    let mut volume = shadowed_volume();
    put(&mut volume, BLOCK + 40, &(BLOCK as u64).to_le_bytes());
    let image = image_from_volume(volume);
    let listed = list(&image);
    assert_eq!(listed.len(), 1);
    assert!(listed[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("reached twice")));
    assert!(open(&image, 0, 0, &CancellationToken::new()).is_err());
}

#[test]
fn corrupt_store_header_is_unavailable_in_listing_and_cannot_open() {
    let mut volume = shadowed_volume();
    put(&mut volume, 2 * BLOCK + 20, &3u32.to_le_bytes());
    let image = image_from_volume(volume);
    let listed = list(&image);
    assert!(listed[0].shadow_copies[0].unavailable_reason.is_some());
    assert!(listed[0].shadow_copies[0].shadow_copy_id.is_none());
    assert!(open(&image, 0, 0, &CancellationToken::new()).is_err());
}

#[test]
fn corrupt_catalog_size_opens_with_verified_ntfs_length() {
    let mut volume = shadowed_volume();
    let mut boot = [0; 512];
    put(&mut boot, 3, b"NTFS    ");
    put(&mut boot, 11, &512u16.to_le_bytes());
    put(
        &mut boot,
        40,
        &((volume.len() as u64 / 512) - 1).to_le_bytes(),
    );
    put(&mut boot, 510, &[0x55, 0xaa]);
    put(&mut volume, 0, &boot);
    let backup = volume.len() - 512;
    put(&mut volume, backup, &boot);
    put(&mut volume, BLOCK + 128 + 8, &u64::MAX.to_le_bytes());
    let image = image_from_volume(volume);
    assert_eq!(list(&image)[0].shadow_copies[0].volume_size, u64::MAX);
    let snapshot = open(&image, 0, 0, &CancellationToken::new()).unwrap();
    assert_eq!(snapshot.len(), (VOLUME_BLOCKS * BLOCK) as u64);
    assert_eq!(
        snapshot.read_range((10 * BLOCK) as u64, 512).unwrap(),
        vec![0x55; 512]
    );
}

#[test]
fn rejects_volumes_without_shadow_copies_and_bad_requests() {
    let image = disk_with_shadow_copy();
    let token = CancellationToken::new();
    assert!(open(&image, 1, 0, &token).is_err(), "FAT partition");
    assert!(open(&image, 2, 1, &token).is_err(), "only one shadow copy");
    assert!(open(&image, 9, 0, &token).is_err(), "no such partition");
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(open(&image, 2, 0, &cancelled).is_err());

    let plain = Arc::new(
        DiskImage::from_assembled(
            "plain".into(),
            Arc::new(MemorySource(vec![0u8; VOLUME_BLOCKS * BLOCK])),
            vec![Partition {
                id: 0,
                kind: PartitionKind::Ntfs,
                start_lba: 0,
                length_lba: (VOLUME_BLOCKS * BLOCK / 512) as u64,
                label: None,
                unique_guid: None,
            }],
            512,
        )
        .unwrap(),
    );
    assert!(list(&plain).is_empty());
    assert!(open(&plain, 0, 0, &token).is_err());
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Env-gated end-to-end check on dfVFS `test_data/vss.raw` (commit
/// `0778d3fb20728cbf5c60e14b702527eab0b1c227`): an NTFS volume with two shadow
/// copies. The digests are pyvshadow's reconstruction of each store
/// (libvshadow-python 20260714), also asserted by the vendored reader's
/// `tests/oracle_dfvfs.rs`.
#[test]
fn reconstructs_dfvfs_shadow_copies_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_VSS_TEST") else {
        eprintln!("LOVELYCARVE_VSS_TEST not set; skipping the dfVFS vss.raw check");
        return;
    };
    let expected = [
        (
            "de81cc22-aa8b-11eb-9339-8cdcd4557abc",
            "d830fe9e2f75e66edfe0f9b0730c0ec8148233ed9c2ab5601195c293d593d9a9",
        ),
        (
            "de81cc2b-aa8b-11eb-9339-8cdcd4557abc",
            "d47ca0497f4aa3098a7d0b5407a6406a4f8d747e6207fdbc197e2224fb4838ae",
        ),
    ];
    let image = Arc::new(DiskImage::open(std::path::Path::new(&path)).unwrap());
    let volumes = list(&image);
    assert_eq!(volumes.len(), 1, "{volumes:?}");
    let volume = &volumes[0];
    let stores: Vec<&str> = volume
        .shadow_copies
        .iter()
        .map(|copy| copy.store_id.as_str())
        .collect();
    assert_eq!(stores, expected.map(|(store, _)| store));
    let root_names = |image: &Arc<DiskImage>| -> Vec<String> {
        let session = Arc::new(
            crate::core::filesystem::evidence_session::EvidenceSession::from(Arc::clone(image)),
        );
        let volumes = children_for_image(&session, "root").unwrap();
        assert_eq!(volumes.len(), 1);
        let mut names: Vec<String> = children_for_image(&session, &volumes[0].id)
            .unwrap()
            .into_iter()
            .filter(|node| !node.deleted)
            .map(|node| node.name)
            .collect();
        names.sort();
        names
    };
    // dfVFS created `vss1` after the first snapshot and `vss2` after the
    // second, so each point-in-time view shows only the earlier markers.
    let markers = |names: &[String]| -> Vec<bool> {
        ["vss1", "vss2"]
            .iter()
            .map(|marker| names.iter().any(|name| name == marker))
            .collect()
    };
    assert_eq!(markers(&root_names(&image)), [true, true]);
    let expected_markers = [[false, false], [true, false]];
    for (index, (_, digest)) in expected.iter().enumerate() {
        let snapshot = open(
            &image,
            volume.partition_id,
            index,
            &CancellationToken::new(),
        )
        .unwrap();
        let bytes = snapshot.read_range(0, snapshot.len() as usize).unwrap();
        assert_eq!(sha256_hex(&bytes), *digest, "shadow copy {}", index + 1);
        assert_eq!(snapshot.partitions.len(), 1);
        assert_eq!(snapshot.partitions[0].kind, PartitionKind::Ntfs);
        // The NTFS reader browses the reconstructed volume like any other.
        let names = root_names(&Arc::new(snapshot));
        assert!(
            names.iter().any(|name| name == "passwords.txt"),
            "{names:?}"
        );
        assert_eq!(
            markers(&names),
            expected_markers[index],
            "shadow copy {}",
            index + 1
        );
    }
}
