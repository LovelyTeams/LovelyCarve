use super::*;
use crate::core::catalog::filedb::{filedb_file::IndexMeta, filedb_query};
use crate::core::evidence::image::{ImageFormat, INDEX_ONLY_READ_ERROR};
use crate::core::filesystem::tree::fs_tree;

#[tokio::test]
async fn offline_case_publishes_stable_runtime_and_serves_indexed_pages_and_filters(
) -> anyhow::Result<()> {
    use crate::commands::filesystem::filesystem_snapshot_index::{
        case_index_targets, IndexCoverage,
    };
    use crate::core::casework::case::test_support::{case_with_evidence, publish_volume};
    use crate::core::catalog::{
        directory_query::{self, DirectoryQuery},
        snapshot::volume_snapshot::SnapshotEntry,
    };
    let directory = tempfile::tempdir()?;
    let mut case = case_with_evidence(directory.path(), "saved-image");
    let node = FsNode {
        id: "ntfs:0:42".into(),
        name: "saved.txt".into(),
        parent_id: Some("part:0".into()),
        kind: NodeKind::File,
        size: 12,
        deleted: false,
        inode: Some(42),
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    };
    publish_volume(
        &mut case,
        "saved-image",
        "part:0",
        "2026-10-11T00:00:00Z",
        &[SnapshotEntry {
            parent: "part:0".into(),
            node: node.clone(),
            path: "/saved.txt".into(),
        }],
    );
    let state = AppState::new();
    *state.active_case.lock().await = Some(case);
    let image = open_indexed_evidence(&state, "saved-image", "/evidence/saved-image.E01")
        .await
        .expect("restore indexed runtime")
        .ok_or_else(|| anyhow::anyhow!("expected offline runtime"))?;
    assert!(Arc::ptr_eq(
        &image,
        state
            .images
            .lock()
            .await
            .get("saved-image")
            .expect("published runtime")
    ));
    let targets = {
        let active = state.active_case.lock().await;
        case_index_targets(
            active.as_ref().expect("active case"),
            &image,
            "saved-image",
            IndexCoverage::Any,
        )
    };
    assert_eq!(targets.len(), 1);
    let mut query = DirectoryQuery::default();
    query.filter.extensions = vec!["txt".into()];
    let page = directory_query::indexed_page(&targets[0].index, "part:0", &query, None, 1)?
        .expect("indexed page");
    assert_eq!(
        (page.items[0].id.as_str(), page.total, page.next_cursor),
        (node.id.as_str(), Some(1), None)
    );
    query.filter.size_ge = Some(100);
    assert!(
        directory_query::indexed_page(&targets[0].index, "part:0", &query, None, 1)?
            .expect("filtered page")
            .items
            .is_empty()
    );
    assert!(
        open_indexed_evidence(&state, "saved-image", "/evidence/other.E01")
            .await
            .expect("resolve other evidence path")
            .is_none()
    );
    state.active_case.lock().await.take();
    assert!(
        open_indexed_evidence(&state, "saved-image", "/evidence/saved-image.E01")
            .await
            .expect("resolve a closed case")
            .is_none()
    );
    Ok(())
}

fn fixture(directory: &std::path::Path) -> anyhow::Result<(EvidenceRef, VolumeRecord, PathBuf)> {
    let path = directory.join("index.db");
    let record = VolumeRecord {
        image_id: "saved-image".into(),
        volume_key: "part:0".into(),
        vol_id: 0,
        source_size: 4096,
        fingerprint: "saved-fingerprint".into(),
        snapshot_file: "snapshot.dirz".into(),
        built_at: "2026-10-11T00:00:00Z".into(),
        entry_count: 1,
        truncated: false,
        index_file: Some("index.db".into()),
        index_built_at: Some("2026-10-11T00:00:00Z".into()),
    };
    let connection = filedb_file::create_build_connection(&path)?;
    filedb_file::write_meta(
        &connection,
        &IndexMeta {
            image_id: record.image_id.clone(),
            volume_key: record.volume_key.clone(),
            vol_id: record.vol_id,
            built_at: record.built_at.clone(),
            fingerprint: record.fingerprint.clone(),
            entry_count: record.entry_count,
            truncated: record.truncated,
            indexed_at: record.built_at.clone(),
        },
    )?;
    let node = FsNode {
        id: "ntfs:0:42".into(),
        name: "saved.txt".into(),
        parent_id: Some("part:0".into()),
        kind: NodeKind::File,
        size: 12,
        deleted: false,
        inode: Some(42),
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    };
    connection.execute("INSERT INTO filedb (ordinal,nid,pid,name,name_low,full_path,full_path_low,kind,extension,size,is_delete,meta_addr,meta_addr_high,node_json) VALUES (0,?1,'part:0','saved.txt','saved.txt','/saved.txt','/saved.txt','file','txt',12,0,42,0,?2)", rusqlite::params![node.id, serde_json::to_string(&node)?])?;
    connection.execute("INSERT INTO filedb_walked_dirs (pid) VALUES ('part:0')", [])?;
    connection.close().map_err(|(_, error)| error)?;
    let evidence = EvidenceRef {
        image_id: record.image_id.clone(),
        path: directory.join("missing.E01").display().to_string(),
        source_format: ImageFormat::Ewf,
        size_bytes: 4096,
        added_at: record.built_at.clone(),
        platform: None,
        manual_platform: None,
        evidence_type: None,
    };
    Ok((evidence, record, path))
}

#[test]
fn offline_indexes_restore_saved_identity_and_directory_without_source_bytes() -> anyhow::Result<()>
{
    let directory = tempfile::tempdir()?;
    let (evidence, record, index) = fixture(directory.path())?;
    let session = restore_indexed_session(evidence.clone(), &[(record, index.clone())])?
        .ok_or_else(|| anyhow::anyhow!("expected offline session"))?;
    let session = Arc::new(session);
    assert_eq!(
        (
            session.info().id,
            session.info().source_format,
            session.info().size_bytes,
            session.info().index_only,
            session.info().sector_size
        ),
        (evidence.image_id, ImageFormat::Ewf, 4096, true, 0)
    );
    assert_eq!(
        fs_tree::children_for_image(&session, "root")?
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["part:0"]
    );
    let indexed = filedb_query::children(&index, "part:0")?
        .ok_or_else(|| anyhow::anyhow!("expected indexed children"))?;
    assert_eq!(
        indexed
            .iter()
            .map(|node| (node.id.as_str(), node.name.as_str(), node.size))
            .collect::<Vec<_>>(),
        vec![("ntfs:0:42", "saved.txt", 12)]
    );
    assert!(fs_tree::children_for_image(&session, "unindexed-directory").is_err());
    for (offset, length) in [(0, 1), (4096, 0), (8192, 16)] {
        assert_eq!(
            session
                .read_range(offset, length)
                .expect_err("offline read must fail")
                .to_string(),
            INDEX_ONLY_READ_ERROR
        );
    }
    assert!(session.validate_source_file_stable().is_err());
    assert!(fs_tree::read_file_limited(&session, "ntfs:0:42", 12).is_err());
    assert!(fs_tree::stream_file_chunks(&session, "ntfs:0:42", 1024, |_| Ok(())).is_err());
    let existing = directory.path().join("existing.txt");
    std::fs::write(&existing, b"preserve existing content")?;
    assert!(fs_tree::export_file(&session, "ntfs:0:42", &existing).is_err());
    assert_eq!(std::fs::read(&existing)?, b"preserve existing content");
    let output_directory = directory.path().join("exported");
    let output_zip = directory.path().join("exported.zip");
    let cancel = tokio_util::sync::CancellationToken::new();
    assert!(fs_tree::export_dir(&session, "root", &output_directory, &cancel, &|_, _| {}).is_err());
    assert!(fs_tree::export_dir_zip(&session, "root", &output_zip, &cancel, &|_, _| {}).is_err());
    assert!(!output_directory.exists());
    assert!(!output_zip.exists());
    Ok(())
}

#[test]
fn offline_indexes_reject_stale_mismatched_missing_and_corrupt_publications() -> anyhow::Result<()>
{
    let directory = tempfile::tempdir()?;
    let (evidence, record, index) = fixture(directory.path())?;
    for changed in [
        VolumeRecord {
            image_id: "different".into(),
            ..record.clone()
        },
        VolumeRecord {
            fingerprint: "different".into(),
            ..record.clone()
        },
        VolumeRecord {
            index_built_at: None,
            ..record.clone()
        },
    ] {
        assert!(verified_index(&changed, &index).is_err());
        assert!(restore_indexed_session(evidence.clone(), &[(changed, index.clone())])?.is_none());
    }
    let missing = directory.path().join("absent.db");
    assert!(
        restore_indexed_session(evidence.clone(), &[(record.clone(), missing.clone())])?.is_none()
    );
    assert!(!missing.exists());
    let corrupt = directory.path().join("corrupt.db");
    std::fs::write(&corrupt, b"not a sqlite index")?;
    assert!(restore_indexed_session(evidence.clone(), &[(record, corrupt)])?.is_none());
    assert!(restore_indexed_session(evidence, &[])?.is_none());
    Ok(())
}
