use super::*;
use crate::evidence::image::PartitionKind;

#[test]
fn parse_dsl_id() {
    let id = format!("zfs:part:3:dsl:{}", hex::encode("ROOT/default"));
    let (src, path) = parse_zfs_id(&id).expect("parse");
    match (src, path) {
        (ZfsSource::Partition(3), ZfsPath::Dsl { dataset_path }) => {
            assert_eq!(dataset_path, "ROOT/default");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn parse_zpl_id() {
    let id = format!(
        "zfs:part:3:zpl:{}:{}",
        hex::encode("ROOT/default"),
        hex::encode("/etc/rc.conf")
    );
    let (src, path) = parse_zfs_id(&id).expect("parse");
    match (src, path) {
        (
            ZfsSource::Partition(3),
            ZfsPath::Zpl {
                dataset_path,
                file_path,
            },
        ) => {
            assert_eq!(dataset_path, "ROOT/default");
            assert_eq!(file_path, "/etc/rc.conf");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn parse_pool_root_id() {
    let id = "zfs:part:3:dsl:";
    let (_, path) = parse_zfs_id(id).expect("parse");
    match path {
        ZfsPath::Dsl { dataset_path } => assert!(dataset_path.is_empty()),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn rejects_bogus_prefix() {
    assert!(parse_zfs_id("part:3").is_none());
    assert!(parse_zfs_id("zfs:part:3:bogus:00").is_none());
    assert!(parse_zfs_id("zfs:part:3:dsl:00:extra").is_none());
}

#[test]
fn split_path_examples() {
    assert_eq!(split_path("/COPYRIGHT"), ("", "COPYRIGHT"));
    assert_eq!(split_path("/etc/rc.conf"), ("/etc", "rc.conf"));
    assert_eq!(split_path("/a/b/c"), ("/a/b", "c"));
}

/// Drive the adapter against the FreeBSD test image when the path is
/// supplied via `LOVELYCARVE_ZFS_TEST`. Walks the pool root, descends
/// into `ROOT/default`, and confirms `read_file` returns bytes.
#[test]
fn lists_real_zfs_root_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_ZFS_TEST") else {
        return;
    };
    let image = Arc::new(DiskImage::open(path).expect("ZFS image should open"));
    let zfs_partition = image
        .partitions
        .iter()
        .find(|p| p.kind == PartitionKind::Zfs)
        .cloned()
        .expect("expected at least one ZFS partition");
    let volume = ZfsVolume::open(Arc::clone(&image), &zfs_partition).expect("open ZFS");
    let pool_children = volume.partition_children().expect("pool children");
    assert!(
        pool_children.iter().any(|n| n.name == "ROOT"),
        "pool should expose ROOT dataset"
    );
    let bytes = volume
        .read_file("ROOT/default", "/COPYRIGHT", 8192)
        .expect("read /COPYRIGHT");
    assert!(!bytes.is_empty(), "/COPYRIGHT should yield bytes");
}
