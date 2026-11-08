use crate::{directory::Directory, extents::Stream, inode::Inode, Header};
use lamfold::{BlockSource, FoldError};
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
    fn read_at(&mut self, offset: u64, raw: &mut [u8]) -> lamfold::Result<()> {
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| FoldError::Io("native source seek"))?;
        self.file
            .read_exact(raw)
            .map_err(|_| FoldError::Io("native source read"))
    }
}
#[test]
#[ignore = "set OCFS2_ORACLE_DIR to native mkfs/kernel/fsck image directory"]
fn native_header_inodes_and_directory_data() {
    let directory = std::env::var("OCFS2_ORACLE_DIR").unwrap();
    for (block, cluster) in [(512, 4096), (1024, 4096), (2048, 16384), (4096, 65536)] {
        let name = format!("ocfs2-{block}-{cluster}");
        let file = File::open(Path::new(&directory).join(format!("{name}.img"))).unwrap();
        let length = file.metadata().unwrap().len();
        let mut source = Source { file, length };
        let header =
            Header::inspect(&mut source).unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(header.block_size, block);
        assert_eq!(header.cluster_size, cluster);
        let root = Inode::read(&mut source, &header, header.root).unwrap();
        let stream = Stream::inode(&mut source, &header, &root).unwrap();
        let entries = Directory::read(&mut source, &header, &root, &stream).unwrap();
        assert!(entries.entries.iter().any(|entry| entry.name == "hello"));
        for entry in &entries.entries {
            let inode = Inode::read(&mut source, &header, entry.node).unwrap();
            if let Some(kind) = entry.kind {
                assert_eq!(kind, inode.kind());
            }
        }
        eprintln!(
            "OCFS2_NATIVE_PREFIX {name} {} root entries",
            entries.entries.len()
        );
    }
}
