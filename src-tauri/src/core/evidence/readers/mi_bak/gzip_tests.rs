use super::*;
use flate2::{write::GzEncoder, Compression};
use std::io::Write;

fn tar() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data) in [
        ("first.txt", b"first".as_slice()),
        ("second.txt", b"second"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, data).unwrap();
    }
    builder.into_inner().unwrap()
}

fn gzip(chunks: &[&[u8]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for chunk in chunks {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(chunk).unwrap();
        bytes.extend(encoder.finish().unwrap());
    }
    bytes
}

fn backup(compressed: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(b"mi-prefixANDROID BACKUP\n5\n1\nnone\n")
        .unwrap();
    file.write_all(compressed).unwrap();
    file.flush().unwrap();
    file
}

#[test]
fn gzip_prefixed_backup_indexes_and_extracts_all_members() {
    let tar = tar();
    let compressed = gzip(&[b"", &tar[..600], &tar[600..], b""]);
    let file = backup(&compressed);
    let image = open_bak_archive(file.path(), "backup.bak".into()).unwrap();
    assert_eq!(image.path(), file.path());
    assert_eq!(image.label(), "backup.bak");
    assert_eq!(image.read("first.txt", usize::MAX).unwrap(), b"first");
    assert_eq!(image.read("second.txt", usize::MAX).unwrap(), b"second");
    let output = tempfile::tempdir().unwrap();
    assert_eq!(
        unpack_bak_file(file.path(), output.path()).unwrap().files,
        2
    );
    assert_eq!(
        fs::read(output.path().join("second.txt")).unwrap(),
        b"second"
    );
}

#[test]
fn gzip_backup_rejects_invalid_tail_before_extracting_any_file() {
    let tar = tar();
    let valid = gzip(&[&tar]);
    let mut bad_crc = valid.clone();
    let offset = bad_crc.len() - 8;
    bad_crc[offset] ^= 1;
    let mut bad_empty = gzip(&[b""]);
    let offset = bad_empty.len() - 8;
    bad_empty[offset] ^= 1;
    let mut bad_tail = valid.clone();
    bad_tail.extend(bad_empty);
    let mut junk = valid.clone();
    junk.extend(b"junk");
    for bytes in [bad_crc, bad_tail, junk, valid[..valid.len() - 1].to_vec()] {
        let file = backup(&bytes);
        assert!(open_bak_archive(file.path(), "backup.bak".into()).is_err());
        let output = tempfile::tempdir().unwrap();
        assert!(unpack_bak_file(file.path(), output.path()).is_err());
        assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    }
}
