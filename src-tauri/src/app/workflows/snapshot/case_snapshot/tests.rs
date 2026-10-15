use super::*;
use crate::core::evidence::image::DiskImage;
use crate::core::filesystem::tree::fs_tree::{self, NodeKind};
use std::fs::File;
use std::io::Write;

fn backup_view_fixture(format: ImageFormat) -> (tempfile::TempDir, DiskImage) {
    let directory = tempfile::tempdir().unwrap();
    let backup = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/android-backup/abe-v1.ab"),
    )
    .unwrap();
    let path = match format {
        ImageFormat::Zip => {
            let path = directory.path().join("phone.zip");
            let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            writer
                .start_file("QQ(com.tencent.mobileqq).BAK", options)
                .unwrap();
            writer.write_all(&backup).unwrap();
            writer.start_file("ordinary.bak", options).unwrap();
            writer.write_all(b"opaque database backup").unwrap();
            writer
                .add_directory("real-directory.bak/", options)
                .unwrap();
            writer.finish().unwrap();
            path
        }
        ImageFormat::Folder => {
            let path = directory.path().join("phone");
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("QQ(com.tencent.mobileqq).BAK"), backup).unwrap();
            std::fs::write(path.join("ordinary.bak"), b"opaque database backup").unwrap();
            std::fs::create_dir(path.join("real-directory.bak")).unwrap();
            path
        }
        _ => panic!("backup fixture requires ZIP or folder"),
    };
    let image = DiskImage::open_with_id("phone".into(), path).unwrap();
    (directory, image)
}

#[test]
fn backup_directories_hide_suffixes_keep_ids_and_can_be_entered() {
    for format in [ImageFormat::Zip, ImageFormat::Folder] {
        let (_directory, image) = backup_view_fixture(format);
        let image = Arc::new(EvidenceSession::from(image));
        let root = fs_tree::children_for_image(&image, "root")
            .unwrap()
            .remove(0);
        let nodes = fs_tree::children_for_image(&image, &root.id).unwrap();
        let backup = nodes
            .iter()
            .find(|node| node.name == "QQ(com.tencent.mobileqq)")
            .unwrap();
        assert_eq!(backup.kind, NodeKind::Dir);
        assert!(backup
            .id
            .ends_with(&hex::encode("QQ(com.tencent.mobileqq).BAK")));
        assert!(nodes
            .iter()
            .any(|node| node.name == "ordinary.bak" && node.kind == NodeKind::File));
        assert!(nodes
            .iter()
            .any(|node| node.name == "real-directory.bak" && node.kind == NodeKind::Dir));
        let apps = fs_tree::children_for_image(&image, &backup.id)
            .unwrap()
            .remove(0);
        assert_eq!((apps.name.as_str(), apps.kind), ("apps", NodeKind::Dir));
        let package = fs_tree::children_for_image(&image, &apps.id)
            .unwrap()
            .remove(0);
        let database = fs_tree::children_for_image(&image, &package.id)
            .unwrap()
            .remove(0);
        let file = fs_tree::children_for_image(&image, &database.id)
            .unwrap()
            .remove(0);
        assert_eq!(fs_tree::read_file(&image, &file.id).unwrap(), b"payload");
    }
}

#[test]
fn old_file_only_backup_index_is_retired_before_browsing_and_rebuilt_as_directories() {
    use crate::commands::filesystem::filesystem_snapshot_index::{
        case_index_targets, IndexCoverage,
    };
    use crate::core::casework::case::test_support::case_with_evidence;
    use crate::core::catalog::filedb::{
        filedb_query,
        filedb_store::{self, IndexSource},
    };
    use volume_snapshot::{
        SnapshotEntry, SnapshotHeader, VolumeSnapshot, VolumeSnapshotBuilt, SNAPSHOT_SCHEMA,
    };

    let (_source, mut disk) = backup_view_fixture(ImageFormat::Zip);
    // Reproduce the pre-expansion fingerprint without changing source identity.
    let projection = disk.zip.take();
    let legacy_fingerprint = disk.evidence_fingerprint();
    disk.zip = projection;
    let fingerprint = disk.evidence_fingerprint();
    assert_ne!(fingerprint, legacy_fingerprint);
    assert_eq!(
        DiskImage::open_with_id("other-runtime-id".into(), &disk.path)
            .unwrap()
            .evidence_fingerprint(),
        fingerprint
    );
    let image = Arc::new(EvidenceSession::from(disk));
    let case_directory = tempfile::tempdir().unwrap();
    let mut case = case_with_evidence(case_directory.path(), &image.id);
    let node = fs_tree::children_for_image(&image, "zip:")
        .unwrap()
        .into_iter()
        .find(|node| node.name == "QQ(com.tencent.mobileqq)")
        .unwrap();
    let mut old_node = node.clone();
    old_node.kind = NodeKind::File;
    old_node.name.push_str(".BAK");
    let entries = [
        SnapshotEntry {
            parent: "root".into(),
            node: fs_tree::children_for_image(&image, "root")
                .unwrap()
                .remove(0),
            path: "/phone.zip".into(),
        },
        SnapshotEntry {
            parent: "zip:".into(),
            node: old_node,
            path: "/phone.zip/QQ(com.tencent.mobileqq).BAK".into(),
        },
    ];
    let built_at = "2026-01-01T00:00:00.000Z";
    let header = SnapshotHeader {
        schema_version: SNAPSHOT_SCHEMA,
        volume_key: "root".into(),
        image_id: image.id.clone(),
        source_volume_size: image.len(),
        built_at: built_at.into(),
    };
    let staged = case.staging_dir().join("old.dirz");
    let mut encoder = zstd::stream::write::Encoder::new(File::create(&staged).unwrap(), 1).unwrap();
    volume_snapshot::write_snapshot_header(&mut encoder, &header).unwrap();
    for entry in &entries {
        volume_snapshot::write_snapshot_entry(&mut encoder, entry, &mut Vec::new()).unwrap();
    }
    encoder.finish().unwrap();
    let old = VolumeSnapshotBuilt {
        header: header.clone(),
        file_name: String::new(),
        entry_count: entries.len() as u64,
        truncated: false,
    };
    case.publish_snapshot(
        &staged,
        case::snapshot_publication(0, &old, &legacy_fingerprint),
    )
    .unwrap();
    let old_index = case.staging_dir().join("old.db");
    filedb_store::build_index_from_entries(
        &entries,
        &old_index,
        &IndexSource {
            image_id: &image.id,
            volume_key: "root",
            vol_id: 0,
            built_at,
            fingerprint: &legacy_fingerprint,
            truncated: false,
        },
    )
    .unwrap();
    assert!(case
        .publish_index(&image.id, "root", built_at, &old_index)
        .unwrap());
    case.install_snapshot(Arc::new(VolumeSnapshot::new(header, entries.to_vec())));
    let old_targets = case_index_targets(&case, &image, &image.id, IndexCoverage::Any);
    assert_eq!(old_targets.len(), 1);
    assert_eq!(
        filedb_query::children(&old_targets[0].index, "zip:")
            .unwrap()
            .unwrap()[0]
            .kind,
        NodeKind::File
    );

    retire_outdated_backup_views(&mut case, &image).unwrap();
    assert!(case_index_targets(&case, &image, &image.id, IndexCoverage::Any).is_empty());
    assert!(case.covering_snapshot(&image.id, "zip:").is_none());
    let live = fs_tree::children_for_image(&image, "zip:")
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == node.id)
        .unwrap();
    assert_eq!(
        serde_json::to_value(live).unwrap(),
        serde_json::to_value(&node).unwrap()
    );

    let staged = case.staging_dir().join("rebuilt.dirz");
    let built = volume_snapshot::build_volume_snapshot(
        &image,
        "root",
        &staged,
        100,
        "2026-10-15T00:00:00.000Z".into(),
        &CancellationToken::new(),
        &mut |_| {},
    )
    .unwrap();
    let snapshot = volume_snapshot::load_snapshot(&staged).unwrap();
    assert!(snapshot
        .children("zip:")
        .unwrap()
        .iter()
        .any(|entry| entry.id == node.id && entry.name == node.name && entry.kind == node.kind));
    let staged_index = case.staging_dir().join("rebuilt.db");
    filedb_store::build_index(
        &staged,
        &staged_index,
        &IndexSource {
            image_id: &image.id,
            volume_key: "root",
            vol_id: 0,
            built_at: &built.header.built_at,
            fingerprint: &fingerprint,
            truncated: false,
        },
        None,
        None,
    )
    .unwrap();
    case.publish_snapshot(&staged, case::snapshot_publication(0, &built, &fingerprint))
        .unwrap();
    assert!(case
        .publish_index(&image.id, "root", &built.header.built_at, &staged_index)
        .unwrap());
    retire_outdated_backup_views(&mut case, &image).unwrap();
    let targets = case_index_targets(&case, &image, &image.id, IndexCoverage::Any);
    assert_eq!(targets.len(), 1);
    assert!(filedb_query::children(&targets[0].index, "zip:")
        .unwrap()
        .unwrap()
        .iter()
        .any(|entry| entry.id == node.id && entry.name == node.name && entry.kind == node.kind));
}

#[test]
fn lvm_partition_is_auto_snapshot_supported() {
    assert!(snapshot_supported_plain_partition(PartitionKind::Lvm));
}

#[test]
fn non_browsable_partition_kinds_are_not_auto_snapshotted() {
    for kind in [
        PartitionKind::Apm,
        PartitionKind::BiosBoot,
        PartitionKind::Bsd,
        PartitionKind::Extended,
        PartitionKind::Luks,
        PartitionKind::Mdadm,
        PartitionKind::Swap,
        PartitionKind::Unknown,
    ] {
        assert!(!snapshot_supported_plain_partition(kind), "{kind:?}");
    }
}
