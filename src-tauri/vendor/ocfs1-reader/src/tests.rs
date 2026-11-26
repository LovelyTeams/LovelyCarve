use crate::Ocfs1;
use flate2::read::ZlibDecoder;
use lamfold::{BlockCache, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use std::io::Read;

fn image() -> Vec<u8> {
    let mut raw = Vec::new();
    ZlibDecoder::new(&include_bytes!("../tests/fixtures/oraclecfs-12.zlib")[..])
        .read_to_end(&mut raw)
        .unwrap();
    raw
}

#[test]
fn directory_chains_local_files_and_external_extent_tree() {
    let raw = image();
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = Ocfs1::open(SliceSource::new(&raw), &mut context).unwrap();
    assert_eq!(reader.label(), "ocfs-lab");
    let root = reader.root();
    let files = reader.read_dir(root, &mut context).unwrap();
    assert_eq!(
        files
            .iter()
            .map(|file| file.name.as_str())
            .collect::<Vec<_>>(),
        ["archive", "control.bin", "shortcut", "empty.dbf"]
    );
    let control = reader
        .lookup(root, "control.bin", &mut context)
        .unwrap()
        .unwrap();
    let mut bytes = [0; 720];
    assert_eq!(
        reader
            .read_at(control, 0, &mut bytes, &mut context)
            .unwrap(),
        715
    );
    assert_eq!(
        &bytes[..715],
        &(0..715).map(|i| (i ^ 0xA5) as u8).collect::<Vec<_>>()
    );
    let empty = reader
        .lookup(root, "empty.dbf", &mut context)
        .unwrap()
        .unwrap();
    assert_eq!(
        reader.read_at(empty, 0, &mut bytes, &mut context).unwrap(),
        0
    );
    let dir = reader
        .lookup(root, "archive", &mut context)
        .unwrap()
        .unwrap();
    let file = reader
        .lookup(dir, "archive-file.dbf", &mut context)
        .unwrap()
        .unwrap();
    let link = reader
        .lookup(root, "shortcut", &mut context)
        .unwrap()
        .unwrap();
    assert_eq!(
        reader.read_link(link, &mut context).unwrap().as_deref(),
        Some(b"archive/archive-file.dbf".as_slice())
    );
    let mut reopened = Ocfs1::open(SliceSource::new(&raw), &mut context).unwrap();
    let size = reopened.metadata(file, &mut context).unwrap().size;
    assert_eq!(size, 4 * 131_072 - 71);
    assert_eq!(
        reopened.timestamps(file, &mut context).unwrap().created,
        Some((1_059_525_553, 0))
    );
    let oracle: Vec<_> = (0..size)
        .map(|offset| {
            let local = offset % 131_072;
            (local * 29 + local / 193 + (offset / 131_072) * 7) as u8
        })
        .collect();
    for (offset, requested) in [
        (0, size as usize),
        (131_060, 85),
        (262_140, 488),
        (size - 15, 32),
    ] {
        let mut range = vec![0; requested];
        let copied = reopened
            .read_at(file, offset, &mut range, &mut context)
            .unwrap();
        assert_eq!(copied, requested.min((size - offset) as usize));
        assert_eq!(
            range[..copied],
            oracle[offset as usize..offset as usize + copied]
        );
    }
}

#[test]
fn validates_recovery_slot_clusters_tree_pointers_and_volume_version() {
    let mut raw = image();
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    raw[0x4200 + 12] = 1;
    assert!(matches!(
        Ocfs1::open(SliceSource::new(&raw), &mut context),
        Err(FoldError::Unsupported("ocfs1: node requires recovery"))
    ));
    raw[0x4200 + 12] = 0;
    raw[4] = 2;
    assert!(matches!(
        Ocfs1::open(SliceSource::new(&raw), &mut context),
        Err(FoldError::Unsupported("ocfs1: volume revision"))
    ));
    raw[4] = 1;
    raw[0xC000] &= !(1 << 2);
    assert!(matches!(
        Ocfs1::open(SliceSource::new(&raw), &mut context),
        Err(FoldError::Corrupt("ocfs1: reference to a free cluster"))
    ));
    raw[0xC000] |= 1 << 2;
    raw[0xD0200 + 40..0xD0200 + 48].copy_from_slice(&0xD0200u64.to_le_bytes());
    let mut reader = Ocfs1::open(SliceSource::new(&raw), &mut context).unwrap();
    assert!(matches!(
        reader.read_at(0x91000, 0, &mut [0], &mut context),
        Err(FoldError::Corrupt(
            "ocfs1: extent leaf links disagree with tree"
        ))
    ));
}
