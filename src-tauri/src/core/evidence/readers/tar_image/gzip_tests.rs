use super::*;
use crate::core::evidence::image::{DiskImage, ImageFormat};
use flate2::{write::GzEncoder, Compression};

fn fixture() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, data) in [
        ("first.txt", b"first".as_slice()),
        ("second.txt", b"second"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, data).unwrap();
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

#[test]
fn gzip_members_split_tar_headers_and_keep_original_evidence_identity() {
    let tar = fixture();
    let compressed = gzip(&[b"", &tar[..600], &tar[600..1300], &tar[1300..], b""]);
    for suffix in [".tgz", ".tar.gz"] {
        let mut file = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
        file.write_all(&compressed).unwrap();
        file.flush().unwrap();
        let image = DiskImage::open(file.path()).unwrap();
        assert_eq!(image.path, file.path());
        assert_eq!(image.info().source_format, ImageFormat::Tar);
        let archive = image.tar.as_ref().unwrap();
        assert_eq!(archive.path(), file.path());
        assert_eq!(archive.read("first.txt", usize::MAX).unwrap(), b"first");
        assert_eq!(archive.read("second.txt", usize::MAX).unwrap(), b"second");
        assert_eq!(image.read_range(0, tar.len()).unwrap(), tar);
        assert_eq!(std::fs::read(file.path()).unwrap(), compressed);
    }
}

#[test]
fn gzip_late_corruption_truncation_and_junk_do_not_publish_partial_tar() {
    let tar = fixture();
    let valid = gzip(&[&tar]);
    let mut bad_crc = valid.clone();
    let offset = bad_crc.len() - 8;
    bad_crc[offset] ^= 1;
    let mut bad_size = valid.clone();
    let offset = bad_size.len() - 4;
    bad_size[offset] ^= 1;
    let mut bad_empty = gzip(&[b""]);
    let offset = bad_empty.len() - 8;
    bad_empty[offset] ^= 1;
    let mut bad_tail = valid.clone();
    bad_tail.extend(bad_empty);
    let mut junk = valid.clone();
    junk.extend(b"junk");
    for bytes in [
        bad_crc,
        bad_size,
        bad_tail,
        junk,
        valid[..valid.len() - 1].to_vec(),
    ] {
        let mut file = tempfile::Builder::new().suffix(".tgz").tempfile().unwrap();
        file.write_all(&bytes).unwrap();
        file.flush().unwrap();
        assert!(TarImage::open_gzip(file.path()).is_err());
        // The established extension probe falls back to inspectable RAW bytes;
        // it must not expose an apparently valid, partially decoded tar tree.
        let image = DiskImage::open(file.path()).unwrap();
        assert!(image.tar.is_none());
        assert_eq!(image.info().source_format, ImageFormat::Raw);
        assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
    }
}
