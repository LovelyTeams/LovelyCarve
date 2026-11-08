//! Opt-in public native-image audit. The image is never modified or bundled.
use super::*;
use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SubstrateCtx};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

struct FileSource {
    file: File,
    base: u64,
    length: u64,
}
impl BlockSource for FileSource {
    fn len(&self) -> u64 {
        self.length
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        require(
            offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= self.length),
            "native BFS oracle source range",
        )?;
        self.file
            .seek(SeekFrom::Start(self.base + offset))
            .map_err(|error| FoldError::Source(error.to_string()))?;
        self.file
            .read_exact(bytes)
            .map_err(|error| FoldError::Source(error.to_string()))
    }
}
#[test]
#[ignore = "set BEFS_ORACLE_IMAGE to official hrev60295 anyboot image; see ORIGIN.md"]
fn native_haiku_image_all_files_attributes_and_ranges() {
    let path = std::env::var_os("BEFS_ORACLE_IMAGE").expect("BEFS_ORACLE_IMAGE");
    let mut file = File::open(path).unwrap();
    let total = file.metadata().unwrap().len();
    let mut head = [0; 512];
    file.read_exact(&mut head).unwrap();
    let mut base = 0;
    let mut length = total;
    if head[510..] == [0x55, 0xaa] {
        for at in (446..510).step_by(16) {
            if head[at + 4] == 0xeb {
                base = u64::from(u32::from_le_bytes(
                    head[at + 8..at + 12].try_into().unwrap(),
                )) * 512;
                length = u64::from(u32::from_le_bytes(
                    head[at + 12..at + 16].try_into().unwrap(),
                )) * 512;
                break;
            }
        }
    }
    assert!(base.checked_add(length).is_some_and(|end| end <= total));
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = BeFs::open(FileSource { file, base, length }, &mut cx).unwrap();
    reader.set_timestamp_encoding(TimestampEncoding::Haiku);
    let root = reader.root();
    assert!(reader.lookup(root, "system", &mut cx).unwrap().is_some());
    let mut pending = vec![(root, String::new())];
    let mut directories = HashSet::new();
    let mut files = 0;
    let mut attribute_count = 0;
    let mut package_verified = false;
    let mut bytes = vec![0; 1024 * 1024];
    while let Some((directory, path)) = pending.pop() {
        assert!(directories.insert(directory) && directories.len() < 50000);
        for entry in reader.read_dir(directory, &mut cx).unwrap() {
            let path = format!("{path}/{}", entry.name);
            let metadata = reader.metadata(entry.node, &mut cx).unwrap();
            let times = reader.timestamps(entry.node, &mut cx).unwrap();
            assert!(times.modified.is_some() && times.created.is_some());
            if metadata.kind == FileKind::Directory {
                pending.push((entry.node, path));
                continue;
            }
            if metadata.kind == FileKind::Other {
                continue;
            }
            let mut hash = Sha256::new();
            let mut offset = 0;
            let at = metadata.size.saturating_sub(101);
            let wanted = (metadata.size - at).min(97) as usize;
            let mut expected = Vec::new();
            while offset < metadata.size {
                let want = (metadata.size - offset).min(bytes.len() as u64) as usize;
                assert_eq!(
                    reader
                        .read_at(entry.node, offset, &mut bytes[..want], &mut cx)
                        .unwrap(),
                    want
                );
                hash.update(&bytes[..want]);
                let start = at.max(offset);
                let end = (at + wanted as u64).min(offset + want as u64);
                if start < end {
                    expected.extend_from_slice(
                        &bytes[(start - offset) as usize..(end - offset) as usize],
                    );
                }
                offset += want as u64;
            }
            let mut range = [0; 97];
            let count = reader.read_at(entry.node, at, &mut range, &mut cx).unwrap();
            assert_eq!(count, wanted);
            assert_eq!(&range[..count], expected.as_slice());
            for attribute in reader.attributes(entry.node).unwrap() {
                let mut offset = 0;
                while offset < attribute.size {
                    let want = (attribute.size - offset).min(bytes.len() as u64) as usize;
                    assert_eq!(
                        reader
                            .read_attribute_at(
                                entry.node,
                                &attribute.name,
                                offset,
                                &mut bytes[..want]
                            )
                            .unwrap(),
                        want
                    );
                    offset += want as u64;
                }
                attribute_count += 1;
            }
            let digest = format!("{:x}", hash.finalize());
            if path == "/system/packages/haiku-r1~beta6_hrev60295-1-x86_64.hpkg" {
                assert_eq!(metadata.size, 40_048_778);
                assert_eq!(
                    digest,
                    "f624ff224ac0cc68c22300e15babc3e058b09fc145f0af19f3257c0c5e9c85dc"
                );
                package_verified = true;
            }
            println!("BFS_FILE {path:?} {} {digest}", metadata.size);
            files += 1;
        }
    }
    assert!(files > 10 && directories.len() > 2 && package_verified);
    println!(
        "BFS_AUDIT files={files}, directories={}, attributes={attribute_count}",
        directories.len()
    );
}
