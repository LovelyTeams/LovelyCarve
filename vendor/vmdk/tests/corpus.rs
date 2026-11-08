use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;
use vmdk::VmdkReader;

fn corpus_dir() -> Option<PathBuf> {
    std::env::var("CORPUS_DIR").ok().map(PathBuf::from)
}

#[test]
fn corpus_sparse_vmdk_opens_and_has_nonzero_size() {
    let Some(dir) = corpus_dir() else { return };
    let path = dir.join("sparse.vmdk");
    if !path.exists() {
        return;
    }
    let data = std::fs::read(&path).expect("read sparse.vmdk");
    let reader = VmdkReader::open(Cursor::new(data)).expect("open sparse.vmdk");
    assert!(
        reader.virtual_disk_size() > 0,
        "virtual_disk_size must be > 0"
    );
}

#[test]
fn corpus_sparse_vmdk_read_is_stable() {
    let Some(dir) = corpus_dir() else { return };
    let path = dir.join("sparse.vmdk");
    if !path.exists() {
        return;
    }
    let data = std::fs::read(&path).expect("read sparse.vmdk");
    let mut reader = VmdkReader::open(Cursor::new(data)).expect("open");
    let mut buf = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut buf).expect("read sector 0");
    // Sector 0 of a freshly-created sparse VMDK is always zeros.
    assert_eq!(
        buf, [0u8; 512],
        "sector 0 of an empty VMDK must be all zeros"
    );
}
