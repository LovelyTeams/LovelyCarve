//! Persist/reopen regressions. Fixture containers deliberately have no Ext4
//! backing: any accidental recomposition (including siblings) fails the tests.
use super::*;
use crate::casework::case::test_support::case_with_evidence;
use crate::casework::case::CaseState;
use crate::catalog::filedb::filedb_query;
use crate::catalog::host_docker_index::{self, CaseTarget, PrepareMode, PreparedContainer};
use crate::filesystem::evidence_session::EvidenceSession;
use std::path::Path;

const BUILT_AT: &str = "2026-09-19T00:00:00.000Z";

fn image_with_containers(path: &Path, seed_mapping: bool) -> Arc<EvidenceSession> {
    let image =
        Arc::new(EvidenceSession::open_with_id("docker-fixture".into(), path).expect("image"));
    let mut source = FsSession::new(&image)
        .children("root")
        .expect("source nodes");
    if source
        .first()
        .is_some_and(|node| node.kind == NodeKind::Dir)
    {
        source = FsSession::new(&image)
            .children(&source[0].id)
            .expect("folder children");
    }
    assert_eq!(source.len(), 1);
    let tree: MergedContainerFs = serde_json::from_value(serde_json::json!({
        "entries": {"/hello.txt": {"source_node": source[0]}},
        "children": {"": ["/hello.txt"]}
    }))
    .expect("merged fixture");
    let merged = Arc::new(tree);
    let containers: Vec<_> = ["selected", "untouched"]
        .into_iter()
        .map(|id| {
            Arc::new(HostDockerContainer {
                id: id.into(),
                name: id.into(),
                image_reference: None,
                created: None,
                source: Ext4Source::Partition(0),
                docker_root_inode: 2,
                mount_id: "missing-layer".into(),
                init_id: None,
                lower_references: vec![],
                runtime_files: vec![],
                merged: parking_lot::Mutex::new(
                    (seed_mapping && id == "selected").then(|| Arc::clone(&merged)),
                ),
                index_lock: parking_lot::Mutex::new(()),
            })
        })
        .collect();
    let container_indexes = containers
        .iter()
        .enumerate()
        .map(|(index, container)| (container.id.clone(), index))
        .collect();
    image
        .host_docker
        .set(Some(Arc::new(HostDocker {
            containers,
            container_indexes,
        })))
        .expect("catalog");
    image
}

fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("fixture");
    std::fs::create_dir(directory.path().join("source")).expect("source");
    std::fs::write(
        directory.path().join("source/hello.txt"),
        b"persistent Docker preview",
    )
    .expect("file");
    directory
}

/// Prepares against the case and publishes whatever the build staged, the
/// way `fs_docker_prepare` does.
fn prepare_in(
    case: &mut CaseState,
    image: &Arc<EvidenceSession>,
    root: &str,
    mode: PrepareMode,
    built_at: &str,
) -> anyhow::Result<PreparedContainer> {
    let target = CaseTarget::of(case, &image.id, root);
    let mut prepared =
        host_docker_index::prepare(image, root, Some(&target), mode, built_at.to_string())?;
    if let Some(staged) = prepared.staged.take() {
        prepared.persisted = host_docker_index::publish(case, staged)?;
    }
    Ok(prepared)
}

#[test]
fn selected_container_persists_and_reopens_without_composing_siblings() {
    let fixture = fixture();
    let source = fixture.path().join("source");
    let case_dir = fixture.path().join("case");
    let image = image_with_containers(&source, true);
    let mut case = case_with_evidence(&case_dir, &image.id);
    let root = container_node_id("selected", "");
    let built =
        prepare_in(&mut case, &image, &root, PrepareMode::Build, BUILT_AT).expect("persist");
    assert!(built.ready && built.persisted);
    let record = case.volume(&image.id, &root).expect("published").clone();
    let index = case.index_path(&record).expect("indexed volume");
    let original = filedb_query::children(&index, &root)
        .expect("query")
        .expect("indexed");
    assert_eq!(original.len(), 1);
    assert_eq!(original[0].id, container_node_id("selected", "/hello.txt"));
    assert!(case.volume(&image.id, "host-docker:untouched:").is_none());
    drop(image);

    let reopened = image_with_containers(&source, false);
    *reopened.host_docker_cache_dir.write() =
        Some(host_docker_index::cache_directory(&case_dir, &reopened.id));
    // File reads restore their own mapping, even before visiting the list.
    let id = parse_host_docker_id(&original[0].id).expect("id");
    assert_eq!(
        read_file(&reopened, &id, 1024).expect("cold preview"),
        b"persistent Docker preview"
    );
    let reused =
        prepare_in(&mut case, &reopened, &root, PrepareMode::Status, "unused").expect("reuse");
    assert!(reused.ready && reused.persisted);
    assert_eq!(reused.entry_count, 1);
    assert_eq!(
        case.volume(&reopened.id, &root).expect("published"),
        &record,
        "a status probe republishes nothing"
    );
    assert!(catalog(&reopened)
        .expect("catalog")
        .container("untouched")
        .expect("sibling")
        .merged
        .lock()
        .is_none());
}

#[test]
fn failed_snapshot_write_is_not_ready_and_can_retry() {
    let fixture = fixture();
    let case_dir = fixture.path().join("case");
    let image = image_with_containers(&fixture.path().join("source"), true);
    let mut case = case_with_evidence(&case_dir, &image.id);
    let staging = case.staging_dir();
    std::fs::remove_dir(&staging).expect("empty staging");
    std::fs::write(&staging, b"blocked directory").expect("block staging");
    let root = container_node_id("selected", "");
    assert!(prepare_in(
        &mut case,
        &image,
        &root,
        PrepareMode::Build,
        "2026-09-19T00:00:01.000Z"
    )
    .is_err());
    let pending =
        prepare_in(&mut case, &image, &root, PrepareMode::Status, "probe").expect("status");
    assert!(!pending.ready && !pending.persisted);
    assert!(case.volume(&image.id, &root).is_none());
    std::fs::remove_file(&staging).expect("unblock");
    let completed = prepare_in(
        &mut case,
        &image,
        &root,
        PrepareMode::Build,
        "2026-09-19T00:00:02.000Z",
    )
    .expect("retry");
    assert!(completed.ready && completed.persisted);
    assert_eq!(
        std::fs::read_dir(&staging).expect("staging").count(),
        0,
        "published files leave staging"
    );
}

#[test]
fn corrupt_mapping_or_missing_snapshot_requires_rebuild() {
    let fixture = fixture();
    let source = fixture.path().join("source");
    let case_dir = fixture.path().join("case");
    let image = image_with_containers(&source, true);
    let mut case = case_with_evidence(&case_dir, &image.id);
    let root = container_node_id("selected", "");
    prepare_in(&mut case, &image, &root, PrepareMode::Build, BUILT_AT).expect("build");
    let mapping_dir = host_docker_index::cache_directory(&case_dir, &image.id);
    let mapping = std::fs::read_dir(mapping_dir)
        .expect("mapping directory")
        .next()
        .expect("file")
        .expect("entry")
        .path();
    std::fs::write(mapping, b"broken zstd").expect("corrupt");
    let reopened = image_with_containers(&source, false);
    assert!(
        !prepare_in(&mut case, &reopened, &root, PrepareMode::Status, "probe")
            .expect("status")
            .ready
    );
    let record = case.volume(&image.id, &root).expect("published").clone();
    std::fs::remove_file(case.snapshot_path(&record)).expect("remove snapshot");
    assert!(
        !prepare_in(&mut case, &image, &root, PrepareMode::Status, "probe")
            .expect("status")
            .ready
    );
}

#[test]
fn missing_or_stale_index_requires_rebuild() {
    let fixture = fixture();
    let case_dir = fixture.path().join("case");
    let image = image_with_containers(&fixture.path().join("source"), true);
    let mut case = case_with_evidence(&case_dir, &image.id);
    let root = container_node_id("selected", "");
    prepare_in(&mut case, &image, &root, PrepareMode::Build, BUILT_AT).expect("build");
    let record = case.volume(&image.id, &root).expect("published").clone();
    std::fs::remove_file(case.index_path(&record).expect("index")).expect("remove index");
    assert!(
        !prepare_in(&mut case, &image, &root, PrepareMode::Status, "probe")
            .expect("status")
            .ready
    );
    let rebuilt = prepare_in(
        &mut case,
        &image,
        &root,
        PrepareMode::Build,
        "2026-09-19T00:00:01.000Z",
    )
    .expect("rebuild");
    assert!(rebuilt.ready && rebuilt.persisted);
    let current = case.volume(&image.id, &root).expect("republished");
    assert!(current.index_is_current());
    assert!(case
        .index_path(current)
        .is_some_and(|index_path| index_path.is_file()));
}
