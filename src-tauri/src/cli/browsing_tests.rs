use super::*;
fn insensitive(_: String) -> std::future::Ready<Result<bool, CliError>> {
    std::future::ready(Ok(true))
}
use crate::{
    commands::filesystem::filesystem_page::fs_node_page_from_filedb,
    core::catalog::filedb::{
        filedb_query,
        filedb_store::test_support::{build_test_index, entry},
    },
};

#[tokio::test]
async fn index_pages_cover_large_directories_and_preserve_walk_limits() {
    let directory = tempfile::tempdir().unwrap();
    let mut entries = (0..502)
        .map(|number| {
            entry(
                "part:0",
                &format!("file-{number}"),
                &format!("file{number}.txt"),
                NodeKind::File,
                &format!("/file{number}.txt"),
            )
        })
        .collect::<Vec<_>>();
    entries.push(entry(
        "part:0",
        "nested",
        "nested",
        NodeKind::Dir,
        "/nested",
    ));
    entries.push(entry(
        "nested",
        "report",
        "REPORT.txt",
        NodeKind::File,
        "/nested/REPORT.txt",
    ));
    let index = build_test_index(directory.path(), "part:0", &entries);
    let mut page = |parent: String, cursor: Option<String>, limit| {
        let result = filedb_query::children_page(&index, &parent, cursor.as_deref(), Some(limit))
            .unwrap()
            .unwrap();
        std::future::ready(Ok(fs_node_page_from_filedb(result)))
    };
    let current = vec![("volume".into(), "part:0".into())];
    assert_eq!(
        node(
            &resolve(&current, "file501.txt", &mut page, insensitive)
                .await
                .unwrap(),
            &mut page
        )
        .await
        .unwrap()
        .unwrap()
        .name,
        "file501.txt"
    );
    let cancellation = CancellationToken::new();
    assert_eq!(
        resolve(&current, "nested/../file501.txt", &mut page, insensitive)
            .await
            .unwrap(),
        vec![
            ("volume".into(), "part:0".into()),
            ("file501.txt".into(), "file-501".into())
        ]
    );
    let (items, truncated) = walk(
        &current,
        32,
        10000,
        Some("report"),
        &cancellation,
        &mut page,
    )
    .await
    .unwrap();
    assert_eq!(
        (items.len(), items[0].path.as_str(), truncated),
        (1, "/volume/nested/REPORT.txt", false)
    );
    let (items, truncated) = walk(&current, 1, 503, None, &cancellation, &mut page)
        .await
        .unwrap();
    assert_eq!((items.len(), truncated), (503, false));
    let (items, truncated) = walk(&current, 1, 501, None, &cancellation, &mut page)
        .await
        .unwrap();
    assert_eq!((items.len(), truncated), (501, true));
    assert!(resolve(&current, "missing", &mut page, insensitive)
        .await
        .is_err());
    cancellation.cancel();
    assert!(walk(&current, 32, 10000, None, &cancellation, &mut page)
        .await
        .is_err());
}

#[tokio::test]
async fn duplicate_names_on_separate_index_pages_remain_ambiguous() {
    let directory = tempfile::tempdir().unwrap();
    let mut entries = (0..499)
        .map(|number| {
            entry(
                "part:0",
                &format!("id-{number:03}"),
                &format!("file{number}.txt"),
                NodeKind::File,
                &format!("/file{number}.txt"),
            )
        })
        .collect::<Vec<_>>();
    entries.push(entry(
        "part:0",
        "duplicate-1",
        "z-duplicate",
        NodeKind::File,
        "/z-duplicate",
    ));
    entries.push(entry(
        "part:0",
        "duplicate-2",
        "z-duplicate",
        NodeKind::File,
        "/z-duplicate",
    ));
    let index = build_test_index(directory.path(), "part:0", &entries);
    let mut page = |parent: String, cursor: Option<String>, limit| {
        std::future::ready(Ok(fs_node_page_from_filedb(
            filedb_query::children_page(&index, &parent, cursor.as_deref(), Some(limit))
                .unwrap()
                .unwrap(),
        )))
    };
    let error = resolve(
        &[("volume".into(), "part:0".into())],
        "z-duplicate",
        &mut page,
        insensitive,
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("同名节点不唯一"));
    let error = resolve(
        &[("volume".into(), "part:0".into())],
        "Z-DUPLICATE",
        &mut page,
        insensitive,
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("忽略大小写后节点不唯一"));
}

#[tokio::test]
async fn exact_names_win_over_folded_matches_across_pages() {
    let directory = tempfile::tempdir().unwrap();
    let mut entries = (0..500)
        .map(|number| {
            entry(
                "part:0",
                &format!("id-{number}"),
                &format!("M{number:03}"),
                NodeKind::File,
                &format!("/M{number:03}"),
            )
        })
        .collect::<Vec<_>>();
    for name in ["FILE.txt", "file.txt"] {
        entries.push(entry(
            "part:0",
            name,
            name,
            NodeKind::File,
            &format!("/{name}"),
        ));
    }
    let index = build_test_index(directory.path(), "part:0", &entries);
    let mut page = |parent: String, cursor: Option<String>, limit| {
        std::future::ready(Ok(fs_node_page_from_filedb(
            filedb_query::children_page(&index, &parent, cursor.as_deref(), Some(limit))
                .unwrap()
                .unwrap(),
        )))
    };
    let current = vec![("volume".into(), "part:0".into())];
    for name in ["FILE.txt", "file.txt"] {
        let result = resolve(&current, name, &mut page, insensitive)
            .await
            .unwrap();
        assert_eq!(result.last().unwrap().0, name);
    }
    assert!(resolve(&current, "File.txt", &mut page, insensitive)
        .await
        .unwrap_err()
        .message
        .contains("不唯一"));
    assert_eq!(
        resolve(&current, "m499", &mut page, insensitive)
            .await
            .unwrap()
            .last()
            .unwrap()
            .0,
        "M499"
    );
    assert!(
        resolve(&current, "m499", &mut page, |_| std::future::ready(Ok(
            false
        )))
        .await
        .is_err()
    );
}

#[tokio::test]
async fn unindexed_id_lookup_pages_duplicate_volumes_without_visiting_other_partitions() {
    use crate::{
        commands::filesystem::filesystem_page::fs_node_page_from_sorted_nodes,
        core::catalog::filedb::filedb_store::test_support::node as test_node,
    };
    let directory = tempfile::tempdir().unwrap();
    let mut entries = (0..502)
        .map(|number| {
            entry(
                "part:2",
                &format!("ntfs:part:2:{number}"),
                &format!("file{number:03}"),
                NodeKind::File,
                &format!("/file{number:03}"),
            )
        })
        .collect::<Vec<_>>();
    entries.push(entry(
        "part:2",
        "nested",
        "nested",
        NodeKind::Dir,
        "/nested",
    ));
    entries.push(entry(
        "nested",
        "target",
        "target.txt",
        NodeKind::File,
        "/nested/target.txt",
    ));
    let index = build_test_index(directory.path(), "part:2", &entries);
    let mut page = |parent: String, cursor: Option<String>, limit| {
        let result = if parent.is_empty() {
            fs_node_page_from_sorted_nodes(
                vec![
                    test_node("part:0", "Basic data partition", NodeKind::Dir),
                    test_node("part:2", "Basic data partition", NodeKind::Dir),
                ],
                cursor.as_deref(),
                Some(limit),
            )
            .unwrap()
        } else {
            assert_ne!(parent, "part:0", "ID must route to the requested partition");
            fs_node_page_from_filedb(
                filedb_query::children_page(&index, &parent, cursor.as_deref(), Some(limit))
                    .unwrap()
                    .unwrap(),
            )
        };
        std::future::ready(Ok(result))
    };
    assert_eq!(
        locate_id("ntfs:part:2:501", &CancellationToken::new(), &mut page)
            .await
            .unwrap(),
        vec![
            ("Basic data partition".into(), "part:2".into()),
            ("file501".into(), "ntfs:part:2:501".into())
        ]
    );
    assert!(
        locate_id("ntfs:part:2:999", &CancellationToken::new(), &mut page)
            .await
            .is_err()
    );
    assert!(locate_id("root", &CancellationToken::new(), &mut page)
        .await
        .unwrap()
        .is_empty());
    let mut truncated = |_: String, _: Option<String>, _| {
        std::future::ready(Ok(FsNodePage {
            items: Vec::new(),
            next_cursor: None,
            total: None,
            truncated: true,
        }))
    };
    assert!(
        locate_id("missing", &CancellationToken::new(), &mut truncated)
            .await
            .unwrap_err()
            .message
            .contains("不完整")
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        locate_id("missing", &cancellation, &mut truncated)
            .await
            .unwrap_err()
            .code,
        "cancelled"
    );
}
