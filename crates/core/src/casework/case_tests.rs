use super::*;
use crate::casework::case_journal;
use crate::catalog::snapshot::volume_snapshot::{
    SnapshotHeader, VolumeSnapshotBuilt, SNAPSHOT_SCHEMA,
};
use crate::evidence::image::ImageFormat;

fn options(name: &str) -> CreateCaseOptions {
    CreateCaseOptions {
        name: name.to_string(),
        ..CreateCaseOptions::default()
    }
}

fn evidence(image_id: &str, path: &str) -> EvidenceRef {
    EvidenceRef {
        image_id: image_id.to_string(),
        path: path.to_string(),
        source_format: ImageFormat::Ewf,
        size_bytes: 1,
        added_at: "2026-10-09T00:00:00.000Z".to_string(),
        platform: None,
        manual_platform: None,
        evidence_type: None,
    }
}

fn built(image_id: &str, volume_key: &str, built_at: &str) -> VolumeSnapshotBuilt {
    VolumeSnapshotBuilt {
        header: SnapshotHeader {
            schema_version: SNAPSHOT_SCHEMA,
            volume_key: volume_key.to_string(),
            image_id: image_id.to_string(),
            source_volume_size: 4096,
            built_at: built_at.to_string(),
        },
        file_name: "staged.dirz".to_string(),
        entry_count: 3,
        truncated: false,
    }
}

/// Stages a fake snapshot / index file and publishes both.
fn publish_volume(case: &mut CaseState, image_id: &str, volume_key: &str, built_at: &str) {
    let staged = case
        .staging_dir()
        .join(format!("{image_id}-{volume_key}.dirz").replace(':', "_"));
    std::fs::write(&staged, b"dirz").expect("stage snapshot");
    let publication = snapshot_publication(0, &built(image_id, volume_key, built_at), "fp");
    assert!(matches!(
        case.publish_snapshot(&staged, publication)
            .expect("publish snapshot"),
        SnapshotPublish::Published(_)
    ));
    let staged_index = case.staging_dir().join("index.db");
    std::fs::write(&staged_index, b"index").expect("stage index");
    assert!(case
        .publish_index(image_id, volume_key, built_at, &staged_index)
        .expect("publish index"));
}

fn journal_actions(case: &CaseState) -> Vec<String> {
    let mut actions: Vec<String> = case_journal::recent(&case.ledger, 100)
        .expect("journal")
        .into_iter()
        .map(|entry| entry.action)
        .collect();
    actions.reverse();
    actions
}

#[test]
fn create_then_open_round_trips_through_the_ledger() {
    let dir = tempfile::tempdir().expect("tempdir");
    let case = create_case(dir.path(), options("Operation Test")).expect("create");
    assert!(case.profile().case_number.starts_with("LC-"));
    assert!(dir.path().join("case.db").is_file());
    assert!(dir.path().join("evidence").is_dir());
    assert!(dir.path().join("staging").is_dir());
    let id = case.info().id;
    case.close();

    let reopened = open_case(dir.path()).expect("open");
    let info = reopened.info();
    assert_eq!(
        (info.id.as_str(), info.name.as_str(), info.locked),
        (id.as_str(), "Operation Test", false)
    );
    assert_eq!(info.schema_version, case_ledger::SCHEMA_VERSION);
    assert_eq!(
        journal_actions(&reopened),
        vec!["case.create", "case.close", "case.open"]
    );
    assert_eq!(
        case_journal::verify(&reopened.ledger, &id)
            .expect("verify")
            .first_broken_seq,
        None
    );
}

#[test]
fn existing_and_legacy_cases_are_never_clobbered() {
    let dir = tempfile::tempdir().expect("tempdir");
    create_case(dir.path(), options("first"))
        .expect("create")
        .close();
    assert!(create_case(dir.path(), options("second")).is_err());

    let legacy = tempfile::tempdir().expect("tempdir");
    std::fs::write(legacy.path().join("old.carve"), b"sqlite").expect("legacy case");
    let error = open_case(legacy.path()).expect_err("legacy refused");
    assert!(format!("{error:#}").contains("older LovelyCarve layout"));
    assert!(create_case(legacy.path(), options("x")).is_err());

    let empty = tempfile::tempdir().expect("tempdir");
    let error = open_case(empty.path()).expect_err("not a case");
    assert!(format!("{error:#}").contains("no LovelyCarve case"));
}

#[test]
fn a_case_is_open_in_one_place_at_a_time() {
    let dir = tempfile::tempdir().expect("tempdir");
    let case = create_case(dir.path(), options("held")).expect("create");
    assert!(open_case(dir.path()).is_err());
    case.close();
    open_case(dir.path()).expect("open after close").close();
}

#[test]
fn evidence_registration_is_idempotent_and_one_per_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut case = create_case(dir.path(), options("c")).expect("create");
    assert_eq!(
        case.register_evidence(&evidence("img", "/x.E01"))
            .expect("add"),
        EvidenceRegistration::Added
    );
    assert!(dir.path().join("evidence").join("img").is_dir());
    let mut resized = evidence("img", "/x.E01");
    resized.size_bytes = 20;
    assert_eq!(
        case.register_evidence(&resized).expect("refresh"),
        EvidenceRegistration::Known
    );
    assert_eq!(case.evidence_refs()[0].size_bytes, 20);
    assert_eq!(
        case.register_evidence(&evidence("other", "/x.E01"))
            .expect("conflict"),
        EvidenceRegistration::SourceTaken {
            existing_image_id: "img".to_string()
        }
    );
    assert_eq!(
        case.evidence_image_id_for_path("/x.E01").as_deref(),
        Some("img")
    );
    case.close();
    let reopened = open_case(dir.path()).expect("open");
    let listed = reopened.evidence_refs();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].source_format, ImageFormat::Ewf);
    assert_eq!(listed[0].added_at, "2026-10-09T00:00:00.000Z");
}

#[test]
fn published_volume_files_live_in_the_volume_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut case = create_case(dir.path(), options("c")).expect("create");
    case.register_evidence(&evidence("img", "/x.E01"))
        .expect("add");
    publish_volume(&mut case, "img", "part:1", "2026-10-09T01:00:00.000Z");
    let record = case.volume("img", "part:1").expect("volume").clone();
    assert!(record.index_is_current());
    let volume_dir = dir
        .path()
        .join("evidence")
        .join("img")
        .join("volumes")
        .join("part-1");
    assert!(volume_dir.join(&record.snapshot_file).is_file());
    assert_eq!(
        case.index_path(&record),
        record
            .index_file
            .as_ref()
            .map(|index_file| volume_dir.join(index_file))
    );
    assert_eq!(
        std::fs::read_dir(case.staging_dir())
            .expect("staging")
            .count(),
        0,
        "published files leave staging"
    );

    // A rebuild retires the previous generation's files.
    let previous_snapshot = volume_dir.join(&record.snapshot_file);
    publish_volume(&mut case, "img", "part:1", "2026-10-09T02:00:00.000Z");
    assert!(!previous_snapshot.exists());
    assert_eq!(
        std::fs::read_dir(&volume_dir).expect("volume dir").count(),
        2
    );
    case.close();

    let reopened = open_case(dir.path()).expect("open");
    assert_eq!(reopened.snapshot_refs().len(), 1);
    assert!(reopened.snapshot_refs()[0].indexed);
}

#[test]
fn removing_evidence_cascades_rows_and_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut case = create_case(dir.path(), options("c")).expect("create");
    for image_id in ["a", "b"] {
        case.register_evidence(&evidence(image_id, &format!("/{image_id}.E01")))
            .expect("add");
        publish_volume(&mut case, image_id, "part:0", "2026-10-09T01:00:00.000Z");
    }
    case.ledger
        .execute(
            "INSERT INTO tag (evidence_id, node_id, label, tagged_at)
             SELECT id, 'node', '重点', 'now' FROM evidence",
            [],
        )
        .expect("tags");

    case.remove_evidence("a").expect("remove");

    assert_eq!(case.evidence_refs().len(), 1);
    assert!(case.volume("a", "part:0").is_none());
    assert!(!dir.path().join("evidence").join("a").exists());
    assert!(dir.path().join("evidence").join("b").is_dir());
    let tags: i64 = case
        .ledger
        .query_row("SELECT COUNT(*) FROM tag", [], |row| row.get(0))
        .expect("count tags");
    assert_eq!(tags, 1, "only b's tag remains");
    assert_eq!(
        journal_actions(&case).last().map(String::as_str),
        Some("evidence.remove")
    );
}

#[test]
fn heal_prunes_orphans_and_repairs_records() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut case = create_case(dir.path(), options("c")).expect("create");
    case.register_evidence(&evidence("img", "/x.E01"))
        .expect("add");
    case.register_evidence(&evidence("gone", "/y.E01"))
        .expect("add");
    publish_volume(&mut case, "img", "part:0", "2026-10-09T01:00:00.000Z");
    publish_volume(&mut case, "img", "part:1", "2026-10-09T01:00:00.000Z");
    publish_volume(&mut case, "gone", "part:0", "2026-10-09T01:00:00.000Z");
    let keep = case.volume("img", "part:0").expect("keep").clone();
    let lost_index = case.volume("img", "part:1").expect("lost").clone();
    let lost_snapshot = case.volume("gone", "part:0").expect("gone").clone();
    case.close();

    let volume_dir = |image_id: &str, segment: &str| {
        dir.path()
            .join("evidence")
            .join(image_id)
            .join("volumes")
            .join(segment)
    };
    std::fs::write(dir.path().join("staging").join("half.db"), b"x").expect("stale staging");
    std::fs::create_dir_all(dir.path().join("evidence").join("ghost")).expect("orphan dir");
    std::fs::write(volume_dir("img", "part-0").join("index-old.db"), b"x").expect("old gen");
    std::fs::create_dir_all(volume_dir("img", "part-9")).expect("orphan volume");
    std::fs::remove_file(
        volume_dir("img", "part-1").join(lost_index.index_file.as_deref().expect("index")),
    )
    .expect("lose index");
    std::fs::remove_file(volume_dir("gone", "part-0").join(&lost_snapshot.snapshot_file))
        .expect("lose snapshot");

    let reopened = open_case(dir.path()).expect("open heals");
    assert_eq!(
        std::fs::read_dir(dir.path().join("staging"))
            .expect("staging")
            .count(),
        0
    );
    assert!(!dir.path().join("evidence").join("ghost").exists());
    assert!(!volume_dir("img", "part-0").join("index-old.db").exists());
    assert!(!volume_dir("img", "part-9").exists());
    assert_eq!(reopened.volume("img", "part:0"), Some(&keep));
    let repaired = reopened.volume("img", "part:1").expect("still published");
    assert_eq!(repaired.index_file, None);
    assert!(reopened.volume("gone", "part:0").is_none());
    reopened.close();
    // Idempotent.
    let again = open_case(dir.path()).expect("open again");
    assert_eq!(again.volumes().len(), 2);
}

#[test]
fn lock_rename_and_remove_keep_foreign_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let case_dir = dir.path().join("case");
    let mut case = create_case(&case_dir, options("c")).expect("create");
    case.set_locked(true).expect("lock");
    assert!(case.info().locked);
    case.rename("renamed").expect("rename");
    assert!(case.rename("  ").is_err());
    case.close();

    let peeked = peek_case(&case_dir).expect("peek");
    assert!(peeked.locked);
    assert_eq!(peeked.name, "renamed");

    std::fs::write(case_dir.join("notes.txt"), b"examiner notes").expect("foreign file");
    remove_case(&case_dir).expect("remove");
    assert!(
        case_dir.join("notes.txt").is_file(),
        "foreign files survive"
    );
    assert!(!case_dir.join("case.db").exists());
    assert!(!case_dir.join("evidence").exists());
    assert!(remove_case(&case_dir).is_err());

    let bare = dir.path().join("bare");
    create_case(&bare, options("bare")).expect("create").close();
    remove_case(&bare).expect("remove");
    assert!(!bare.exists(), "an emptied case directory is removed");
}
