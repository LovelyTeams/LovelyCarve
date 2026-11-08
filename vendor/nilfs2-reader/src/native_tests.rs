use crate::Nilfs2;
use lamfold::{BlockCache, BlockSource, FoldError, FoldFrontend, NoVerifier, SubstrateCtx};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

struct Source {
    file: File,
    length: u64,
}
impl Source {
    fn open(path: &Path) -> Self {
        let file = File::open(path).unwrap();
        let length = file.metadata().unwrap().len();
        Self { file, length }
    }
}
impl BlockSource for Source {
    fn len(&self) -> u64 {
        self.length
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> lamfold::Result<()> {
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| FoldError::Io("native seek"))?;
        self.file
            .read_exact(bytes)
            .map_err(|_| FoldError::Io("native read"))
    }
}
#[test]
#[ignore = "set NILFS2_ORACLE_DIR to native mkfs/kernel snapshot inputs"]
fn native_linux_first_volume_bootstrap() {
    let directory = std::env::var("NILFS2_ORACLE_DIR").expect("native fixture directory");
    let source = Source::open(&Path::new(&directory).join("nilfs2-1024-2048-block_count.img"));
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = Nilfs2::open(source, &mut context).unwrap();
    let root = reader.root();
    let entries = reader.read_dir(root, &mut context).unwrap();
    let hello = entries.iter().find(|entry| entry.name == "hello").unwrap();
    let mut bytes = vec![0; 100];
    let length = reader
        .read_at(hello.node, 0, &mut bytes, &mut context)
        .unwrap();
    assert_eq!(&bytes[..length], b"native NILFS2 evidence\n");
    assert_eq!(reader.snapshots().unwrap().len(), 2);
    println!(
        "NILFS2_BOOTSTRAP root entries {}, checkpoint {}",
        entries.len(),
        reader.checkpoint().number
    );
}

fn audit(reader: &mut Nilfs2<Source>, records: &serde_json::Map<String, serde_json::Value>) {
    use sha2::{Digest, Sha256};
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut pending = vec![(reader.root(), String::new())];
    let mut directories = std::collections::HashSet::from([reader.root()]);
    let mut found = std::collections::BTreeSet::new();
    while let Some((directory, parent)) = pending.pop() {
        for entry in reader
            .read_dir(directory, &mut context)
            .unwrap_or_else(|error| panic!("directory {parent}: {error:?}"))
        {
            let path = format!("{parent}/{}", entry.name);
            let expected = records
                .get(&path)
                .unwrap_or_else(|| panic!("unexpected name {path}"));
            assert!(found.insert(path.clone()));
            assert_eq!(
                entry.node,
                expected["inode"].as_u64().unwrap(),
                "inode {path}"
            );
            let metadata = reader.metadata(entry.node, &mut context).unwrap();
            assert_eq!(
                metadata.mode as u64,
                expected["mode"].as_u64().unwrap(),
                "mode {path}"
            );
            assert_eq!(
                metadata.size,
                expected["size"].as_u64().unwrap(),
                "size {path}"
            );
            let times = reader.timestamps(entry.node, &mut context).unwrap();
            assert!(
                times.created.is_none() && times.accessed.is_none(),
                "unrecorded times {path}"
            );
            for (actual, key) in [(times.modified, "mtime_ns"), (times.changed, "ctime_ns")] {
                let actual = actual.unwrap();
                assert_eq!(
                    actual.0 as i128 * 1_000_000_000 + actual.1 as i128,
                    expected[key].as_i64().unwrap() as i128,
                    "{key} {path}"
                );
            }
            match metadata.kind {
                lamfold::FileKind::Directory => {
                    assert!(directories.insert(entry.node), "directory alias {path}");
                    pending.push((entry.node, path));
                }
                lamfold::FileKind::Regular => {
                    assert!(metadata.size <= 64 * 1024 * 1024);
                    let mut whole = Vec::new();
                    let mut offset = 0;
                    let mut buffer = vec![0; 65536];
                    loop {
                        let length = reader
                            .read_at(entry.node, offset, &mut buffer, &mut context)
                            .unwrap_or_else(|error| {
                                panic!("read {path} offset {offset}: {error:?}")
                            });
                        if length == 0 {
                            break;
                        }
                        whole.extend_from_slice(&buffer[..length]);
                        offset += length as u64;
                    }
                    assert_eq!(offset, metadata.size, "complete length {path}");
                    assert_eq!(
                        format!("{:x}", Sha256::digest(&whole)),
                        expected["sha256"].as_str().unwrap(),
                        "hash {path}"
                    );
                    let block = reader.header().block_size as u64;
                    for offset in [
                        0,
                        metadata.size / 3,
                        block - 5,
                        block * 63 - 11,
                        metadata.size.saturating_sub(31),
                        metadata.size,
                        metadata.size + 7,
                    ] {
                        let mut ranged = vec![0; 227];
                        let length = reader
                            .read_at(entry.node, offset, &mut ranged, &mut context)
                            .unwrap();
                        let start = (offset as usize).min(whole.len());
                        let end = (start + ranged.len()).min(whole.len());
                        assert_eq!(
                            &ranged[..length],
                            &whole[start..end],
                            "range {path} offset {offset}"
                        );
                    }
                }
                lamfold::FileKind::Symlink => assert_eq!(
                    reader.read_link(entry.node, &mut context).unwrap().unwrap(),
                    expected["target"].as_str().unwrap().as_bytes(),
                    "link {path}"
                ),
                lamfold::FileKind::Other => assert!(
                    reader
                        .read_at(entry.node, 0, &mut [0; 16], &mut context)
                        .is_err(),
                    "special inode {path}"
                ),
            }
        }
    }
    assert_eq!(
        found,
        records.keys().cloned().collect(),
        "complete native name set"
    );
}

#[test]
#[ignore = "set NILFS2_ORACLE_DIR to native mkfs/kernel snapshot inputs"]
fn native_linux_all_current_and_historical_views() {
    use sha2::{Digest, Sha256};
    let directory = std::env::var("NILFS2_ORACLE_DIR").expect("native fixture directory");
    for name in [
        "nilfs2-1024-2048-block_count",
        "nilfs2-2048-1024-block_count",
        "nilfs2-4096-512-block_count",
        "nilfs2-4096-2048-none",
        "nilfs2-4096-512-cleaned",
    ] {
        let gc_directory =
            std::env::var("NILFS2_GC_ORACLE_DIR").unwrap_or_else(|_| directory.clone());
        let selected_directory = if name.ends_with("cleaned") {
            &gc_directory
        } else {
            &directory
        };
        let path = Path::new(selected_directory).join(format!("{name}.img"));
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(Path::new(selected_directory).join(format!("{name}.json"))).unwrap(),
        )
        .unwrap();
        let mut source = Source::open(&path);
        assert_eq!(source.len(), 512 * 1024 * 1024);
        let mut hash = Sha256::new();
        let mut bytes = vec![0; 1024 * 1024];
        let mut total = 0;
        loop {
            let length = source.file.read(&mut bytes).unwrap();
            if length == 0 {
                break;
            }
            hash.update(&bytes[..length]);
            total += length;
        }
        assert_eq!(total, source.len() as usize);
        assert_eq!(
            format!("{:x}", hash.finalize()),
            manifest["image_sha256"].as_str().unwrap()
        );
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let mut current = Nilfs2::open(Source::open(&path), &mut context)
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        audit(&mut current, manifest["records"].as_object().unwrap());
        let snapshots = current.snapshots().unwrap();
        let history = manifest["snapshots"].as_object().unwrap();
        assert_eq!(snapshots.len(), history.len());
        assert_eq!(
            current.checkpoint().blocks.is_some(),
            !name.ends_with("none")
        );
        for snapshot in snapshots {
            let mut reader = Nilfs2::open_checkpoint(Source::open(&path), snapshot.number)
                .unwrap_or_else(|error| panic!("{name} checkpoint {}: {error:?}", snapshot.number));
            assert!(reader.checkpoint().snapshot);
            audit(
                &mut reader,
                history[&snapshot.number.to_string()].as_object().unwrap(),
            );
        }
        println!(
            "NILFS2_NATIVE {name} verified {} current names and {} historical snapshots",
            manifest["records"].as_object().unwrap().len(),
            history.len()
        );
    }
}
