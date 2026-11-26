use std::io::Read;

use flate2::read::ZlibDecoder;
use lamfold::{BlockCache, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};

use crate::UnixwareBfs;

const EMPTY: &[u8] = include_bytes!("../tests/fixtures/util-linux-empty.zlib");
const POPULATED: &[u8] = include_bytes!("../tests/fixtures/populated.zlib");

fn raw(compressed: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    ZlibDecoder::new(compressed)
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[test]
fn independently_formatted_volume_accepts_preallocated_root_slack() {
    let bytes = raw(EMPTY);
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut source = SliceSource::new(&bytes);
    assert!(UnixwareBfs::probe(&mut source).unwrap());
    let mut fs = UnixwareBfs::open(source, &mut cx).unwrap();
    assert_eq!(fs.header().data_start, 8704);
    assert_eq!(fs.header().inode_count, 128);
    assert_eq!(fs.header().name, "TESTFS");
    assert_eq!(fs.header().volume, "STAND");
    assert_eq!(fs.metadata(fs.root(), &mut cx).unwrap().size, 32);
    assert!(fs.read_dir(fs.root(), &mut cx).unwrap().is_empty());
}

#[test]
fn preserved_names_and_inode_numbers_resolve_to_contiguous_file_bytes() {
    let bytes = raw(POPULATED);
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut fs = UnixwareBfs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let root = fs.root();
    let names = fs.read_dir(root, &mut cx).unwrap();
    assert_eq!(
        names
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["unix", "unix.old", "empty", "1234567890.dat"]
    );
    let file = fs.lookup(root, "unix", &mut cx).unwrap().unwrap();
    assert_eq!(fs.lookup(root, "unix.old", &mut cx).unwrap(), Some(file));
    let mut output = [0; 16];
    assert_eq!(fs.read_at(file, 510, &mut output[..8], &mut cx).unwrap(), 8);
    assert_eq!(&output[..8], &[254, 255, 0, 1, 2, 3, 4, 5]);
    assert_eq!(fs.read_at(file, 1280, &mut output, &mut cx).unwrap(), 9);
    assert_eq!(&output[..9], b"unixware\n");
    assert_eq!(fs.read_at(file, 1289, &mut output, &mut cx).unwrap(), 0);
    let empty = fs.lookup(root, "empty", &mut cx).unwrap().unwrap();
    assert_eq!(fs.read_at(empty, 0, &mut output, &mut cx).unwrap(), 0);
    let times = fs.timestamps(file, &mut cx).unwrap();
    assert_eq!(times.accessed, Some((1706010000, 0)));
    assert_eq!(times.modified, Some((1706010001, 0)));
    assert_eq!(times.changed, Some((1706010002, 0)));
}

#[test]
fn refuses_invalid_ranges_and_compaction_state() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let bytes = raw(POPULATED);
    assert!(matches!(
        UnixwareBfs::open(SliceSource::new(&bytes[..bytes.len() - 1]), &mut cx),
        Err(FoldError::Corrupt(_))
    ));
    let mut invalid = bytes.clone();
    invalid[20..24].copy_from_slice(&24u32.to_le_bytes());
    assert!(matches!(
        UnixwareBfs::open(SliceSource::new(&invalid), &mut cx),
        Err(FoldError::Unsupported(_))
    ));
    let mut invalid = bytes.clone();
    let inode = 512 + 64;
    invalid[inode + 12..inode + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        UnixwareBfs::open(SliceSource::new(&invalid), &mut cx),
        Err(FoldError::Corrupt(_))
    ));
    let mut invalid = bytes;
    invalid[inode + 4..inode + 8].copy_from_slice(&17u32.to_le_bytes());
    assert!(matches!(
        UnixwareBfs::open(SliceSource::new(&invalid), &mut cx),
        Err(FoldError::Corrupt(_))
    ));
}
