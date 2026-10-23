//! The path-resolution layer over a real frontend: read by path, follow a real
//! symlink, list a directory by path — the surface a host (LamBoot `FsBackend`)
//! consumes. Same `erofs.img` fixture as `erofs_base.rs`.

use lamfold::{
    metadata_path, read_dir_path, read_path, BlockCache, FileKind, FoldFrontend, NoVerifier,
    SliceSource, SubstrateCtx,
};
use lamfold_erofs::Erofs;

const IMG: &[u8] = include_bytes!("fixtures/erofs.img");

#[test]
fn reads_by_path_and_follows_symlink() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Erofs::open(SliceSource::new(IMG), &mut cx).unwrap();

    assert_eq!(
        read_path(&mut fs, &mut cx, "/readme.txt").unwrap(),
        b"erofs root file\n"
    );
    assert_eq!(
        read_path(&mut fs, &mut cx, "/sub/inner.txt").unwrap(),
        b"erofs nested\n"
    );

    // /link.txt -> readme.txt : read_path follows the symlink to the target.
    assert_eq!(
        read_path(&mut fs, &mut cx, "/link.txt").unwrap(),
        b"erofs root file\n"
    );
    // metadata_path reports the target's kind (Regular), not Symlink.
    assert_eq!(
        metadata_path(&mut fs, &mut cx, "/link.txt").unwrap().kind,
        FileKind::Regular
    );

    let big = read_path(&mut fs, &mut cx, "/big.dat").unwrap();
    assert_eq!(big.len(), 300_000);
    assert!(big.iter().enumerate().all(|(i, &b)| b == (i % 256) as u8));

    let mut names: Vec<String> = read_dir_path(&mut fs, &mut cx, "/sub")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    assert_eq!(names, vec!["inner.txt".to_string()]);
}
