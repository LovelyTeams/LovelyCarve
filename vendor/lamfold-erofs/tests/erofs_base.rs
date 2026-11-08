//! Integration test against a real `mkfs.erofs` (uncompressed) image.
//!
//! Fixture `fixtures/erofs.img` (4 KiB blocks, extended inodes, FLAT_INLINE):
//!   /readme.txt    "erofs root file\n"  (16 B  → pure inline)
//!   /sub/inner.txt "erofs nested\n"      (13 B  → pure inline)
//!   /big.dat       byte[i] = i % 256     (300000 B → 73 full blocks + 992 B inline tail)
//!   /link.txt   → readme.txt             (symlink, inline target)

use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_erofs::Erofs;

const IMG: &[u8] = include_bytes!("fixtures/erofs.img");

#[test]
fn probe_accepts_erofs() {
    let mut src = SliceSource::new(IMG);
    assert!(Erofs::probe(&mut src).unwrap());
}

#[test]
fn probe_rejects_non_erofs() {
    let junk = vec![0u8; 8192];
    let mut src = SliceSource::new(&junk);
    assert!(!Erofs::probe(&mut src).unwrap());
}

#[test]
fn walks_tree_inline_extent_and_symlink() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };

    let mut fs = Erofs::open(SliceSource::new(IMG), &mut cx).unwrap();
    let root = fs.root();

    let names: Vec<String> = fs
        .read_dir(root, &mut cx)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"readme.txt".to_string()), "got {names:?}");
    assert!(names.contains(&"big.dat".to_string()), "got {names:?}");
    assert!(names.contains(&"sub".to_string()), "got {names:?}");
    assert!(names.contains(&"link.txt".to_string()), "got {names:?}");
    // "." and ".." are navigational, not surfaced.
    assert!(!names.contains(&".".to_string()), "got {names:?}");

    // Pure-inline small file.
    let readme = fs.lookup(root, "readme.txt", &mut cx).unwrap().unwrap();
    let md = fs.metadata(readme, &mut cx).unwrap();
    assert_eq!(md.kind, FileKind::Regular);
    assert_eq!(md.size, 16);
    let mut buf = vec![0u8; 16];
    let n = fs.read_at(readme, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(&buf[..n], b"erofs root file\n");

    // Nested directory + inline file.
    let sub = fs.lookup(root, "sub", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(sub, &mut cx).unwrap().kind, FileKind::Directory);
    let inner = fs.lookup(sub, "inner.txt", &mut cx).unwrap().unwrap();
    let mut ibuf = vec![0u8; 13];
    fs.read_at(inner, 0, &mut ibuf, &mut cx).unwrap();
    assert_eq!(&ibuf, b"erofs nested\n");

    // Symlink target via read_link.
    let link = fs.lookup(root, "link.txt", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(link, &mut cx).unwrap().kind, FileKind::Symlink);
    assert_eq!(fs.read_link(link, &mut cx).unwrap().unwrap(), b"readme.txt");

    // Full-block + inline-tail file (73 full blocks at blkaddr 1, 992 B inline).
    let big = fs.lookup(root, "big.dat", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(big, &mut cx).unwrap().size, 300_000);
    let mut bbuf = vec![0u8; 300_000];
    let bn = fs.read_at(big, 0, &mut bbuf, &mut cx).unwrap();
    assert_eq!(bn, 300_000);
    assert!(bbuf.iter().enumerate().all(|(i, &b)| b == (i % 256) as u8));

    // Partial read straddling the full-block→inline-tail boundary (299008).
    let mut mid = vec![0u8; 400];
    let mn = fs.read_at(big, 298_900, &mut mid, &mut cx).unwrap();
    assert_eq!(mn, 400);
    assert!(mid
        .iter()
        .enumerate()
        .all(|(i, &b)| b == ((298_900 + i) % 256) as u8));

    assert_eq!(fs.read_at(big, 300_000, &mut bbuf, &mut cx).unwrap(), 0);
    assert!(fs.lookup(root, "nope", &mut cx).unwrap().is_none());
}
