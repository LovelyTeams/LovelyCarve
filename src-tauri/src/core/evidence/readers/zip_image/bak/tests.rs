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
fn standalone_backup_and_gzip_loading_observe_cancellation() {
    use crate::core::evidence::image::{DiskImage, EvidenceCache, OpenProgress};
    let payload = backup(false);
    let prefix = b"MIUI BACKUP\0prefixANDROID BACKUP\n5\n0\nnone\n";
    assert!(payload.starts_with(prefix));
    for suffix in [".ab", ".tar.gz"] {
        let mut file = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
        if suffix == ".ab" {
            file.write_all(&payload[b"MIUI BACKUP\0prefix".len()..])
                .unwrap();
        } else {
            let mut encoder =
                flate2::write::GzEncoder::new(file.as_file_mut(), Compression::default());
            encoder.write_all(&payload[prefix.len()..]).unwrap();
            encoder.finish().unwrap();
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        let error = DiskImage::open_cancellable(
            "cancelled".into(),
            file.path(),
            &|event| {
                if matches!(event, OpenProgress::Decompressing { done, .. } if done > 0) {
                    cancel.cancel();
                }
            },
            None,
            EvidenceCache::Disabled,
            &cancel,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("cancelled"), "{error:#}");
    }
}

#[test]
fn cancelled_backup_loading_never_publishes_a_partial_zip_or_raw_fallback() {
    use crate::core::evidence::image::{DiskImage, EvidenceCache, OpenProgress};
    let file = tempfile::Builder::new().suffix(".zip").tempfile().unwrap();
    let mut writer = ZipWriter::new(file.reopen().unwrap());
    writer
        .start_file("app.bak", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(&backup(false)).unwrap();
    writer.finish().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let error = DiskImage::open_cancellable(
        "cancelled".into(),
        file.path(),
        &|event| {
            if matches!(event, OpenProgress::Decompressing { done, .. } if done > 0) {
                cancel.cancel();
            }
        },
        None,
        EvidenceCache::Directory(cache.path()),
        &cancel,
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error:#}");
    // A cancelled decode leaves no listing for the next open to trust.
    assert!(!listing_path(cache.path()).exists());
}

#[test]
fn reports_backup_progress_through_the_observed_image_open_path() {
    use crate::core::evidence::image::{DiskImage, OpenProgress};

    let payload = backup(false);
    let file = tempfile::Builder::new().suffix(".zip").tempfile().unwrap();
    let mut writer = ZipWriter::new(file.reopen().unwrap());
    for name in ["first.bak", "second.ab"] {
        writer
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&payload).unwrap();
    }
    writer.finish().unwrap();
    let updates = std::sync::Mutex::new(Vec::new());
    let image = DiskImage::open_with_id_observed("observed".into(), file.path(), &|event| {
        if let OpenProgress::Decompressing { label, done, total } = event {
            updates.lock().unwrap().push((label, done, total));
        }
    })
    .unwrap();
    assert!(image.zip.is_some());
    let updates = updates.into_inner().unwrap();
    let total = 2 * payload.len() as u64;
    assert_eq!(updates.first().unwrap(), &("first.bak".into(), 0, total));
    assert_eq!(updates.last().unwrap(), &("second.ab".into(), total, total));
    assert!(updates
        .iter()
        .all(|(_, done, expected)| *expected == total && done <= expected));
    assert!(updates.windows(2).all(|pair| pair[0].1 <= pair[1].1));
    assert!(updates
        .iter()
        .any(|(label, done, _)| label == "first.bak" && *done == payload.len() as u64));
    assert!(updates
        .iter()
        .any(|(label, done, _)| label == "second.ab" && *done == payload.len() as u64));
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

/// Two backups (DEFLATE and STORED members, plain and zlib payloads), an
/// ordinary `.bak` that stays a file, and a plain member.
fn write_phone_zip(path: &Path, first_backup: &[u8]) {
    let mut writer = ZipWriter::new(File::create(path).unwrap());
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer
        .start_file("MIUI/backup/first.bak", deflated)
        .unwrap();
    writer.write_all(first_backup).unwrap();
    writer.start_file("MIUI/backup/second.bak", stored).unwrap();
    writer.write_all(&backup(true)).unwrap();
    writer.start_file("ordinary.bak", deflated).unwrap();
    writer.write_all(b"opaque database backup").unwrap();
    writer.start_file("notes.txt", deflated).unwrap();
    writer.write_all(b"plain member").unwrap();
    writer.finish().unwrap();
}

/// Opens `path` with `cache` as its evidence cache; returns the image and how
/// many decode progress updates the open reported.
fn open_with_cache(path: &Path, cache: &Path) -> (ZipImage, usize) {
    let updates = std::cell::Cell::new(0);
    let image = ZipImage::open_cancellable(
        path,
        EvidenceCache::Directory(cache),
        &|_, _, _| updates.set(updates.get() + 1),
        &tokio_util::sync::CancellationToken::new(),
    )
    .unwrap();
    (image, updates.get())
}

fn backup_summary(image: &ZipImage) -> Vec<(String, usize, u64)> {
    image
        .bak_archives
        .iter()
        .map(|backup| {
            (
                backup.label().to_owned(),
                backup.entry_count(),
                backup.total_size(),
            )
        })
        .collect()
}

#[test]
fn reopening_restores_backup_directories_from_the_listing_and_decodes_on_first_read() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("phone.zip");
    let cache = directory.path().join("cache");
    write_phone_zip(&archive, &backup(false));

    let (decoded, decode_updates) = open_with_cache(&archive, &cache);
    assert!(decode_updates > 0);
    assert_eq!(decoded.bak_archives.len(), 2);
    assert!(decoded
        .bak_archives
        .iter()
        .all(|backup| backup.decoded().is_some()));
    assert!(listing_path(&cache).is_file());

    let (restored, restore_updates) = open_with_cache(&archive, &cache);
    assert_eq!(restore_updates, 0);
    assert!(restored
        .bak_archives
        .iter()
        .all(|backup| backup.decoded().is_none()));
    assert_eq!(restored.entries, decoded.entries);
    assert_eq!(restored.total_size(), decoded.total_size());
    assert_eq!(backup_summary(&restored), backup_summary(&decoded));
    assert_eq!(
        restored.children("MIUI/backup/first.bak").len(),
        decoded.children("MIUI/backup/first.bak").len()
    );

    // Content reads decode only the backup they touch.
    let data = "MIUI/backup/second.bak/apps/com.example/db/data";
    assert_eq!(restored.read(data, 3).unwrap(), b"pay");
    assert!(restored.bak_archives[1].decoded().is_some());
    assert!(restored.bak_archives[0].decoded().is_none());
    let mut streamed = Vec::new();
    restored
        .stream_file("MIUI/backup/first.bak/apps/com.example/link", 2, |chunk| {
            streamed.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
    assert_eq!(streamed, b"payload");
    assert!(restored
        .read("MIUI/backup/first.bak/escape", usize::MAX)
        .is_err());
    assert_eq!(
        restored.lookup("ordinary.bak").unwrap().kind,
        ZipEntryKind::File
    );
    assert_eq!(
        restored.read("ordinary.bak", usize::MAX).unwrap(),
        b"opaque database backup"
    );
}

#[test]
fn a_changed_archive_decodes_again_instead_of_restoring_its_old_listing() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("phone.zip");
    let cache = directory.path().join("cache");
    write_phone_zip(&archive, &backup(false));
    open_with_cache(&archive, &cache);

    write_phone_zip(&archive, &backup(true));
    let (changed, updates) = open_with_cache(&archive, &cache);
    assert!(updates > 0);
    assert!(changed
        .bak_archives
        .iter()
        .all(|backup| backup.decoded().is_some()));

    // The refreshed listing serves the next open.
    let (restored, updates) = open_with_cache(&archive, &cache);
    assert_eq!(updates, 0);
    assert_eq!(restored.entries, changed.entries);
}

#[test]
fn a_backup_that_differs_from_its_listing_refuses_content_reads() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("phone.zip");
    let cache = directory.path().join("cache");
    write_phone_zip(&archive, &backup(false));
    open_with_cache(&archive, &cache);

    // Make the listing describe a member the backup does not hold.
    let listing = listing_path(&cache);
    let mut json = Vec::new();
    zstd::stream::read::Decoder::new(File::open(&listing).unwrap())
        .unwrap()
        .read_to_end(&mut json)
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
    let entries = value["members"][0]["expansion"]["entries"]
        .as_array_mut()
        .unwrap();
    let data = entries
        .iter_mut()
        .find(|entry| entry["path"] == "apps/com.example/db/data")
        .unwrap();
    data["mtime"] = serde_json::json!(1);
    let mut encoder =
        zstd::stream::write::Encoder::new(File::create(&listing).unwrap(), 3).unwrap();
    serde_json::to_writer(&mut encoder, &value).unwrap();
    encoder.finish().unwrap();

    let (restored, _) = open_with_cache(&archive, &cache);
    let path = "MIUI/backup/first.bak/apps/com.example/db/data";
    assert_eq!(restored.lookup(path).unwrap().mtime, 1);
    let error = restored.read(path, usize::MAX).unwrap_err();
    assert!(
        format!("{error:#}").contains("no longer matches its listing"),
        "{error:#}"
    );
    assert!(restored.bak_archives[0].decoded().is_none());
    // Other backups are unaffected.
    assert_eq!(
        restored
            .read(
                "MIUI/backup/second.bak/apps/com.example/db/data",
                usize::MAX
            )
            .unwrap(),
        b"payload"
    );
}

#[test]
fn restored_backups_keep_the_evidence_fingerprint() {
    use crate::core::evidence::image::DiskImage;
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("phone.zip");
    let cache = directory.path().join("cache");
    write_phone_zip(&archive, &backup(false));
    let open = |evidence_cache| {
        DiskImage::open_cancellable(
            "phone".into(),
            &archive,
            &|_| {},
            None,
            evidence_cache,
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap()
    };
    let uncached = open(EvidenceCache::Disabled);
    let decoded = open(EvidenceCache::Directory(&cache));
    let restored = open(EvidenceCache::Directory(&cache));
    assert!(restored
        .zip
        .as_ref()
        .unwrap()
        .bak_archives
        .iter()
        .all(|backup| backup.decoded().is_none()));
    assert!(restored.has_expanded_backups());
    assert_eq!(
        restored.evidence_fingerprint(),
        decoded.evidence_fingerprint()
    );
    assert_eq!(
        uncached.evidence_fingerprint(),
        decoded.evidence_fingerprint()
    );
}

#[test]
fn indexes_real_xiaomi_zip_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_MI_ZIP_TEST") else {
        return;
    };
    let cache = tempfile::tempdir().unwrap();
    let (image, _) = open_with_cache(Path::new(&path), cache.path());
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
    for index in 0..image.bak_archives.len() {
        let backup = image.backup_tar(index).unwrap();
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
                .position(|backup| backup.label() == root)
                .map(|index| image.backup_tar(index).unwrap())
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

    // Reopening restores the same projection from the listing; every backup
    // then decodes on demand and must match what the listing projected.
    let started = std::time::Instant::now();
    let (restored, updates) = open_with_cache(Path::new(&path), cache.path());
    let restore_elapsed = started.elapsed();
    assert_eq!(updates, 0);
    assert_eq!(restored.entries, image.entries);
    assert_eq!(backup_summary(&restored), backup_summary(&image));
    for index in 0..restored.bak_archives.len() {
        let tar = restored.backup_tar(index).unwrap();
        if let Some(entry) = tar
            .entries()
            .find(|entry| entry.kind == TarEntryKind::File && entry.size > 0)
        {
            let member = format!("{}/{}", tar.label(), entry.rel_path);
            assert_eq!(
                restored.read(&member, 4096).unwrap(),
                image.read(&member, 4096).unwrap()
            );
        }
    }
    eprintln!(
        "Xiaomi ZIP: {count} backups, {files} files, {} total entries; restored from listing in {restore_elapsed:.3?}",
        image.entry_count()
    );
}
