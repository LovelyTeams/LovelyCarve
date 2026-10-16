//! Synthetic recovery fixtures never contain real evidence.
use super::*;
use std::{fs, io::Write};

fn options(existing: bool) -> RecoveryOptions {
    RecoveryOptions {
        fat: true,
        ntfs: true,
        exfat: true,
        include_existing: existing,
    }
}
fn whole(image: &EvidenceSession) -> Vec<SectorRange> {
    vec![SectorRange {
        start_sector: 0,
        sector_count: image.len() / image.sector_size as u64,
    }]
}
fn scan_whole(
    image: &Arc<EvidenceSession>,
    options: RecoveryOptions,
    cancel: &CancellationToken,
) -> Result<RecoveryScan> {
    scan(
        image,
        "scan".to_string(),
        options,
        whole(image),
        cancel,
        &mut |_, _, _| {},
    )
}
/// Recovered files below an export directory as (`/`-joined relative path, absolute path).
pub(crate) fn exported_files(directory: &str) -> Vec<(String, std::path::PathBuf)> {
    let root = std::path::Path::new(directory);
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .is_some_and(|name| name != "recovery-report.jsonl")
            {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .iter()
                    .map(|component| component.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                files.push((relative, path));
            }
        }
    }
    files.sort();
    files
}
fn fixture(prefix: usize) -> tempfile::NamedTempFile {
    write_fixture(&fixture_bytes(prefix))
}
fn write_fixture(data: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(data).unwrap();
    file
}
/// The FAT12 fixture plus a live `SUB` folder (cluster 3) holding a deleted
/// `INNER.TXT` (cluster 4, "inner").
fn nested_fixture() -> tempfile::NamedTempFile {
    let mut data = fixture_bytes(0);
    // Cluster 2 stays free for the deleted root file; cluster 3 ends the folder chain.
    for table in [512, 10 * 512] {
        data[table + 3..table + 6].copy_from_slice(&[0x00, 0xf0, 0xff]);
    }
    let entry = 19 * 512 + 32;
    data[entry..entry + 11].copy_from_slice(b"SUB        ");
    data[entry + 11] = 0x10;
    data[entry + 26..entry + 28].copy_from_slice(&3u16.to_le_bytes());
    let folder = 34 * 512;
    data[folder..folder + 11].copy_from_slice(b".          ");
    data[folder + 11] = 0x10;
    data[folder + 26..folder + 28].copy_from_slice(&3u16.to_le_bytes());
    data[folder + 32..folder + 43].copy_from_slice(b"..         ");
    data[folder + 43] = 0x10;
    let inner = folder + 64;
    data[inner..inner + 11].copy_from_slice(b"\xe5NNER   TXT");
    data[inner + 11] = 0x20;
    data[inner + 26..inner + 28].copy_from_slice(&4u16.to_le_bytes());
    data[inner + 28..inner + 32].copy_from_slice(&5u32.to_le_bytes());
    data[35 * 512..35 * 512 + 5].copy_from_slice(b"inner");
    write_fixture(&data)
}
fn fixture_bytes(prefix: usize) -> Vec<u8> {
    let mut data = vec![0u8; prefix + 2880 * 512];
    let boot = &mut data[prefix..prefix + 512];
    boot[0..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    boot[3..11].copy_from_slice(b"MSDOS5.0");
    boot[11..13].copy_from_slice(&512u16.to_le_bytes());
    boot[13] = 1;
    boot[14..16].copy_from_slice(&1u16.to_le_bytes());
    boot[16] = 2;
    boot[17..19].copy_from_slice(&224u16.to_le_bytes());
    boot[19..21].copy_from_slice(&2880u16.to_le_bytes());
    boot[21] = 0xf0;
    boot[22..24].copy_from_slice(&9u16.to_le_bytes());
    boot[38] = 0x29;
    boot[43..54].copy_from_slice(b"RECOVERY   ");
    boot[54..62].copy_from_slice(b"FAT12   ");
    boot[510..512].copy_from_slice(&[0x55, 0xaa]);
    for start in [prefix + 512, prefix + 10 * 512] {
        data[start..start + 3].copy_from_slice(&[0xf0, 0xff, 0xff]);
    }
    let entry = prefix + 19 * 512;
    data[entry..entry + 11].copy_from_slice(b"\xe5ELETED TXT");
    data[entry + 11] = 0x20;
    data[entry + 26..entry + 28].copy_from_slice(&2u16.to_le_bytes());
    data[entry + 28..entry + 32].copy_from_slice(&5u32.to_le_bytes());
    data[prefix + 33 * 512..prefix + 33 * 512 + 5].copy_from_slice(b"hello");
    data
}
#[test]
fn recovery_deleted_fat_and_lost_volume_export_have_identical_bytes() {
    for prefix in [0, 8 * 1024 * 1024] {
        let source = fixture(prefix);
        let before = fs::read(source.path()).unwrap();
        let image = Arc::new(EvidenceSession::open(source.path()).unwrap());
        let cancel = CancellationToken::new();
        let scan = scan_whole(&image, options(false), &cancel).unwrap();
        assert!(!scan.truncated);
        assert_eq!(scan.candidates.len(), 1, "{scan:#?}");
        let candidate = &scan.candidates[0];
        assert!(candidate.node.deleted);
        assert!(candidate.readable);
        assert_eq!(scan.volumes[0].partition.start_lba, prefix as u64 / 512);
        assert_eq!(scan.volumes[0].lost, prefix != 0);
        let destination = tempfile::tempdir().unwrap();
        let result = export::recover(
            &image,
            &scan,
            &[candidate.id.clone()],
            destination.path(),
            &cancel,
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(result.recovered, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.bytes_written, 5);
        let outputs = exported_files(&result.directory);
        let name = crate::core::filesystem::tree::fs_tree_export::sanitize_export_component(
            &candidate.node.name,
        );
        let folder = scan.volumes[0]
            .partition
            .label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or("FAT12")
            .to_string();
        assert_eq!(
            outputs
                .iter()
                .map(|(relative, _)| relative.as_str())
                .collect::<Vec<_>>(),
            vec![format!("v0-{folder}/{name}")]
        );
        assert_eq!(fs::read(&outputs[0].1).unwrap(), b"hello");
        let report = fs::read_to_string(&result.report_path).unwrap();
        let line =
            serde_json::from_str::<serde_json::Value>(report.lines().nth(1).unwrap()).unwrap();
        assert_eq!(line["relative_output"], outputs[0].0.as_str());
        assert_eq!(fs::read(source.path()).unwrap(), before);
        let mut invalid = scan.clone();
        invalid.candidates[0].node.size = 6;
        let failed = export::recover(
            &image,
            &invalid,
            &[candidate.id.clone()],
            destination.path(),
            &cancel,
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(failed.failed, 1);
        assert_eq!(
            fs::read_dir(failed.directory).unwrap().count(),
            1,
            "Only the report may remain after a short output, without empty folders"
        );
        assert_eq!(fs::read(&outputs[0].1).unwrap(), b"hello");
    }
}
#[test]
fn recovery_range_and_cancellation_are_enforced() {
    let source = fixture(0);
    let image = Arc::new(EvidenceSession::open(source.path()).unwrap());
    let token = CancellationToken::new();
    let total = image.len() / image.sector_size as u64;
    for ranges in [
        vec![],
        vec![SectorRange {
            start_sector: 0,
            sector_count: u64::MAX,
        }],
        vec![SectorRange {
            start_sector: 1,
            sector_count: total,
        }],
        vec![SectorRange {
            start_sector: 0,
            sector_count: 0,
        }],
    ] {
        assert!(
            scan(
                &image,
                "scan".to_string(),
                options(false),
                ranges.clone(),
                &token,
                &mut |_, _, _| {}
            )
            .is_err(),
            "{ranges:?}"
        );
    }
    assert!(scan_whole(&image, RecoveryOptions::default(), &token).is_err());
    // A range beginning after the boot sector cannot report that volume.
    let tail = scan(
        &image,
        "scan".to_string(),
        options(false),
        vec![SectorRange {
            start_sector: 1,
            sector_count: total - 1,
        }],
        &token,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert!(tail.volumes.is_empty(), "{tail:#?}");
    token.cancel();
    assert!(scan_whole(&image, options(false), &token).is_err());
}
#[test]
fn recovery_external_image_smoke() {
    let Some(path) = std::env::var_os("RECOVERY_TEST_IMAGE") else {
        return;
    };
    let image = Arc::new(EvidenceSession::open(path).unwrap());
    let token = CancellationToken::new();
    let result = scan_whole(&image, options(true), &token).unwrap();
    assert!(!result.truncated);
    assert!(
        result.volumes.iter().all(|volume| volume.error.is_none()),
        "{result:#?}"
    );
    assert!(!result.candidates.is_empty());
    let destination = tempfile::tempdir().unwrap();
    let ids = result
        .candidates
        .iter()
        .filter(|candidate| candidate.readable)
        .map(|candidate| candidate.id.clone())
        .collect::<Vec<_>>();
    let recovered = export::recover(
        &image,
        &result,
        &ids,
        destination.path(),
        &token,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(recovered.failed, 0, "{recovered:#?}");
    eprintln!(
        "recovery smoke: volumes={}, files={}, deleted={}, exported={}, bytes={}",
        result.volumes.len(),
        result.candidates.len(),
        result
            .candidates
            .iter()
            .filter(|item| item.node.deleted)
            .count(),
        recovered.recovered,
        recovered.bytes_written
    );
}
#[test]
fn recovery_ntfs_resident_nonresident_and_sparse_streams_match() {
    let image = Arc::new(
        EvidenceSession::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/vendor/ntfs/testdata/testfs1"
        ))
        .unwrap(),
    );
    let token = CancellationToken::new();
    let result = scan_whole(&image, options(true), &token).unwrap();
    assert!(!result.truncated);
    assert!(
        result.volumes.iter().all(|volume| volume.error.is_none()),
        "{result:#?}"
    );
    let destination = tempfile::tempdir().unwrap();
    for (name, expected) in [
        ("file-with-12345", b"12345".to_vec()),
        ("1000-bytes-file", b"12345".repeat(200)),
        (
            "sparse-file",
            [b"12345".to_vec(), vec![0; 499995], b"11111".to_vec()].concat(),
        ),
    ] {
        let candidate = result
            .candidates
            .iter()
            .find(|candidate| candidate.node.name == name)
            .unwrap();
        let exported = export::recover(
            &image,
            &result,
            &[candidate.id.clone()],
            destination.path(),
            &token,
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(exported.failed, 0, "{exported:#?}");
        let outputs = exported_files(&exported.directory);
        assert_eq!(outputs.len(), 1, "{outputs:?}");
        assert!(outputs[0].0.ends_with(&format!("/{name}")), "{outputs:?}");
        assert_eq!(fs::read(&outputs[0].1).unwrap(), expected, "{name}");
    }
}
#[test]
fn recovery_export_mirrors_recovered_folders() {
    use crate::core::filesystem::tree::fs_tree_export::sanitize_export_component;
    let source = nested_fixture();
    let image = Arc::new(EvidenceSession::open(source.path()).unwrap());
    let token = CancellationToken::new();
    let result = scan_whole(&image, options(false), &token).unwrap();
    let ids = result
        .candidates
        .iter()
        .map(|candidate| candidate.id.clone())
        .collect::<Vec<_>>();
    let paths = result
        .candidates
        .iter()
        .map(|candidate| candidate.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 2, "{result:#?}");
    assert!(
        paths.iter().any(|path| path.starts_with("/SUB/")),
        "{paths:?}"
    );
    let destination = tempfile::tempdir().unwrap();
    let exported = export::recover(
        &image,
        &result,
        &ids,
        destination.path(),
        &token,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(exported.failed, 0, "{exported:#?}");
    let report = fs::read_to_string(&exported.report_path).unwrap();
    let mut seen = 0;
    for line in report.lines().skip(1) {
        let entry = serde_json::from_str::<serde_json::Value>(line).unwrap();
        let path = entry["candidate"]["path"].as_str().unwrap();
        let relative = entry["relative_output"].as_str().unwrap();
        let (volume_folder, rest) = relative.split_once('/').unwrap();
        assert!(volume_folder.starts_with("v0-"), "{relative}");
        let mirrored = path
            .split('/')
            .filter(|component| !component.is_empty())
            .map(sanitize_export_component)
            .collect::<Vec<_>>()
            .join("/");
        assert_eq!(rest, mirrored, "{path}");
        let written = std::path::Path::new(&exported.directory).join(relative);
        assert_eq!(
            fs::metadata(written).unwrap().len(),
            entry["candidate"]["node"]["size"].as_u64().unwrap()
        );
        seen += 1;
    }
    assert_eq!(seen, ids.len());
    let files = exported_files(&exported.directory);
    assert_eq!(
        files
            .iter()
            .map(|(relative, path)| (relative.matches('/').count(), fs::read(path).unwrap()))
            .collect::<Vec<_>>(),
        vec![(2, b"inner".to_vec()), (1, b"hello".to_vec())],
        "{files:?}"
    );
}
#[test]
fn recovery_ntfs_without_boot_reconstructs_geometry_and_keeps_source_bytes() {
    use crate::core::filesystem::volumes::ntfs::discover_mft;
    let mut bytes = fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/vendor/ntfs/testdata/testfs1"
    ))
    .unwrap();
    let sector = u16::from_le_bytes(bytes[11..13].try_into().unwrap()) as u64;
    let cluster = sector * u64::from(bytes[13]);
    let mft = u64::from_le_bytes(bytes[48..56].try_into().unwrap()) * cluster;
    bytes[..512].fill(0);
    let mut source = tempfile::NamedTempFile::new().unwrap();
    source.write_all(&bytes).unwrap();
    source.flush().unwrap();
    let image = Arc::new(EvidenceSession::open(source.path()).unwrap());
    let (partition, geometry) = discover_mft(&image, mft).unwrap();
    assert_eq!(geometry.cluster_size as u64, cluster);
    assert_eq!(partition.start_lba, 0);
    let token = CancellationToken::new();
    let scanned = scan_whole(&image, options(false), &token).unwrap();
    let index = scanned
        .volumes
        .iter()
        .position(|volume| volume.reconstruction.is_some())
        .unwrap();
    let file = scanned
        .candidates
        .iter()
        .find(|candidate| {
            candidate.volume_index == index && candidate.node.name == "file-with-12345"
        })
        .unwrap();
    assert_eq!(file.node.size, 5);
    let destination = tempfile::tempdir().unwrap();
    let exported = export::recover(
        &image,
        &scanned,
        &[file.id.clone()],
        destination.path(),
        &token,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(exported.recovered, 1);
    assert_eq!(exported.bytes_written, 5);
    let volume = crate::core::filesystem::volumes::ntfs::NtfsVolume::open_recovered(
        &image, &partition, &geometry,
    )
    .unwrap();
    let boot = volume.read_file(7, 512).unwrap();
    assert_eq!(
        boot,
        vec![0; 512],
        "Inferred geometry must not replace file content"
    );
    assert_eq!(fs::read(source.path()).unwrap(), bytes);
    let mut corrupted = bytes;
    corrupted[mft as usize + 510] ^= 1;
    let mut broken = tempfile::NamedTempFile::new().unwrap();
    broken.write_all(&corrupted).unwrap();
    broken.flush().unwrap();
    let image = EvidenceSession::open(broken.path()).unwrap();
    assert!(discover_mft(&image, mft).is_err());
}

#[test]
fn recovery_external_deleted_and_lost_candidates_are_exportable() {
    let Some(path) = std::env::var_os("RECOVERY_TEST_IMAGE") else {
        return;
    };
    let image = Arc::new(EvidenceSession::open(path).unwrap());
    let token = CancellationToken::new();
    let scanned = scan_whole(&image, options(false), &token).unwrap();
    let readable = scanned
        .candidates
        .iter()
        .filter(|entry| entry.readable)
        .collect::<Vec<_>>();
    assert!(!readable.is_empty());
    let destination = tempfile::tempdir().unwrap();
    let ids = readable
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    let exported = export::recover(
        &image,
        &scanned,
        &ids,
        destination.path(),
        &token,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(exported.failed, 0, "{exported:#?}");
    for (index, volume) in scanned.volumes.iter().enumerate() {
        let files = scanned
            .candidates
            .iter()
            .filter(|entry| entry.volume_index == index)
            .collect::<Vec<_>>();
        eprintln!(
            "recovered volume {:?} {:?}: files={}, dirs={}, bytes={}, rebuilt={}",
            volume.partition.kind,
            volume.partition.label,
            files.len(),
            scanned
                .directories
                .iter()
                .filter(|entry| entry.volume_index == index)
                .count(),
            files.iter().map(|entry| entry.node.size).sum::<u64>(),
            volume.reconstruction.is_some()
        );
    }
    eprintln!(
        "deleted and lost volume recovery: files={}, exported={}, bytes={}",
        readable.len(),
        exported.recovered,
        exported.bytes_written
    );
    if let Some(path) = std::env::var_os("RECOVERY_SCAN_REPORT") {
        fs::write(path, serde_json::to_vec(&scanned).unwrap()).unwrap();
    }
    let mut only_fat = options(true);
    only_fat.ntfs = false;
    only_fat.exfat = false;
    let filtered = scan_whole(&image, only_fat, &token).unwrap();
    assert!(filtered.volumes.iter().all(|volume| matches!(
        volume.partition.kind,
        PartitionKind::Fat12 | PartitionKind::Fat16 | PartitionKind::Fat32
    )));
}
#[test]
fn recovery_u_image_matches_original_volume_and_recycled_names() {
    let Some(path) = std::env::var_os("RECOVERY_U_IMAGE") else {
        return;
    };
    let image = Arc::new(EvidenceSession::open(path).unwrap());
    let token = CancellationToken::new();
    let scanned = scan_whole(&image, options(true), &token).unwrap();
    let index = scanned
        .volumes
        .iter()
        .position(|volume| volume.reconstruction.is_some())
        .unwrap();
    assert_eq!(
        scanned.volumes[index].partition.label.as_deref(),
        Some("新加卷")
    );
    assert_eq!(scanned.volumes[index].partition.start_lba, 32768);
    let files = scanned
        .candidates
        .iter()
        .filter(|entry| entry.volume_index == index)
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 89);
    assert_eq!(
        files.iter().map(|entry| entry.node.size).sum::<u64>(),
        871331343
    );
    let full = files
        .iter()
        .find(|entry| entry.path == "/full.bin")
        .unwrap();
    assert_eq!(full.node.size, 100 * 1024 * 1024);
    assert!(full
        .source_path
        .as_ref()
        .unwrap()
        .contains("/$RECYCLE.BIN/"));
    let temporary = scanned
        .directories
        .iter()
        .filter(|entry| entry.volume_index == index && entry.path == "/temp")
        .collect::<Vec<_>>();
    assert_eq!(temporary.len(), 2);
    assert_ne!(temporary[0].node.id, temporary[1].node.id);
    assert_eq!(
        files
            .iter()
            .filter(|entry| entry.node.parent_id.as_deref() == Some("part:0"))
            .count(),
        8
    );
    assert_eq!(
        scanned
            .directories
            .iter()
            .filter(|entry| entry.volume_index == index
                && entry.node.parent_id.as_deref() == Some("part:0"))
            .count(),
        6
    );
    let destination = tempfile::tempdir().unwrap();
    let exported = export::recover(
        &image,
        &scanned,
        &[full.id.clone()],
        destination.path(),
        &token,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(exported.recovered, 1);
    assert_eq!(exported.bytes_written, 104857600);
    eprintln!("U.E01 oracle: NTFS 新加卷, 89 files, 830.966 MiB, root 14 items, full.bin exported, temp identities preserved");
}
