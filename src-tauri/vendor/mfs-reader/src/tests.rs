use std::io::Read;

use flate2::read::ZlibDecoder;
use lamfold::{BlockCache, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};

use crate::{ForkKind, Header, Mfs, PROBE_BYTES};

const RAW400: &[u8] = include_bytes!("../tests/fixtures/raw-400.zlib");
const RAW800: &[u8] = include_bytes!("../tests/fixtures/raw-800.zlib");
const DC400: &[u8] = include_bytes!("../tests/fixtures/dc42-400.zlib");
const DC800: &[u8] = include_bytes!("../tests/fixtures/dc42-800.zlib");

fn image(bytes: &[u8]) -> Vec<u8> {
    let mut plain = Vec::new();
    ZlibDecoder::new(bytes).read_to_end(&mut plain).unwrap();
    plain
}

#[test]
fn independently_written_volumes_keep_both_forks_and_local_dates() {
    let expected_data: Vec<_> = (0..8199).map(|n| (n * 13 + n / 47) as u8).collect();
    let expected_resource: Vec<_> = (0..4673).map(|n| (n * 7 + n / 81) as u8).collect();
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    for (fixture, dc42) in [
        (RAW400, false),
        (RAW800, false),
        (DC400, true),
        (DC800, true),
    ] {
        let bytes = image(fixture);
        let header = Header::locate(&bytes[..PROBE_BYTES], bytes.len() as u64, |_, _| {
            panic!("primary MDB must be valid")
        })
        .unwrap()
        .unwrap();
        assert_eq!(header.name, "MFS Work");
        assert_eq!(header.disk_copy(), dc42);
        let mut volume = Mfs::open(SliceSource::new(&bytes), &mut cx).unwrap();
        let root = volume.root();
        assert_eq!(volume.read_dir(root, &mut cx).unwrap().len(), 3);
        let file = volume.lookup(root, "RÉSUMÉ", &mut cx).unwrap().unwrap();
        assert!(
            volume
                .local_timestamps(file, &mut cx)
                .unwrap()
                .created
                .is_some()
        );
        assert!(volume.timestamps(file, &mut cx).unwrap().created.is_none());
        let mut range = vec![0; 3101];
        assert_eq!(
            volume
                .read_fork_at(file, ForkKind::Data, 1001, &mut range)
                .unwrap(),
            3101
        );
        assert_eq!(range, expected_data[1001..4102]);
        assert_eq!(
            volume
                .read_fork_at(file, ForkKind::Resource, 2017, &mut range)
                .unwrap(),
            2656
        );
        assert_eq!(range[..2656], expected_resource[2017..]);
        let only = volume
            .lookup(root, "Only Resource", &mut cx)
            .unwrap()
            .unwrap();
        assert_eq!(volume.fork_size(only, ForkKind::Data).unwrap(), 0);
        assert_eq!(volume.fork_size(only, ForkKind::Resource).unwrap(), 4673);
        assert_eq!(
            volume
                .read_fork_at(only, ForkKind::Resource, 0, &mut range)
                .unwrap(),
            range.len()
        );
        assert_eq!(range, expected_resource[..3101]);
    }
}

fn update_map(bytes: &mut [u8], index: usize, value: u16) {
    let start = 1088 + index * 3 / 2;
    let old = u16::from_be_bytes(bytes[start..start + 2].try_into().unwrap());
    let value = if index.is_multiple_of(2) {
        (old & 15) | (value << 4)
    } else {
        (old & 0xf000) | value
    };
    bytes[start..start + 2].copy_from_slice(&value.to_be_bytes());
}

#[test]
fn reads_reserved_directory_blocks_and_validated_alternate_mdb() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut bytes = image(RAW400);
    let target = 8192 + 300 * 1024;
    bytes.copy_within(2048..8192, target);
    bytes[1038..1040].copy_from_slice(&((target / 512) as u16).to_be_bytes());
    for index in 300..306 {
        update_map(&mut bytes, index, 0xfff);
    }
    let mut volume = Mfs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    assert_eq!(volume.read_dir(volume.root(), &mut cx).unwrap().len(), 3);
    let alternate = bytes.len() - 1024;
    bytes.copy_within(1024..2048, alternate);
    bytes[1024..1026].fill(0);
    let mut volume = Mfs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    assert!(volume.header.backup());
    assert_eq!(volume.read_dir(volume.root(), &mut cx).unwrap().len(), 3);
}

#[test]
fn rejects_bad_disk_copy_checksums_and_broken_fork_chains() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    for at in [84 + 4, 84 + 409600 + 12] {
        let mut bytes = image(DC400);
        bytes[at] ^= 0x40;
        assert!(matches!(
            Mfs::open(SliceSource::new(&bytes), &mut cx),
            Err(FoldError::Corrupt(_))
        ));
    }
    let mut bytes = image(RAW400);
    let first = crate::be16(&bytes[2048..], 22);
    update_map(&mut bytes, usize::from(first - 2), 1);
    let mut volume = Mfs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let file = volume
        .lookup(volume.root(), "Résumé", &mut cx)
        .unwrap()
        .unwrap();
    assert!(matches!(
        volume.read_fork_at(file, ForkKind::Data, 0, &mut [0; 64]),
        Err(FoldError::Corrupt(_))
    ));
}

#[test]
fn displays_same_name_versions_as_distinct_files() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut bytes = image(RAW400);
    let first = 2048;
    let len = usize::from(bytes[first + 50]);
    let second = first + (51 + len).next_multiple_of(2);
    let third = second + (51 + usize::from(bytes[second + 50])).next_multiple_of(2);
    bytes[third + 1] = 7;
    bytes.copy_within(first + 50..first + 51 + len, third + 50);
    let mut volume = Mfs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let live = volume
        .lookup(volume.root(), "Résumé", &mut cx)
        .unwrap()
        .unwrap();
    let version = volume
        .lookup(volume.root(), "RÉSUMÉ ⟨v7⟩", &mut cx)
        .unwrap()
        .unwrap();
    assert_ne!(live, version);
    assert_eq!(volume.fork_size(version, ForkKind::Data).unwrap(), 0);
}
