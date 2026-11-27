use std::io::Read;

use flate2::read::ZlibDecoder;
use lamfold::{BlockCache, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};

use crate::{DESCRIPTOR_OFFSET, SECTOR_SIZE, Xdvdfs};

fn native() -> Vec<u8> {
    let mut bytes = Vec::new();
    ZlibDecoder::new(&include_bytes!("../tests/fixtures/extract-xiso.zlib")[..])
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[test]
fn opens_independently_written_multisector_directory() {
    let bytes = native();
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut image = Xdvdfs::open(SliceSource::new(&bytes), &mut context).unwrap();
    let root = image.root();
    let textures = image
        .lookup(root, "textures", &mut context)
        .unwrap()
        .unwrap();
    let children = image.read_dir(textures, &mut context).unwrap();
    assert_eq!(children.len(), 122);
    let frame = image
        .lookup(textures, "FRAME.BIN", &mut context)
        .unwrap()
        .unwrap();
    let data: Vec<_> = (0..=255u8)
        .cycle()
        .take(256 * 19)
        .chain(b"ending".iter().copied())
        .collect();
    let mut range = [0; 12];
    assert_eq!(
        image
            .read_at(frame, 2043, &mut range, &mut context)
            .unwrap(),
        12
    );
    assert_eq!(range, data[2043..2055]);
    assert_eq!(
        image
            .read_at(frame, data.len() as u64 - 3, &mut range, &mut context)
            .unwrap(),
        3
    );
    assert_eq!(&range[..3], b"ing");
    let empty = image.lookup(root, "EMPTY", &mut context).unwrap().unwrap();
    assert_eq!(
        image.read_at(empty, 0, &mut range, &mut context).unwrap(),
        0
    );
}

#[test]
fn rejects_short_extents_and_repeated_directory_offsets() {
    let mut bytes = native();
    let descriptor = DESCRIPTOR_OFFSET as usize;
    bytes[descriptor + SECTOR_SIZE - 1] ^= 1;
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    assert!(matches!(
        Xdvdfs::open(SliceSource::new(&bytes), &mut context),
        Err(FoldError::Corrupt(_))
    ));

    let mut bytes = native();
    let root_sector =
        u32::from_le_bytes(bytes[descriptor + 20..descriptor + 24].try_into().unwrap());
    let root = root_sector as usize * SECTOR_SIZE;
    let child = usize::from(u16::from_le_bytes(
        bytes[root..root + 2].try_into().unwrap(),
    )) * 4;
    assert!(child > 0);
    bytes[root + 2..root + 4].copy_from_slice(&((child / 4) as u16).to_le_bytes());
    let mut image = Xdvdfs::open(SliceSource::new(&bytes), &mut context).unwrap();
    assert!(matches!(
        image.read_dir(image.root(), &mut context),
        Err(FoldError::Corrupt(_))
    ));

    let mut bytes = native();
    bytes[root + 4..root + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut image = Xdvdfs::open(SliceSource::new(&bytes), &mut context).unwrap();
    assert!(matches!(
        image.read_dir(image.root(), &mut context),
        Err(FoldError::Corrupt(_))
    ));
}
