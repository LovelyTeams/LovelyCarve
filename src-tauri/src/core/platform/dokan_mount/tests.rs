use super::is_docker_mount_root;
use super::{read_node_range_cached, EvidenceSession, DOKAN_STREAM_CHUNK};
use crate::core::{
    evidence::{image::DiskImage, partitions::SectorSource},
    filesystem::tree::fs_tree::{FsNode, Mactb, NodeKind},
};
use parking_lot::Mutex;
use std::sync::Arc;

#[test]
fn mount_volume_labels_remove_shell_separators_and_keep_the_length_limit() {
    assert_eq!(
        super::sanitize_volume_label("卷 (NTFS) · Backup"),
        "卷_NTFS_Backup"
    );
    assert_eq!(super::sanitize_volume_label(" · () "), "volume");
    assert_eq!(
        super::sanitize_volume_label(&"卷".repeat(40))
            .chars()
            .count(),
        32
    );
}

#[test]
fn recognizes_archive_and_host_docker_projections() {
    assert!(is_docker_mount_root("docker:"));
    assert!(is_docker_mount_root("docker:2f657463"));
    assert!(is_docker_mount_root("host-docker:"));
    assert!(is_docker_mount_root("host-docker:abc:"));
    assert!(is_docker_mount_root("host-docker:abc:2f657463"));
    assert!(!is_docker_mount_root("part:0"));
    assert!(!is_docker_mount_root("host-docker-invalid"));
}

#[test]
fn cached_reads_follow_seeks_tail_and_eof() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let bytes: Vec<u8> = (0..DOKAN_STREAM_CHUNK + 128)
        .map(|index| (index % 251) as u8)
        .collect();
    std::fs::write(directory.path().join("payload.bin"), &bytes)?;
    let image = Arc::new(EvidenceSession::open(directory.path())?);
    let node = crate::core::filesystem::tree::fs_tree::children_for_image(&image, "folder:")?
        .into_iter()
        .find(|node| node.name == "payload.bin")
        .ok_or_else(|| anyhow::anyhow!("folder fixture has no payload node"))?;
    let cache = Mutex::new(None);
    let mut buffer = [0; 64];

    assert_eq!(
        read_node_range_cached(&image, &node, &cache, 0, &mut buffer)?,
        64
    );
    assert_eq!(buffer.as_slice(), &bytes[..64]);
    assert_eq!(
        read_node_range_cached(&image, &node, &cache, 100, &mut buffer)?,
        64
    );
    assert_eq!(buffer.as_slice(), &bytes[100..164]);
    assert_eq!(cache.lock().as_ref().map(|window| window.start), Some(0));

    let tail = bytes.len() - 17;
    buffer.fill(255);
    assert_eq!(
        read_node_range_cached(&image, &node, &cache, tail as u64, &mut buffer)?,
        17
    );
    assert_eq!(&buffer[..17], &bytes[tail..]);
    assert_eq!(&buffer[17..], &[255; 47]);
    assert_eq!(
        cache.lock().as_ref().map(|window| window.start),
        Some(tail as u64)
    );
    assert_eq!(
        read_node_range_cached(&image, &node, &cache, node.size, &mut buffer)?,
        0
    );
    assert_eq!(
        read_node_range_cached(&image, &node, &cache, 0, &mut [])?,
        0
    );

    assert_eq!(
        read_node_range_cached(&image, &node, &cache, 12, &mut buffer)?,
        64
    );
    assert_eq!(buffer.as_slice(), &bytes[12..76]);
    Ok(())
}

/// A large virtual disk that refuses prefix reads. The test can seek beyond
/// 3 GiB without allocating a disk or allowing stream-and-skip to pass.
struct SeekOnlySource {
    minimum_offset: u64,
    size: u64,
    reads: Arc<Mutex<Vec<(u64, usize)>>>,
}

impl SectorSource for SeekOnlySource {
    fn read(&self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(offset >= self.minimum_offset, "unexpected prefix read");
        self.reads.lock().push((offset, length));
        Ok((0..length)
            .map(|index| ((offset + index as u64) % 251) as u8)
            .collect())
    }

    fn size(&self) -> u64 {
        self.size
    }
}

#[test]
fn cached_seek_reads_only_the_requested_image_window() -> anyhow::Result<()> {
    let base = 4096u64;
    let offset = 3 * 1024 * 1024 * 1024u64;
    let size = 4 * 1024 * 1024 * 1024u64;
    let reads = Arc::new(Mutex::new(Vec::new()));
    let source = Arc::new(SeekOnlySource {
        minimum_offset: base + offset,
        size,
        reads: Arc::clone(&reads),
    });
    let image = Arc::new(EvidenceSession::from(DiskImage::from_assembled(
        "Dokan range test".into(),
        source,
        Vec::new(),
        512,
    )?));
    let node = FsNode {
        id: format!("deleted:run:part:0:0x{base:x}:0x{:x}", size - base),
        name: "payload.bin".into(),
        parent_id: None,
        kind: NodeKind::File,
        size: size - base,
        deleted: true,
        inode: None,
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    };
    let cache = Mutex::new(None);
    let mut buffer = [0u8; 64];
    for position in [offset, offset + 17, offset + 2 * DOKAN_STREAM_CHUNK as u64] {
        assert_eq!(
            read_node_range_cached(&image, &node, &cache, position, &mut buffer)?,
            64
        );
        let expected: Vec<u8> = (0..64)
            .map(|index| ((base + position + index) % 251) as u8)
            .collect();
        assert_eq!(buffer.as_slice(), expected.as_slice());
    }
    assert_eq!(
        *reads.lock(),
        vec![
            (base + offset, DOKAN_STREAM_CHUNK),
            (
                base + offset + 2 * DOKAN_STREAM_CHUNK as u64,
                DOKAN_STREAM_CHUNK
            ),
        ]
    );

    buffer.fill(255);
    assert_eq!(
        read_node_range_cached(&image, &node, &cache, node.size - 17, &mut buffer)?,
        17
    );
    let expected: Vec<u8> = (size - 17..size)
        .map(|position| (position % 251) as u8)
        .collect();
    assert_eq!(&buffer[..17], expected.as_slice());
    assert_eq!(&buffer[17..], &[255; 47]);
    assert_eq!(reads.lock().last().copied(), Some((size - 17, 17)));
    assert_eq!(
        read_node_range_cached(&image, &node, &cache, node.size, &mut buffer)?,
        0
    );
    assert_eq!(reads.lock().len(), 3);
    Ok(())
}

#[test]
fn short_reads_fail_without_publishing_a_cache_window() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("payload.bin"), [1u8; 32])?;
    let image = Arc::new(EvidenceSession::open(directory.path())?);
    let mut node = crate::core::filesystem::tree::fs_tree::children_for_image(&image, "folder:")?
        .into_iter()
        .find(|node| node.name == "payload.bin")
        .ok_or_else(|| anyhow::anyhow!("folder fixture has no payload node"))?;
    // Simulate a source truncated after its directory metadata was cached.
    node.size += 1;
    let cache = Mutex::new(None);
    let mut buffer = [255u8; 64];
    let error = read_node_range_cached(&image, &node, &cache, 0, &mut buffer)
        .expect_err("short reads must fail");
    assert!(error.to_string().contains("short Dokan read"));
    assert_eq!(buffer, [255; 64]);
    assert!(cache.lock().is_none());
    Ok(())
}
