use super::*;
use crate::filesystem::{
    docker::host_docker,
    fs_dispatch::{self, Volume},
    tree::fs_tree::{ChildrenOptions, FsNode, FsStat},
};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Default)]
struct Provider {
    closed: AtomicBool,
}

#[derive(Debug)]
struct TestVolume(Arc<Provider>);

impl Volume for TestVolume {
    fn children(
        &self,
        _image: &Arc<EvidenceSession>,
        _id: &str,
        _options: ChildrenOptions,
    ) -> anyhow::Result<Vec<FsNode>> {
        Ok(Vec::new())
    }

    fn read_file(&self, _id: &str, maximum: usize) -> anyhow::Result<Vec<u8>> {
        Ok(b"provider"[..maximum.min(8)].to_vec())
    }

    fn read_range(&self, _id: &str, offset: u64, length: usize) -> anyhow::Result<Option<Vec<u8>>> {
        let data = b"provider";
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(data.len());
        let end = start.saturating_add(length).min(data.len());
        Ok(Some(data[start..end].to_vec()))
    }

    fn file_stat(&self, _id: &str, _node_id: &str) -> anyhow::Result<FsStat> {
        anyhow::bail!("stat not supported by test provider")
    }

    fn stream_file(
        &self,
        _id: &str,
        _chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<u64> {
        visitor(b"provider")?;
        Ok(8)
    }
}

impl ExternalVolume for TestVolume {
    fn host_root(&self) -> &str {
        "external:root"
    }

    fn owns_node(&self, node_id: &str) -> bool {
        node_id.starts_with("external:")
    }

    fn is_alive(&self) -> bool {
        !self.0.closed.load(Ordering::SeqCst)
    }

    fn source_id(&self) -> &str {
        "test-source"
    }

    fn close(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async {
            self.0.closed.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[tokio::test]
async fn unloading_registry_keeps_provider_alive_until_last_volume_call_releases_it() {
    let source = tempfile::NamedTempFile::new().expect("source");
    std::fs::write(source.path(), vec![0x5a; 4096]).expect("write");
    let image = Arc::new(DiskImage::open(source.path()).expect("open"));
    let session = Arc::new(EvidenceSession::from(Arc::clone(&image)));
    let provider = Arc::new(Provider::default());
    let backing = Arc::downgrade(&provider);
    session.install_external_volume(TestVolume(provider));
    assert!(session.external_volume("ntfs:unrelated").is_none());
    let call = fs_dispatch::dispatch(&session, "external:file")
        .expect("dispatch")
        .expect("volume");
    assert_eq!(
        call.read_range("external:file", 3, 100).expect("range"),
        Some(b"vider".to_vec())
    );
    let volumes = session.take_external_volumes();
    assert!(session.external_volume("external:file").is_none());
    assert_eq!(volumes.len(), 1);
    volumes[0].close().await.expect("close");
    drop(volumes);
    assert!(backing
        .upgrade()
        .expect("in-flight handle")
        .closed
        .load(Ordering::SeqCst));
    drop(call);
    drop(session);
    assert!(backing.upgrade().is_none());
    // The byte reader can outlive its filesystem state without retaining it.
    assert_eq!(image.read(0, 4).expect("read"), vec![0x5a; 4]);
}

#[test]
fn filesystem_projections_are_scoped_to_session_and_full_source_identity() {
    let source = tempfile::NamedTempFile::new().expect("source");
    std::fs::write(source.path(), vec![0; 4096]).expect("write");
    let image = Arc::new(DiskImage::open(source.path()).expect("open"));
    let session = Arc::new(EvidenceSession::from(Arc::clone(&image)));
    let fresh_session = EvidenceSession::from(Arc::clone(&image));
    let snapshot = Arc::new(NtfsDeletedSnapshot::default());
    let projection = Arc::downgrade(&snapshot);
    session.install_ntfs_deleted_snapshot(NtfsSource::Partition(1), Arc::clone(&snapshot));
    assert!(Arc::ptr_eq(
        &session
            .ntfs_deleted_snapshot(NtfsSource::Partition(1))
            .expect("cached"),
        &snapshot
    ));
    assert!(session
        .ntfs_deleted_snapshot(NtfsSource::Lvm {
            part_id: 1,
            lv_index: 0
        })
        .is_none());
    assert!(fresh_session
        .ntfs_deleted_snapshot(NtfsSource::Partition(1))
        .is_none());
    assert!(host_docker::catalog(&session).is_none());
    assert!(matches!(session.host_docker.get(), Some(None)));
    assert!(fresh_session.host_docker.get().is_none());
    drop(snapshot);
    drop(session);
    assert!(projection.upgrade().is_none());
    assert!(Arc::ptr_eq(fresh_session.disk_image(), &image));
}
