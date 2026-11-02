//! Follow ordinary CLI paths from RAID detection through LVM/Btrfs to bytes.
use std::{io::Write, sync::Arc};

use serde_json::{json, Value};

use super::{
    browsing::LastLink,
    catalog,
    context::{Context, Exec, Mode, Request},
    Output,
};
use crate::{
    core::{
        evidence::partitions::lvm2::test_support as lvm,
        filesystem::volumes::btrfs::test_support as btrfs,
    },
    host::AppHandle,
    state::AppState,
};

async fn execute(exec: &Exec, context: &mut Context, path: &str, args: Value) -> Value {
    let command = catalog::catalog().unwrap().find_path(path).unwrap();
    let values = args.as_object().unwrap().clone();
    command.validate(&values).unwrap();
    match exec
        .execute(
            context,
            Request {
                timezone: None,
                command,
                values,
                ask: vec![],
                force: false,
                dry_run: false,
                text: false,
                fields: None,
                with_source: false,
            },
        )
        .await
        .unwrap()
        .output
    {
        Output::Json(result) => result.value().clone(),
        Output::Bytes(bytes) => json!({"data": hex::encode(bytes)}),
        Output::Written => panic!("session output"),
    }
}

#[test]
fn raid_lvm_btrfs_navigation_uses_selected_images_and_discovered_paths() {
    // Match the CLI's worker stack: generated dispatch and parser futures are
    // larger than the Windows test harness's default stack.
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
    let mut source = tempfile::NamedTempFile::new().unwrap();
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
    source.write_all(&bytes).unwrap();
    let state = Arc::new(AppState::new());
    let exec = Exec {
        reporter: super::progress::Reporter::silent(),
        host: AppHandle::new(Some(temporary.path().to_owned()), |_, _| Ok(())),
        state: state.clone(),
        mode: Mode::Session,
        quiet: true,
        background: Default::default(),
    };
    let mut context = Context::default();
    let source_info = execute(
        &exec,
        &mut context,
        "images open",
        json!({"path": source.path()}),
    )
    .await;
    execute(
        &exec,
        &mut context,
        "images open",
        json!({"path": temporary.path()}),
    )
    .await;
    let selected = execute(
        &exec,
        &mut context,
        "context set",
        json!({"imageId": source_info["id"]}),
    )
    .await;
    assert_eq!(selected["image"]["id"], source_info["id"]);
    assert_eq!(selected["dir"], "/");
    let stack = execute(&exec, &mut context, "raid detect", json!({})).await;
    if on_md {
        assert_eq!(stack["arrays"].as_array().unwrap().len(), 1);
        assert_eq!(
            stack["volumeGroups"][0]["members"][0]["imageId"],
            stack["arrays"][0]["assembledImageId"]
        );
    }
    let logical_image = stack["volumeGroups"][0]["logicalVolumes"][0]["assembledImageId"]
        .as_str()
        .unwrap()
        .to_owned();
    let again = execute(&exec, &mut context, "raid detect", json!({})).await;
    assert_eq!(
        again["volumeGroups"][0]["logicalVolumes"][0]["assembledImageId"],
        logical_image
    );
    let selected = execute(
        &exec,
        &mut context,
        "context use",
        json!({"imageId": logical_image}),
    )
    .await;
    assert_eq!(selected["image"]["id"], logical_image);
    let roots = execute(&exec, &mut context, "files list", json!({})).await;
    let volume_name = roots["items"][0]["name"].as_str().unwrap();
    assert!(
        !volume_name.contains('/'),
        "volume label must be one navigation component"
    );
    assert_eq!(roots["items"][0]["kind"], "dir");
    execute(
        &exec,
        &mut context,
        "context set",
        json!({"dir": volume_name}),
    )
    .await;
    let files = execute(&exec, &mut context, "files list", json!({})).await;
    assert!(files["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["name"] == "hello.txt"));
    let bytes = execute(
        &exec,
        &mut context,
        "files read",
        json!({"path": "hello.txt"}),
    )
    .await;
    assert_eq!(bytes["data"], hex::encode(btrfs::CONTENT));
    assert!(exec
        .locate(&context, &logical_image, "HELLO.TXT", LastLink::Follow)
        .await
        .is_err());
    let info = execute(&exec, &mut context, "images get", json!({})).await;
    for reference in [
        logical_image.clone(),
        logical_image.to_uppercase(),
        info["path"].as_str().unwrap().to_owned(),
    ] {
        let reopened = execute(
            &exec,
            &mut context,
            "images open",
            json!({"path": reference}),
        )
        .await;
        assert_eq!(reopened["id"], logical_image);
        assert_eq!(context.pwd(), "/");
    }
    let images = execute(&exec, &mut context, "images list", json!({})).await;
    assert_eq!(
        images.as_array().unwrap().len(),
        if on_md { 4 } else { 3 },
        "reopening must reuse the registered volume"
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
                let exec = Exec {
                    reporter: super::progress::Reporter::silent(),
                    host: AppHandle::new(Some(temporary.path().to_owned()), |_, _| Ok(())),
                    state: state.clone(),
                    mode: Mode::Session,
                    quiet: true,
                    background: Default::default(),
                };
                let mut context = Context::default();
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
                    let image =
                        execute(&exec, &mut context, "images open", json!({"path": path})).await;
                    sources.push(image["id"].as_str().unwrap().to_owned());
                    expected.push(content);
                }
                let ambiguous = execute(&exec, &mut context, "raid detect", json!({})).await;
                assert!(
                    ambiguous["volumeGroups"][0]["logicalVolumes"][0]["assembledImageId"].is_null()
                );
                let mut assembled_ids = Vec::new();
                let mut assembled_paths = Vec::new();
                for (source, content) in sources.iter().zip(&expected) {
                    let stack = execute(
                        &exec,
                        &mut context,
                        "raid detect",
                        json!({"imageIds": [source]}),
                    )
                    .await;
                    assert_eq!(stack["images"].as_array().unwrap().len(), 1);
                    let image_id = stack["volumeGroups"][0]["logicalVolumes"][0]
                        ["assembledImageId"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    let again = execute(
                        &exec,
                        &mut context,
                        "raid detect",
                        json!({"imageIds": [source, source]}),
                    )
                    .await;
                    assert_eq!(
                        again["volumeGroups"][0]["logicalVolumes"][0]["assembledImageId"],
                        image_id
                    );
                    execute(
                        &exec,
                        &mut context,
                        "context use",
                        json!({"imageId": image_id}),
                    )
                    .await;
                    let roots = execute(&exec, &mut context, "files list", json!({})).await;
                    let volume = roots["items"][0]["name"].as_str().unwrap();
                    let bytes = execute(
                        &exec,
                        &mut context,
                        "files read",
                        json!({"path": format!("/{volume}/hello.txt")}),
                    )
                    .await;
                    assert_eq!(bytes, json!({"data": hex::encode(content)}));
                    let info = execute(&exec, &mut context, "images get", json!({})).await;
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
                        Some(ids),
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
                    let image = state.images.lock().await.get(source).unwrap().clone();
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
                        Some(vec![format!("reopened-{index}")]),
                    )
                    .await
                    .unwrap();
                    let id = stack.volume_groups[0].logical_volumes[0]
                        .assembled_image_id
                        .as_ref()
                        .unwrap();
                    assert_eq!(
                        restarted.images.lock().await[id].path.to_string_lossy(),
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
fn caseless_ntfs_ids_case_matching_and_absolute_tree_paths() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                for mode in [Mode::OneShot, Mode::Session] {
                    let temporary = tempfile::tempdir().unwrap();
                    let state = Arc::new(AppState::new());
                    let exec = Exec {
                        reporter: super::progress::Reporter::silent(),
                        host: AppHandle::new(Some(temporary.path().to_owned()), |_, _| Ok(())),
                        state: state.clone(),
                        mode,
                        quiet: true,
                        background: Default::default(),
                    };
                    let mut context = Context::default();
                    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("vendor/ewf/tests/data/ntfs_sample.E01");
                    execute(&exec, &mut context, "images open", json!({"path": path})).await;
                    let roots = execute(&exec, &mut context, "files list", json!({})).await;
                    let volume = roots["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|node| node["id"] == "part:0")
                        .unwrap();
                    let volume_name = volume["name"].as_str().unwrap();
                    execute(
                        &exec,
                        &mut context,
                        "context set",
                        json!({"nodeId": "part:0"}),
                    )
                    .await;
                    let file = execute(
                        &exec,
                        &mut context,
                        "files get",
                        json!({"path": "FILE1.TXT"}),
                    )
                    .await;
                    assert_eq!(file["name"], "file1.txt");
                    assert_eq!(file["path"], format!("/{volume_name}/file1.txt"));
                    for indexed in [false, true] {
                        if indexed {
                            exec.background.wait(&state).await.unwrap();
                        }
                        let by_id = execute(
                            &exec,
                            &mut context,
                            "files get",
                            json!({"nodeId": file["id"]}),
                        )
                        .await;
                        assert_eq!(by_id["path"], file["path"]);
                        assert_eq!(by_id["id"], file["id"]);
                    }
                    for args in [
                        json!({"path": ".", "name": "file1"}),
                        json!({"nodeId": "part:0", "name": "file1"}),
                    ] {
                        let tree = execute(&exec, &mut context, "files tree list", args).await;
                        assert_eq!(tree["items"][0]["path"], file["path"]);
                        let round_trip = execute(
                            &exec,
                            &mut context,
                            "files get",
                            json!({"path": tree["items"][0]["path"]}),
                        )
                        .await;
                        assert_eq!(round_trip["id"], file["id"]);
                    }
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
fn duplicate_complex_volume_names_can_be_entered_by_id_and_path_before_indexing() {
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
                let mut image = DiskImage::open_with_id("duplicate-volumes".into(), path).unwrap();
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
                let exec = Exec {
                    reporter: super::progress::Reporter::silent(),
                    host: AppHandle::new(Some(temporary.path().to_owned()), |_, _| Ok(())),
                    state,
                    mode: Mode::Session,
                    quiet: true,
                    background: Default::default(),
                };
                let mut context = Context {
                    image_id: Some("duplicate-volumes".into()),
                    ..Context::default()
                };
                let info = execute(&exec, &mut context, "images get", json!({})).await;
                assert_eq!(info["partitions"][0]["label"], label);
                assert!(exec
                    .locate(
                        &context,
                        "duplicate-volumes",
                        &format!("/{navigation_name}"),
                        LastLink::Follow
                    )
                    .await
                    .unwrap_err()
                    .message
                    .contains("不唯一"));
                for partition in [0, 2] {
                    let alias = format!("/part:{partition}");
                    let named = execute(
                        &exec,
                        &mut context,
                        "files get",
                        json!({"path": format!("{alias}/FILE1.TXT")}),
                    )
                    .await;
                    assert!(named["id"]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!("ntfs:part:{partition}:")));
                    assert_eq!(named["path"], format!("/{navigation_name}/file1.txt"));
                    let by_alias = execute(
                        &exec,
                        &mut context,
                        "files read",
                        json!({"path": format!("{alias}/file1.txt")}),
                    )
                    .await;
                    let by_id = execute(
                        &exec,
                        &mut context,
                        "files read",
                        json!({"nodeId": named["id"]}),
                    )
                    .await;
                    assert_eq!(by_alias, by_id);
                }
                let selected = execute(
                    &exec,
                    &mut context,
                    "context set",
                    json!({"dir": "/part:2"}),
                )
                .await;
                assert_eq!(selected["dir"], format!("/{navigation_name}"));
                assert_eq!(context.location.last().unwrap().1, "part:2");
                let file = execute(
                    &exec,
                    &mut context,
                    "files get",
                    json!({"path": "FILE1.TXT"}),
                )
                .await;
                assert!(file["id"].as_str().unwrap().starts_with("ntfs:part:2:"));
                let located = execute(
                    &exec,
                    &mut context,
                    "files get",
                    json!({"nodeId": file["id"]}),
                )
                .await;
                assert_eq!(located["path"], format!("/{navigation_name}/file1.txt"));
                execute(&exec, &mut context, "context set", json!({"dir": ".."})).await;
                assert_eq!(context.pwd(), "/");
                assert!(super::navigation::locate_node(
                    &exec,
                    "duplicate-volumes",
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
            DiskImage::open_with_id("apple".into(), fixtures.join(file)).unwrap(),
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
        let mut image = DiskImage::open_with_id("policy".into(), source.clone()).unwrap();
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
