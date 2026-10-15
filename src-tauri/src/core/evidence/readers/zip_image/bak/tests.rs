use super::*;
use flate2::{write::ZlibEncoder, Compression};
use std::io::Write;
use zip::{write::SimpleFileOptions, ZipWriter};

fn backup(compressed: bool) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(7);
    header.set_mode(0o640);
    header.set_mtime(1_700_000_000);
    header.set_cksum();
    builder
        .append_data(&mut header, "apps/com.example/db/data", &b"payload"[..])
        .unwrap();
    let mut link = tar::Header::new_gnu();
    link.set_mode(0o777);
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    builder
        .append_link(&mut link, "apps/com.example/link", "db/data")
        .unwrap();
    builder
        .append_link(&mut link, "escape", "../outside.txt")
        .unwrap();
    let tar = builder.into_inner().unwrap();
    let mut bytes = b"MIUI BACKUP\0prefixANDROID BACKUP\n5\n".to_vec();
    if compressed {
        bytes.extend(b"1\nnone\n");
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar).unwrap();
        bytes.extend(encoder.finish().unwrap());
    } else {
        bytes.extend(b"0\nnone\n");
        bytes.extend(tar);
    }
    bytes
}

fn zip(bytes: &[u8], method: CompressionMethod) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut writer = ZipWriter::new(file.reopen().unwrap());
    let options = SimpleFileOptions::default().compression_method(method);
    writer.start_file("MIUI/backup/app.BAK", options).unwrap();
    writer.write_all(bytes).unwrap();
    writer.start_file("outside.txt", options).unwrap();
    writer.write_all(b"outside").unwrap();
    writer.finish().unwrap();
    file
}

#[test]
fn auto_expands_stored_and_deflated_backups_with_lazy_reads() {
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        for compressed in [false, true] {
            let file = zip(&backup(compressed), method);
            let original = std::fs::read(file.path()).unwrap();
            let image = ZipImage::open(file.path()).unwrap();
            let root = "MIUI/backup/app.BAK";
            let path = format!("{root}/apps/com.example/db/data");
            assert_eq!(image.lookup(root).unwrap().kind, ZipEntryKind::Dir);
            assert_eq!(image.children(root).len(), 2);
            assert_eq!(image.total_size(), 14);
            assert_eq!(image.read(&path, 3).unwrap(), b"pay");
            assert_eq!(image.read(&path, usize::MAX).unwrap(), b"payload");
            assert!(image.data_location(&path).is_none());
            assert!(image.data_location(root).is_none());
            let node = image.lookup(&path).unwrap();
            assert_eq!(node.mode, 0o640);
            assert_eq!(node.mtime, 1_700_000_000);
            let mut streamed = Vec::new();
            assert_eq!(
                image
                    .stream_file(&path, 2, |chunk| {
                        streamed.extend_from_slice(chunk);
                        Ok(())
                    })
                    .unwrap(),
                7
            );
            assert_eq!(streamed, b"payload");
            assert_eq!(
                image
                    .read(&format!("{root}/apps/com.example/link"), 7)
                    .unwrap(),
                b"payload"
            );
            assert!(image.read(&format!("{root}/escape"), usize::MAX).is_err());
            assert_eq!(image.outer_bytes(), original);
            assert_eq!(std::fs::read(file.path()).unwrap(), original);
        }
    }
}

#[test]
fn invalid_and_encrypted_backups_remain_readable_files() {
    for bytes in [
        b"ordinary database backup".as_slice(),
        b"prefixANDROID BACKUP\n5\n0\nAES-256\n",
        b"prefixANDROID BACKUP\n5\n0\nnone\nnot a tar",
    ] {
        let file = zip(bytes, CompressionMethod::Deflated);
        let image = ZipImage::open(file.path()).unwrap();
        assert!(image.bak_archives.is_empty());
        assert_eq!(
            image.lookup("MIUI/backup/app.BAK").unwrap().kind,
            ZipEntryKind::File
        );
        assert_eq!(
            image.read("MIUI/backup/app.BAK", usize::MAX).unwrap(),
            bytes
        );
        assert_eq!(image.total_size(), bytes.len() as u64 + 7);
    }
}

#[test]
fn android_ab_member_expands_with_its_own_path() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut writer = ZipWriter::new(file.reopen().unwrap());
    writer
        .start_file("phone.ab", SimpleFileOptions::default())
        .unwrap();
    let bytes = backup(true);
    let marker = memchr::memmem::find(&bytes, b"ANDROID BACKUP\n").unwrap();
    writer.write_all(&bytes[marker..]).unwrap();
    writer.finish().unwrap();
    let image = ZipImage::open(file.path()).unwrap();
    assert_eq!(image.lookup("phone.ab").unwrap().kind, ZipEntryKind::Dir);
    assert_eq!(
        image.read("phone.ab/apps/com.example/db/data", 7).unwrap(),
        b"payload"
    );
}

#[test]
fn bad_zip_crc_does_not_publish_backup_tree() {
    // The valid zlib stream ends before this member's extra tail; consuming
    // only the payload would miss the enclosing ZIP CRC failure.
    let mut bytes = backup(true);
    bytes.extend(b"tail");
    let file = zip(&bytes, CompressionMethod::Stored);
    let offset = {
        let mut archive = ZipArchive::new(file.reopen().unwrap()).unwrap();
        let offset = archive.by_index(0).unwrap().data_start().unwrap();
        offset
    };
    let mut content = std::fs::read(file.path()).unwrap();
    content[offset as usize + bytes.len() - 1] ^= 1;
    std::fs::write(file.path(), content).unwrap();
    let image = ZipImage::open(file.path()).unwrap();
    assert!(image.bak_archives.is_empty());
    assert_eq!(
        image.lookup("MIUI/backup/app.BAK").unwrap().kind,
        ZipEntryKind::File
    );
}

#[test]
fn indexes_real_xiaomi_zip_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_MI_ZIP_TEST") else {
        return;
    };
    let image = ZipImage::open(&path).expect("open real Xiaomi ZIP");
    let mut archive = ZipArchive::new(File::open(&path).unwrap()).unwrap();
    let mut count = 0;
    for index in 0..archive.len() {
        let member = archive.by_index(index).unwrap();
        let name = member.name().unwrap();
        if !name.to_ascii_lowercase().ends_with(".bak") {
            continue;
        }
        count += 1;
        assert_eq!(image.lookup(&name).unwrap().kind, ZipEntryKind::Dir);
    }
    assert!(count > 0);
    assert_eq!(image.bak_archives.len(), count);
    let mut files = 0usize;
    for backup in &image.bak_archives {
        for entry in backup
            .entries()
            .filter(|entry| entry.kind == TarEntryKind::File)
        {
            let path = format!("{}/{}", backup.label(), entry.rel_path);
            assert_eq!(image.lookup(&path).unwrap().size, entry.size);
            assert!(image.data_location(&path).is_none());
            // Exercise each backup's byte routing without rereading large APKs.
            if files.is_multiple_of(100) {
                assert_eq!(
                    image.read(&path, 64).unwrap().len() as u64,
                    entry.size.min(64)
                );
            }
            files += 1;
        }
    }
    assert!(files > 0);
    if let Some(oracle_path) = std::env::var_os("LOVELYCARVE_MI_ZIP_ORACLE") {
        let oracle: serde_json::Value =
            serde_json::from_slice(&std::fs::read(oracle_path).unwrap()).unwrap();
        let expected = oracle["backups"].as_array().unwrap();
        assert_eq!(expected.len(), count);
        for expected in expected {
            // Decode legacy ZIP names through the same central-directory
            // entry. Independent readers may choose different legacy codecs.
            let member = archive
                .by_index(expected["member_index"].as_u64().unwrap() as usize)
                .unwrap();
            let root = member.name().unwrap();
            let backup = image
                .bak_archives
                .iter()
                .find(|backup| backup.label() == root)
                .unwrap();
            let actual_files: Vec<_> = backup
                .entries()
                .filter(|entry| entry.kind == TarEntryKind::File)
                .collect();
            assert_eq!(
                actual_files.len() as u64,
                expected["files"].as_u64().unwrap()
            );
            assert_eq!(backup.total_size(), expected["bytes"].as_u64().unwrap());
            if let Some(sample) = expected["sample"].as_object() {
                use sha2::{Digest, Sha256};
                let path = format!("{root}/{}", sample["path"].as_str().unwrap());
                let bytes = image.read(&path, usize::MAX).unwrap();
                assert_eq!(bytes.len() as u64, sample["size"].as_u64().unwrap());
                assert_eq!(
                    hex::encode(Sha256::digest(&bytes)),
                    sample["sha256"].as_str().unwrap()
                );
            }
        }
    }
    eprintln!(
        "Xiaomi ZIP: {count} backups, {files} files, {} total entries",
        image.entry_count()
    );
}
