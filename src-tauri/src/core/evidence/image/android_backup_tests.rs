use super::*;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{file_stat, read_file_range};
use flate2::{write::ZlibEncoder, Compression};

fn backup(version: u8, compressed: bool) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o600);
    header.set_size(7);
    header.set_cksum();
    builder
        .append_data(&mut header, "apps/com.example/db/main.db", &b"payload"[..])
        .unwrap();
    let tar = builder.into_inner().unwrap();
    let mut bytes = format!(
        "ANDROID BACKUP\n{version}\n{}\nnone\n",
        u8::from(compressed)
    )
    .into_bytes();
    if compressed {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar).unwrap();
        bytes.extend(encoder.finish().unwrap());
    } else {
        bytes.extend(tar);
    }
    bytes
}

#[test]
fn android_backup_keeps_source_bytes_and_exposes_a_logical_tree() {
    for version in 1..=5 {
        for compressed in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("phone.ab");
            let original = backup(version, compressed);
            std::fs::write(&path, &original).unwrap();
            let image = Arc::new(EvidenceSession::open(&path).unwrap());
            assert_eq!(image.info().source_format, ImageFormat::AndroidBackup);
            assert!(image.partitions.is_empty());
            assert_eq!(image.len(), original.len() as u64);
            assert_eq!(image.sector_size, 1);
            assert_eq!(image.read_range(0, original.len()).unwrap(), original);
            let mut copied = vec![0; original.len()];
            assert_eq!(image.read_into(0, &mut copied).unwrap(), copied.len());
            assert_eq!(copied, original);
            let tar = image.tar.as_ref().unwrap();
            assert_eq!(
                tar.read("apps/com.example/db/main.db", 7).unwrap(),
                b"payload"
            );
            let node_id = format!("tar:{}", hex::encode("apps/com.example/db/main.db"));
            assert_eq!(read_file_range(&image, &node_id, 2, 3).unwrap(), b"ylo");
            let stat = file_stat(&image, &node_id).unwrap();
            assert_eq!(stat.size, 7);
            assert!(stat.data_runs.is_empty());
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
    }
}

#[test]
fn android_backup_detects_magic_and_miui_prefix_without_relying_on_extension() {
    let directory = tempfile::tempdir().unwrap();
    for (name, prefix) in [
        ("renamed.bin", b"".as_slice()),
        ("app.bak", b"MIUI BACKUP\n2\ncom.example Test\n-1\n0\n"),
    ] {
        let path = directory.path().join(name);
        let original = [prefix, backup(5, true).as_slice()].concat();
        std::fs::write(&path, &original).unwrap();
        let image = DiskImage::open(&path).unwrap();
        assert_eq!(image.info().source_format, ImageFormat::AndroidBackup);
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        assert!(image.tar.is_some());
    }
}

#[test]
fn android_backup_rejects_bad_headers_encryption_and_truncated_zlib() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.ab");
    let valid = backup(5, true);
    let mut bad_checksum = valid.clone();
    let last = bad_checksum.len() - 1;
    bad_checksum[last] ^= 1;
    for bytes in [
        b"not a backup".to_vec(),
        b"ANDROID BACKUP\n6\n0\nnone\n".to_vec(),
        b"ANDROID BACKUP\n5\n2\nnone\n".to_vec(),
        b"ANDROID BACKUP\n5\n0\nAES-256\n".to_vec(),
        b"ANDROID BACKUP\n5\n0\nnone".to_vec(),
        valid[..valid.len() - 1].to_vec(),
        bad_checksum,
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(DiskImage::open(&path).is_err());
    }
}

#[test]
fn android_backup_opens_independent_abe_v1_and_v2_fixtures() {
    for name in ["abe-v1.ab", "abe-v2.ab"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/android-backup")
            .join(name);
        let original = std::fs::read(&path).unwrap();
        let image = DiskImage::open(path).unwrap();
        assert_eq!(image.info().source_format, ImageFormat::AndroidBackup);
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        assert_eq!(
            image
                .tar
                .as_ref()
                .unwrap()
                .read("apps/com.example/db/main.db", 7)
                .unwrap(),
            b"payload"
        );
    }
}
