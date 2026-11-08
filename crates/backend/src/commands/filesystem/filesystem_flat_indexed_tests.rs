use super::*;
use crate::core::casework::case::test_support::{case_with_evidence, publish_volume};
use crate::core::catalog::snapshot::volume_snapshot::SnapshotEntry;
use crate::core::evidence::image::{DiskImage, ImageFormat};
use crate::core::filesystem::tree::fs_tree::Mactb;

#[test]
fn offline_flat_pages_and_category_counts_do_not_read_unindexed_deleted_subtrees(
) -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut case = case_with_evidence(directory.path(), "offline");
    let file = FsNode {
        id: "saved-file".into(),
        name: "saved.txt".into(),
        parent_id: Some("part:0".into()),
        kind: NodeKind::File,
        size: 12,
        deleted: false,
        inode: None,
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    };
    let deleted = FsNode {
        id: "deleted-directory".into(),
        name: "gone".into(),
        kind: NodeKind::Dir,
        deleted: true,
        ..file.clone()
    };
    let index = publish_volume(
        &mut case,
        "offline",
        "part:0",
        "2026-10-11T00:00:00Z",
        &[
            SnapshotEntry {
                parent: "part:0".into(),
                node: file.clone(),
                path: "/saved.txt".into(),
            },
            SnapshotEntry {
                parent: "part:0".into(),
                node: deleted,
                path: "/gone".into(),
            },
        ],
    );
    let image = Arc::new(EvidenceSession::from_indexed_image(
        DiskImage::from_index_metadata(
            "offline".into(),
            directory.path().join("absent.E01"),
            ImageFormat::Ewf,
            1,
        )?,
        Vec::new(),
    ));
    let sources = || FlatSources {
        offline_truncated: false,
        index_targets: vec![SnapshotIndexTarget {
            index: index.clone(),
            volume_key: "part:0".into(),
            vol_id: 0,
        }],
        snapshot: None,
    };
    let request = FlatRequest {
        image_id: "offline".into(),
        parent_id: "part:0".into(),
        filter: FlatFileFilter::default(),
        sort: FlatSort::default(),
        skip: 0,
        limit: 1,
    };
    let cache = parking_lot::Mutex::new(FlatWalkCache::default());
    let page = compute_flat_page(&image, sources(), &request, &cache).expect("offline flat page");
    assert_eq!(
        (
            page.total,
            page.truncated,
            page.items
                .iter()
                .map(|entry| entry.node.id.as_str())
                .collect::<Vec<_>>()
        ),
        (1, true, vec![file.id.as_str()])
    );
    let counts = compute_ext_counts(&image, sources(), "offline", "part:0", &cache)
        .expect("offline extension counts");
    assert_eq!(
        counts,
        FsFlatExtCounts {
            counts: BTreeMap::from([("txt".into(), 1)]),
            total: 1
        }
    );
    Ok(())
}
