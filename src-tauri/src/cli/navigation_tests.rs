//! Follow ordinary CLI paths from RAID detection through LVM/Btrfs to bytes.
use std::{io::Write, sync::Arc};

use serde_json::{json, Value};

use super::{
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
                command,
                values,
                ask: vec![],
                force: false,
                dry_run: false,
                text: false,
            },
        )
        .await
        .unwrap()
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
    super::native::wait_idle(&state).await.unwrap();
}
