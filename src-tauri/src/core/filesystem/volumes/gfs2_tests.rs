use super::*;
use crate::core::{
    evidence::image::PartitionKind,
    filesystem::{
        evidence_session::EvidenceSession,
        tree::fs_tree::{self, FsSession},
    },
};
use std::io::Read;
#[test]
fn gfs2_bare_and_mbr_read_range_stream_and_attributes_keep_source_identity() {
    let mut volume = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../../../../vendor/gfs2-reader/tests/fixtures/invented-4096.zlib")
            .as_slice(),
    )
    .read_to_end(&mut volume)
    .unwrap();
    for embedded in [false, true] {
        let mut bytes = if embedded {
            vec![0; 1024 * 1024]
        } else {
            Vec::new()
        };
        if embedded {
            bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
            bytes[450] = 0x83;
            bytes[454..458].copy_from_slice(&2048u32.to_le_bytes());
            bytes[458..462].copy_from_slice(&((volume.len() / 512) as u32).to_le_bytes());
        }
        bytes.extend_from_slice(&volume);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gfs2.img");
        std::fs::write(&path, &bytes).unwrap();
        let image = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
        assert_eq!(image.partitions[0].kind, PartitionKind::Gfs2);
        let mut session = FsSession::new(&image);
        let root = session.children("part:0").unwrap();
        assert_eq!(root.len(), 6);
        let hello = root.iter().find(|node| node.name == "hello").unwrap();
        let alias = root.iter().find(|node| node.name == "hardlink").unwrap();
        assert_eq!(hello.inode, alias.inode);
        assert_ne!(hello.id, alias.id);
        assert_eq!(
            session.read_file_limited(&hello.id, 100).unwrap(),
            b"hello native-style GFS2\n"
        );
        let mut streamed = Vec::new();
        fs_tree::stream_file_chunks(&image, &hello.id, 4096, |chunk| {
            streamed.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(streamed, b"hello native-style GFS2\n");
        let sparse = root.iter().find(|node| node.name == "sparse").unwrap();
        let complete = session.read_file_limited(&sparse.id, 20000).unwrap();
        assert_eq!(
            session.read_file_range(&sparse.id, 4091, 97).unwrap(),
            complete[4091..4188]
        );
        let streams = fs_tree::list_data_streams(&image, &hello.id).unwrap();
        assert!(streams
            .iter()
            .any(|stream| stream.name == "user.external" && stream.size == 100));
        assert_eq!(
            fs_tree::read_named_stream(&image, &hello.id, "user.inline", 100).unwrap(),
            b"\0\xfffixture"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
