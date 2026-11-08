//! Explicit external audit: manifests come from native Linux readers.
use super::*;
use lamfold::{BlockCache, NoVerifier};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

struct FileSource(File, u64);
impl BlockSource for FileSource {
    fn len(&self) -> u64 {
        self.1
    }
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        self.0
            .seek(SeekFrom::Start(offset))
            .and_then(|_| self.0.read_exact(buffer))
            .map_err(|error| FoldError::Source(error.to_string()))
    }
}

fn hash(path: &Path) -> String {
    let mut source = File::open(path).unwrap();
    let mut hasher = Sha256::new();
    let mut bytes = vec![0; 1024 * 1024];
    loop {
        let count = source.read(&mut bytes).unwrap();
        if count == 0 {
            break;
        }
        hasher.update(&bytes[..count]);
    }
    format!("{:x}", hasher.finalize())
}

#[test]
#[ignore = "requires BCACHEFS_ORACLE_DIR with independently generated Linux images and manifests"]
fn native_linux_all_names_metadata_full_hashes_and_ranges() {
    let directory = std::env::var_os("BCACHEFS_ORACLE_DIR").expect("BCACHEFS_ORACLE_DIR required");
    let directory = Path::new(&directory);
    for name in ["512-none", "4096-lz4", "4096-zstd", "4096-gzip"] {
        let path = directory.join(format!("{name}.img"));
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join(format!("{name}.json"))).unwrap())
                .unwrap();
        assert_eq!(
            hash(&path),
            manifest["sha256"].as_str().unwrap(),
            "{name}: source hash"
        );
        let source = File::open(&path).unwrap();
        let size = source.metadata().unwrap().len();
        assert_eq!(size, manifest["size"].as_u64().unwrap());
        let mut cache = BlockCache::new(0);
        let context = &mut SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let mut volume = Bcachefs::open(FileSource(source, size), context).unwrap();
        let mut actual = BTreeMap::new();
        let mut queue = vec![(String::new(), volume.root())];
        while let Some((parent, node)) = queue.pop() {
            for entry in volume.read_dir(node, context).unwrap() {
                let path = if parent.is_empty() {
                    entry.name
                } else {
                    format!("{parent}/{}", entry.name)
                };
                assert!(
                    actual.insert(path.clone(), entry.node).is_none(),
                    "duplicate path {path}"
                );
                if entry.kind == FileKind::Directory {
                    queue.push((path, entry.node));
                }
            }
            assert!(actual.len() < 100_000);
        }
        let entries = manifest["entries"].as_array().unwrap();
        assert_eq!(actual.len(), entries.len(), "{name}: names");
        let mut buffer = vec![0; 1024 * 1024];
        for expected in entries {
            let path = expected["path"].as_str().unwrap();
            let node = *actual.get(path).unwrap();
            let metadata = volume.metadata(node, context).unwrap();
            assert_eq!(
                metadata.mode as u64,
                expected["mode"].as_u64().unwrap(),
                "{name}/{path}: mode"
            );
            assert_eq!(
                volume.inode_number(node, context).unwrap(),
                expected["inode"].as_u64().unwrap(),
                "{name}/{path}: inode"
            );
            let expected_kind = match expected["kind"].as_str().unwrap() {
                "dir" => FileKind::Directory,
                "file" => FileKind::Regular,
                "link" => FileKind::Symlink,
                "special" => FileKind::Other,
                _ => panic!("invalid native kind"),
            };
            assert_eq!(metadata.kind, expected_kind, "{name}/{path}: kind");
            let times = volume.timestamps(node, context).unwrap();
            for (field, actual) in [
                ("atime_ns", times.accessed),
                ("mtime_ns", times.modified),
                ("ctime_ns", times.changed),
            ] {
                let expected = expected[field].as_i64().unwrap();
                assert_eq!(
                    actual,
                    Some((
                        expected.div_euclid(1_000_000_000),
                        expected.rem_euclid(1_000_000_000) as u32
                    )),
                    "{name}/{path}: {field}"
                );
            }
            match metadata.kind {
                FileKind::Regular => {
                    assert_eq!(
                        metadata.size,
                        expected["size"].as_u64().unwrap(),
                        "{name}/{path}: size"
                    );
                    let mut hasher = Sha256::new();
                    let mut offset = 0;
                    while offset < metadata.size {
                        let count = volume.read_at(node, offset, &mut buffer, context).unwrap();
                        assert!(count > 0);
                        hasher.update(&buffer[..count]);
                        offset += count as u64;
                    }
                    assert_eq!(offset, metadata.size);
                    assert_eq!(
                        format!("{:x}", hasher.finalize()),
                        expected["sha256"].as_str().unwrap(),
                        "{name}/{path}: contents"
                    );
                    // The complete stream above matched the native SHA-256;
                    // compare independently sought windows with that logical view.
                    if path == "random.bin" || path == "compressible.bin" || path == "sparse.bin" {
                        for offset in [
                            1,
                            499,
                            4091,
                            65533,
                            metadata.size.saturating_sub(37),
                            metadata.size,
                            u64::MAX,
                        ] {
                            let count = volume
                                .read_at(node, offset, &mut buffer[..113], context)
                                .unwrap();
                            assert_eq!(
                                count,
                                (metadata.size.saturating_sub(offset)).min(113) as usize
                            );
                            let mut control = vec![0; metadata.size.min(32 * 1024 * 1024) as usize];
                            assert_eq!(
                                volume.read_at(node, 0, &mut control, context).unwrap(),
                                control.len()
                            );
                            if count > 0 {
                                assert_eq!(
                                    &buffer[..count],
                                    &control[offset as usize..offset as usize + count],
                                    "{name}/{path}: range at {offset}"
                                );
                            }
                        }
                    }
                }
                FileKind::Symlink => assert_eq!(
                    volume.read_link(node, context).unwrap().unwrap(),
                    expected["target"].as_str().unwrap().as_bytes(),
                    "{name}/{path}: link"
                ),
                FileKind::Other => {
                    assert!(volume.read_at(node, 0, &mut buffer[..1], context).is_err())
                }
                FileKind::Directory => {}
            }
        }
        eprintln!(
            "native {name}: {} names, fields and all regular-file hashes verified",
            entries.len()
        );
    }
}
