//! Follow ordinary command lines from RAID detection through LVM/Btrfs to
//! bytes, and resolve addresses by name, by root ID and by node ID.
use std::{io::Write, sync::Arc};

use serde_json::{json, Value};

use super::{
    exec::{Exec, Mode, RequestOptions},
    CliError, Output,
};
use crate::{
    core::{
        common::ids::ImageId, evidence::partitions::lvm2::test_support as lvm,
        filesystem::volumes::btrfs::test_support as btrfs,
    },
    host::AppHandle,
    state::AppState,
};

fn exec(directory: &std::path::Path, state: Arc<AppState>, mode: Mode) -> Exec {
    Exec {
        reporter: super::progress::Reporter::silent(),
        host: AppHandle::new(Some(directory.to_owned()), |_, _| Ok(())),
        state,
        mode,
        background: Default::default(),
        workspace: Default::default(),
    }
}

/// Run one command line where the evidence is held.
async fn try_run(exec: &Exec, argv: &[&str]) -> Result<Value, CliError> {
    let words: Vec<String> = argv.iter().map(|word| (*word).to_owned()).collect();
    let command = super::runtime::parse(&words)?
        .command
        .ok_or_else(|| CliError::invalid("no command"))?;
    let output = exec.run(command, &RequestOptions::default()).await?.output;
    Ok(match output {
        Output::Json(json) => json.value().clone(),
        Output::Bytes(bytes) => json!({"data": hex::encode(bytes)}),
        Output::Stream(plan) => {
            json!({"data": hex::encode(super::stream::buffer(exec, plan).await?)})
        }
        Output::Written => Value::Null,
    })
}

async fn run(exec: &Exec, argv: &[&str]) -> Value {
    match try_run(exec, argv).await {
        Ok(value) => value,
        Err(error) => panic!("{argv:?}: {} {}", error.code, error.message),
    }
}

fn names(listing: &Value) -> Vec<String> {
    listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn raid_lvm_btrfs_navigation_uses_assembled_aliases_and_discovered_paths() {
    // Match the CLI's worker stack: command and parser futures are larger
    // than the Windows test harness's default stack.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            for on_md in [false, true] {
                runtime.block_on(navigate_raid_lvm_btrfs(on_md));
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn navigate_raid_lvm_btrfs(on_md: bool) {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("member.raw");
    let mut bytes = lvm::fragmented(&btrfs::image());
    if on_md {
        use crate::core::evidence::raid::mdadm_super::test_support::V1Fields;
        let fields = V1Fields {
            size_sectors: bytes.len() as u64 / 512,
            data_size_sectors: bytes.len() as u64 / 512,
            data_offset_sectors: 16,
            ..V1Fields::new(1, 1)
        };
        let mut member = vec![0; 8192];
        member[4096..8192].copy_from_slice(&fields.encode());
        member.extend_from_slice(&bytes);
        bytes = member;
    }
    std::fs::File::create(&source)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    let state = Arc::new(AppState::new());
    let exec = exec(temporary.path(), state.clone(), Mode::Batch);
    let added = run(
        &exec,
        &[
            "evidence",
            "add",
            source.to_str().unwrap(),
            "--as",
            "member",
        ],
    )
    .await;
    assert_eq!(added["items"][0]["address"], "/member");
    let stack = run(&exec, &["raid", "detect"]).await;
    if on_md {
        assert_eq!(stack["arrays"].as_array().unwrap().len(), 1);
        assert_eq!(
            stack["volumeGroups"][0]["members"][0]["imageId"],
            stack["arrays"][0]["assembledImageId"]
        );
    }
    let logical = stack["volumeGroups"][0]["logicalVolumes"][0]["assembledImageId"]
        .as_str()
        .unwrap()
        .to_owned();
    let address = stack["assembled"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["imageId"] == logical.as_str())
        .unwrap()["path"]
        .as_str()
        .unwrap()
        .to_owned();
    // Detecting again reuses the assembled evidence and its alias.
    let again = run(&exec, &["raid", "detect"]).await;
    assert_eq!(
        again["volumeGroups"][0]["logicalVolumes"][0]["assembledImageId"],
        logical
    );
    let roots = run(&exec, &["ls", &address]).await;
    let volume = roots["items"][0]["path"].as_str().unwrap().to_owned();
    assert_eq!(roots["items"][0]["kind"], "dir");
    assert!(volume.starts_with(&format!("{address}/")));
    assert_eq!(
        volume.matches('/').count(),
        2,
        "a volume is one address component: {volume}"
    );
    let files = run(&exec, &["ls", &volume]).await;
    assert!(names(&files).contains(&"hello.txt".to_owned()), "{files}");
    let read = run(&exec, &["cat", &format!("{volume}/hello.txt")]).await;
    assert_eq!(read["data"], hex::encode(btrfs::CONTENT));
    // Btrfs names are case-sensitive.
    assert!(try_run(&exec, &["cat", &format!("{volume}/HELLO.TXT")])
        .await
        .is_err());
    let listed = run(&exec, &["ls", "/"]).await;
    assert_eq!(
        listed["items"].as_array().unwrap().len(),
        if on_md { 3 } else { 2 },
        "evidence: the member, its assembled array (md) and its logical volume"
    );
    exec.background.wait(&state).await.unwrap();
}

#[test]
fn scoped_raid_detection_isolates_cloned_volume_bytes_and_cache_identity() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let temporary = tempfile::tempdir().unwrap();
                let state = Arc::new(AppState::new());
                let exec = exec(temporary.path(), state.clone(), Mode::Batch);
                let mut sources = Vec::new();
                let mut expected = Vec::new();
                for name in ["first", "second"] {
                    let mut filesystem = btrfs::image();
                    let offset = filesystem
                        .windows(btrfs::CONTENT.len())
                        .position(|bytes| bytes == btrfs::CONTENT)
                        .unwrap();
                    let mut content = btrfs::CONTENT.to_vec();
                    content[0] = if name == "first" { b'A' } else { b'B' };
                    filesystem[offset..offset + content.len()].copy_from_slice(&content);
                    let path = temporary.path().join(format!("{name}.raw"));
                    std::fs::write(&path, lvm::fragmented(&filesystem)).unwrap();
                    let added = run(&exec, &["evidence", "add", path.to_str().unwrap()]).await;
                    sources.push(added["items"][0]["id"].as_str().unwrap().to_owned());
                    expected.push(content);
                }
                let ambiguous = run(&exec, &["raid", "detect"]).await;
                assert!(
                    ambiguous["volumeGroups"][0]["logicalVolumes"][0]["assembledImageId"].is_null()
                );
                let mut assembled_ids = Vec::new();
                let mut assembled_paths = Vec::new();
                for (member, content) in ["first", "second"].iter().zip(&expected) {
                    let stack = run(&exec, &["raid", "detect", "--member", member]).await;
                    assert_eq!(stack["images"].as_array().unwrap().len(), 1);
                    let image_id = stack["volumeGroups"][0]["logicalVolumes"][0]
                        ["assembledImageId"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    let again = run(
                        &exec,
                        &["raid", "detect", "--member", member, "--member", member],
                    )
                    .await;
                    assert_eq!(
                        again["volumeGroups"][0]["logicalVolumes"][0]["assembledImageId"],
                        image_id
                    );
                    let address = stack["assembled"][0]["path"].as_str().unwrap().to_owned();
                    let roots = run(&exec, &["ls", &address]).await;
                    let volume = roots["items"][0]["path"].as_str().unwrap().to_owned();
                    let bytes = run(&exec, &["cat", &format!("{volume}/hello.txt")]).await;
                    assert_eq!(bytes, json!({"data": hex::encode(content)}));
                    let info = run(
                        &exec,
                        &["evidence", "show", address.trim_start_matches('/')],
                    )
                    .await;
                    assembled_paths.push(info["path"].as_str().unwrap().to_owned());
                    assembled_ids.push(image_id);
                }
                assert_ne!(assembled_ids[0], assembled_ids[1]);
                assert_ne!(assembled_paths[0], assembled_paths[1]);
                let image_count = state.images.lock().await.len();
                for (ids, code) in [
                    (vec![], crate::app::error::ErrorCode::InvalidInput),
                    (
                        vec!["missing".into()],
                        crate::app::error::ErrorCode::NotFound,
                    ),
                    (
                        vec![sources[0].clone(), "missing".into()],
                        crate::app::error::ErrorCode::NotFound,
                    ),
                    (
                        vec![assembled_ids[0].clone()],
                        crate::app::error::ErrorCode::InvalidInput,
                    ),
                ] {
                    let error = crate::app::workflows::evidence::raid_auto_detect(
                        exec.host.clone(),
                        crate::host::State::new(&state),
                        vec![],
                        Some(ids.into_iter().map(ImageId::new).collect()),
                    )
                    .await
                    .err()
                    .unwrap();
                    assert_eq!(error.code, code);
                    assert_eq!(state.images.lock().await.len(), image_count);
                }
                // New runtime IDs for the same source paths keep the same
                // synthetic paths; no live image IDs enter the scope digest.
                let restarted = AppState::new();
                for (index, source) in sources.iter().enumerate() {
                    let image = state
                        .images
                        .lock()
                        .await
                        .get(source.as_str())
                        .unwrap()
                        .clone();
                    let disk = crate::core::evidence::image::DiskImage::open_with_id(
                        format!("reopened-{index}"),
                        image.path.clone(),
                    )
                    .unwrap();
                    restarted.images.lock().await.insert(
                        disk.id.clone(),
                        Arc::new(
                            crate::core::filesystem::evidence_session::EvidenceSession::from(disk),
                        ),
                    );
                    let stack = crate::app::workflows::evidence::raid_auto_detect(
                        exec.host.clone(),
                        crate::host::State::new(&restarted),
                        vec![],
                        Some(vec![ImageId::new(format!("reopened-{index}"))]),
                    )
                    .await
                    .unwrap();
                    let id = stack.volume_groups[0].logical_volumes[0]
                        .assembled_image_id
                        .as_ref()
                        .unwrap();
                    assert_eq!(
                        restarted.images.lock().await[id.as_str()]
                            .path
                            .to_string_lossy(),
                        assembled_paths[index]
                    );
                }
                exec.background.wait(&state).await.unwrap();
            });
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn caseless_ntfs_ids_case_matching_and_tree_addresses() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                for mode in [Mode::OneShot, Mode::Batch] {
                    let temporary = tempfile::tempdir().unwrap();
                    let state = Arc::new(AppState::new());
                    let exec = exec(temporary.path(), state.clone(), mode);
                    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("vendor/ewf/tests/data/ntfs_sample.E01");
                    run(
                        &exec,
                        &["evidence", "add", path.to_str().unwrap(), "--as", "ntfs"],
                    )
                    .await;
                    let roots = run(&exec, &["ls", "/ntfs"]).await;
                    assert!(roots["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|node| node["path"] == "/ntfs/part:0"));
                    // NTFS folds case: a unique case-insensitive match is the file.
                    let file = run(&exec, &["stat", "/ntfs/part:0/FILE1.TXT"]).await;
                    assert_eq!(file["name"], "file1.txt");
                    assert_eq!(file["path"], "/ntfs/part:0/file1.txt");
                    for indexed in [false, true] {
                        if indexed {
                            exec.background.wait(&state).await.unwrap();
                        }
                        let by_id = run(
                            &exec,
                            &["stat", &format!("/ntfs/@{}", file["id"].as_str().unwrap())],
                        )
                        .await;
                        assert_eq!(by_id["path"], file["path"]);
                        assert_eq!(by_id["id"], file["id"]);
                    }
                    let tree = run(&exec, &["tree", "/ntfs/part:0", "--name", "file1"]).await;
                    assert_eq!(tree["items"][0]["path"], file["path"]);
                    let round_trip =
                        run(&exec, &["stat", tree["items"][0]["path"].as_str().unwrap()]).await;
                    assert_eq!(round_trip["id"], file["id"]);
                    let from_root =
                        run(&exec, &["tree", "/ntfs", "--depth", "3", "--name", "file1"]).await;
                    assert_eq!(from_root["items"][0]["path"], file["path"]);
                    assert!(state.active_case.lock().await.is_none());
                    exec.background.wait(&state).await.unwrap();
                }
            });
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn duplicate_complex_volume_names_are_entered_by_root_id_and_node_id() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                use crate::core::{
                    evidence::image::DiskImage, filesystem::evidence_session::EvidenceSession,
                };
                let temporary = tempfile::tempdir().unwrap();
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("vendor/ewf/tests/data/ntfs_sample.E01");
                let mut image = DiskImage::open_with_id("duplicate-volumes", path).unwrap();
                let mut first = image.partitions[0].clone();
                let label = "数据卷 (Basic data partition) · 备份";
                let navigation_name = "数据卷_Basic_data_partition_备份";
                first.label = Some(label.into());
                let mut second = first.clone();
                second.id = 2;
                image.partitions = vec![first, second];
                let state = Arc::new(AppState::new());
                state
                    .images
                    .lock()
                    .await
                    .insert(image.id.clone(), Arc::new(EvidenceSession::from(image)));
                let exec = exec(temporary.path(), state, Mode::Batch);
                let alias = run(&exec, &["ls", "/"]).await["items"][0]["name"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let info = run(&exec, &["evidence", "show", &alias]).await;
                assert_eq!(info["partitions"][0]["label"], label);
                let ambiguous = try_run(&exec, &["stat", &format!("/{alias}/{navigation_name}")])
                    .await
                    .unwrap_err();
                assert!(
                    ambiguous.message.contains("不唯一"),
                    "{}",
                    ambiguous.message
                );
                for partition in [0, 2] {
                    let volume = format!("/{alias}/part:{partition}");
                    let named = run(&exec, &["stat", &format!("{volume}/FILE1.TXT")]).await;
                    assert!(named["id"]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!("ntfs:part:{partition}:")));
                    assert_eq!(named["path"], format!("{volume}/file1.txt"));
                    let by_path = run(&exec, &["cat", &format!("{volume}/file1.txt")]).await;
                    let by_id = run(
                        &exec,
                        &[
                            "cat",
                            &format!("/{alias}/@{}", named["id"].as_str().unwrap()),
                        ],
                    )
                    .await;
                    assert_eq!(by_path, by_id);
                }
                assert!(super::navigation::locate_node(
                    &exec,
                    &ImageId::new("duplicate-volumes"),
                    "ntfs:part:2:999999"
                )
                .await
                .is_err());
            });
        })
        .unwrap()
        .join()
        .unwrap();
}

#[tokio::test]
async fn path_case_fallback_obeys_selected_apple_volume_flags_and_filesystem_kind() {
    use crate::core::{
        evidence::image::{DiskImage, PartitionKind},
        filesystem::evidence_session::EvidenceSession,
    };
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/apfs/tests/data");
    for (file, insensitive) in [("basic.apfs", true), ("case-sensitive.apfs", false)] {
        let image = Arc::new(EvidenceSession::from(
            DiskImage::open_with_id("apple", fixtures.join(file)).unwrap(),
        ));
        assert_eq!(
            super::navigation::case_insensitive(image, "apfs:part:0:0:".into())
                .await
                .unwrap(),
            insensitive
        );
    }
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/ewf/tests/data/ntfs_sample.E01");
    for (kind, insensitive) in [
        (PartitionKind::Fat12, true),
        (PartitionKind::Fat16, true),
        (PartitionKind::Fat32, true),
        (PartitionKind::ExFat, true),
        (PartitionKind::HfsPlus, true),
        (PartitionKind::HfsX, false),
        (PartitionKind::Ext4, false),
    ] {
        let mut image = DiskImage::open_with_id("policy", source.clone()).unwrap();
        image.partitions[0].kind = kind;
        let image = Arc::new(EvidenceSession::from(image));
        assert_eq!(
            super::navigation::case_insensitive(image.clone(), "part:0".into())
                .await
                .unwrap(),
            insensitive
        );
        assert!(!super::navigation::case_insensitive(image, String::new())
            .await
            .unwrap());
    }
}
