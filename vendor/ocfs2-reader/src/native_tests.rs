use crate::Ocfs2;
use lamfold::{
    BlockCache, BlockSource, FileKind, FoldError, FoldFrontend, NoVerifier, SubstrateCtx,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

struct Source {
    file: File,
    length: u64,
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
#[ignore = "set OCFS2_ORACLE_DIR to native mkfs/kernel/fsck fixture directory"]
fn native_linux_all_files_dates_and_ranges() {
    let directory = std::env::var("OCFS2_ORACLE_DIR").expect("native fixture directory");
    for (block, cluster) in [(512, 4096), (1024, 4096), (2048, 16384), (4096, 65536)] {
        let name = format!("ocfs2-{block}-{cluster}");
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(Path::new(&directory).join(format!("{name}.json"))).unwrap(),
        )
        .unwrap();
        let mut file = File::open(Path::new(&directory).join(format!("{name}.img"))).unwrap();
        let length = file.metadata().unwrap().len();
        assert_eq!(length, 512 * 1024 * 1024);
        let mut source_hash = Sha256::new();
        let mut source_bytes = 0;
        let mut hash_buffer = vec![0; 1024 * 1024];
        loop {
            let count = file.read(&mut hash_buffer).unwrap();
            if count == 0 {
                break;
            }
            source_hash.update(&hash_buffer[..count]);
            source_bytes += count as u64;
        }
        assert_eq!(source_bytes, length);
        assert_eq!(
            format!("{:x}", source_hash.finalize()),
            manifest["image_sha256"].as_str().unwrap()
        );
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let mut reader = Ocfs2::open(Source { file, length }, &mut context)
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(reader.header().block_size, block);
        assert_eq!(reader.header().cluster_size, cluster);
        let mut stack = vec![(reader.root(), String::new())];
        let mut seen = std::collections::HashSet::new();
        let records = manifest["records"].as_object().unwrap();
        while let Some((parent, path)) = stack.pop() {
            for entry in reader
                .read_dir(parent, &mut context)
                .unwrap_or_else(|error| panic!("{name}:{path}: {error:?}"))
            {
                let path = format!("{path}/{}", entry.name);
                assert!(seen.insert(path.clone()));
                let expected = records
                    .get(&path)
                    .unwrap_or_else(|| panic!("unexpected {path}"));
                let metadata = reader.metadata(entry.node, &mut context).unwrap();
                assert_eq!(
                    metadata.mode as u64,
                    expected["mode"].as_u64().unwrap(),
                    "{path} mode"
                );
                assert_eq!(
                    metadata.size,
                    expected["size"].as_u64().unwrap(),
                    "{path} size"
                );
                let times = reader.timestamps(entry.node, &mut context).unwrap();
                for (field, actual) in [
                    ("atime_ns", times.accessed),
                    ("mtime_ns", times.modified),
                    ("ctime_ns", times.changed),
                ] {
                    let nanos = expected[field].as_i64().unwrap();
                    assert_eq!(
                        actual,
                        Some((
                            nanos.div_euclid(1_000_000_000),
                            nanos.rem_euclid(1_000_000_000) as u32
                        )),
                        "{path} {field}"
                    );
                }
                assert!(times.created.is_none());
                let attributes = reader
                    .attributes(entry.node)
                    .unwrap_or_else(|error| panic!("{name}:{path}: EA {error:?}"));
                let expected_attributes = expected["attributes"].as_object().unwrap();
                assert_eq!(
                    attributes.len(),
                    expected_attributes.len(),
                    "{path} EA count"
                );
                for attribute in attributes {
                    let hex: String = attribute
                        .bytes
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect();
                    assert_eq!(
                        Some(hex.as_str()),
                        expected_attributes
                            .get(&attribute.name)
                            .and_then(serde_json::Value::as_str),
                        "{path}:{}",
                        attribute.name
                    );
                }
                match entry.kind {
                    FileKind::Directory => stack.push((entry.node, path)),
                    FileKind::Regular => {
                        assert!(metadata.size <= 512 * 1024 * 1024);
                        let mut full = Vec::new();
                        let mut hasher = Sha256::new();
                        let mut offset = 0;
                        let mut bytes = vec![0; 64 * 1024];
                        while offset < metadata.size {
                            let count = reader
                                .read_at(entry.node, offset, &mut bytes, &mut context)
                                .unwrap();
                            assert!(count != 0);
                            hasher.update(&bytes[..count]);
                            full.extend_from_slice(&bytes[..count]);
                            offset += count as u64;
                        }
                        assert_eq!(offset, metadata.size);
                        assert_eq!(
                            format!("{:x}", hasher.finalize()),
                            expected["sha256"].as_str().unwrap(),
                            "{path} hash"
                        );
                        for offset in [
                            0,
                            block as u64 - 3,
                            block as u64 + 5,
                            131079,
                            metadata.size.saturating_sub(48),
                            metadata.size,
                        ] {
                            let count = reader
                                .read_at(entry.node, offset, &mut bytes[..97], &mut context)
                                .unwrap();
                            assert_eq!(count as u64, metadata.size.saturating_sub(offset).min(97));
                            let start = offset.min(metadata.size) as usize;
                            assert_eq!(
                                &bytes[..count],
                                &full[start..start + count],
                                "{path} range {offset}"
                            );
                        }
                    }
                    FileKind::Symlink => assert_eq!(
                        reader
                            .read_link_text(entry.node, &mut context)
                            .unwrap()
                            .as_deref(),
                        expected["target"].as_str()
                    ),
                    FileKind::Other => assert!(reader
                        .read_at(entry.node, 0, &mut [0; 1], &mut context)
                        .is_err()),
                }
            }
        }
        assert_eq!(seen.len(), records.len());
        eprintln!("OCFS2_NATIVE {name} verified {} names", seen.len());
    }
}
