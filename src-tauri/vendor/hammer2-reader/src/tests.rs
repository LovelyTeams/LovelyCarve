use std::{collections::BTreeMap, io::Read};

use flate2::read::ZlibDecoder;
use lamfold::{
    BlockCache, BlockSource, FileKind, FoldError, FoldFrontend, NoVerifier, SubstrateCtx,
};

use crate::{BLOCK_BYTES, Hammer2, ROOT, ZONE_BYTES};

const LZ4: &[u8] = include_bytes!("../tests/fixtures/lz4.h2z");
const ZLIB: &[u8] = include_bytes!("../tests/fixtures/zlib.h2z");
const MIRROR: &[u8] = include_bytes!("../tests/fixtures/mirror.h2z");
const LEGACY: &[u8] = include_bytes!("../tests/fixtures/v1.h2z");
const SNAPSHOT: &[u8] = include_bytes!("../tests/fixtures/snapshot.h2z");

struct Sparse {
    length: u64,
    blocks: BTreeMap<u64, Vec<u8>>,
}

impl Sparse {
    fn load(compressed: &[u8]) -> Self {
        let mut bytes = Vec::new();
        ZlibDecoder::new(compressed)
            .read_to_end(&mut bytes)
            .unwrap();
        let length = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let mut rest = &bytes[8..];
        let mut blocks = BTreeMap::new();
        while !rest.is_empty() {
            let at = u64::from_le_bytes(rest[..8].try_into().unwrap());
            let count = u32::from_le_bytes(rest[8..12].try_into().unwrap()) as usize;
            assert_eq!(count, BLOCK_BYTES);
            assert_eq!(at % BLOCK_BYTES as u64, 0);
            blocks.insert(at, rest[12..12 + count].to_vec());
            rest = &rest[12 + count..];
        }
        Self { length, blocks }
    }
}

impl BlockSource for Sparse {
    fn len(&self) -> u64 {
        self.length
    }

    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> lamfold::Result<()> {
        if offset
            .checked_add(out.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(FoldError::Io("fixture read exceeds source"));
        }
        let mut done = 0;
        while done < out.len() {
            let at = offset + done as u64;
            let within = at as usize % BLOCK_BYTES;
            let length = (BLOCK_BYTES - within).min(out.len() - done);
            let target = &mut out[done..done + length];
            if let Some(block) = self.blocks.get(&(at - within as u64)) {
                target.copy_from_slice(&block[within..within + length]);
            } else {
                target.fill(0);
            }
            done += length;
        }
        Ok(())
    }
}

#[test]
fn reads_native_pfs_directories_hardlinks_sparse_and_compressed_blocks() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let expected: Vec<_> = (0..196613).map(|i| (i * 17 + i / 51) as u8).collect();
    for fixture in [LZ4, ZLIB, MIRROR] {
        let mut fs = Hammer2::open(Sparse::load(fixture), &mut cx).unwrap();
        let roots = fs.read_dir(ROOT, &mut cx).unwrap();
        assert_eq!(
            roots
                .iter()
                .map(|pfs| pfs.name.as_str())
                .collect::<Vec<_>>(),
            ["DATA", "LOCAL"]
        );
        let data = fs.lookup(ROOT, "DATA", &mut cx).unwrap().unwrap();
        let hello = fs.lookup(data, "hello", &mut cx).unwrap().unwrap();
        let same = fs.lookup(data, "same-inode", &mut cx).unwrap().unwrap();
        assert_eq!(hello, same);
        assert_eq!(fs.inode_number(hello, &mut cx).unwrap(), 0x400);
        let link = fs.lookup(data, "latest", &mut cx).unwrap().unwrap();
        assert_eq!(fs.metadata(link, &mut cx).unwrap().kind, FileKind::Symlink);
        assert_eq!(
            fs.read_link(link, &mut cx).unwrap().as_deref(),
            Some(&b"var/log/event.bin"[..])
        );
        let var = fs.lookup(data, "var", &mut cx).unwrap().unwrap();
        let logs = fs.lookup(var, "log", &mut cx).unwrap().unwrap();
        let event = fs.lookup(logs, "event.bin", &mut cx).unwrap().unwrap();
        assert_eq!(
            fs.timestamps(event, &mut cx).unwrap().modified,
            Some((1700000000, 0))
        );
        let mut out = vec![0; 196615];
        assert_eq!(fs.read_at(event, 0, &mut out, &mut cx).unwrap(), 196613);
        assert_eq!(&out[..expected.len()], expected);
        assert_eq!(
            fs.read_at(event, 65381, &mut out[..12923], &mut cx)
                .unwrap(),
            12923
        );
        assert_eq!(&out[..12923], &expected[65381..78304]);
        let sparse = fs.lookup(data, "sparse.bin", &mut cx).unwrap().unwrap();
        let expected_size = 5 * BLOCK_BYTES as u64 + 93 + 9;
        assert_eq!(fs.metadata(sparse, &mut cx).unwrap().size, expected_size);
        let mut tail = [0xff; 150];
        assert_eq!(
            fs.read_at(sparse, 5 * BLOCK_BYTES as u64 - 31, &mut tail, &mut cx)
                .unwrap(),
            133
        );
        assert_eq!(&tail[..124], &[0; 124]);
        assert_eq!(&tail[124..133], b"hole-tail");
    }
}

#[test]
fn chooses_current_valid_volume_header_and_reports_damaged_data() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut image = Sparse::load(MIRROR);
    let header = Hammer2::header(&mut image).unwrap().unwrap();
    assert_eq!(header.offset, ZONE_BYTES);
    image.blocks.get_mut(&0).unwrap()[100] ^= 1;
    let header = Hammer2::header(&mut image).unwrap().unwrap();
    assert_eq!(header.offset, ZONE_BYTES);
    image.blocks.get_mut(&ZONE_BYTES).unwrap()[100] ^= 1;
    assert!(matches!(
        Hammer2::open(image, &mut cx),
        Err(FoldError::Corrupt(_))
    ));

    let mut image = Sparse::load(LZ4);
    image.blocks.get_mut(&0x8c00000).unwrap()[0x410] ^= 1;
    let mut fs = Hammer2::open(image, &mut cx).unwrap();
    let data = fs.lookup(ROOT, "DATA", &mut cx).unwrap().unwrap();
    let var = fs.lookup(data, "var", &mut cx).unwrap().unwrap();
    let log = fs.lookup(var, "log", &mut cx).unwrap().unwrap();
    let event = fs.lookup(log, "event.bin", &mut cx).unwrap().unwrap();
    assert!(matches!(
        fs.read_at(event, 0, &mut [0; 100], &mut cx),
        Err(FoldError::Corrupt(_))
    ));
}

#[test]
fn reads_version_one_and_names_stored_outside_directory_records() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut image = Sparse::load(LEGACY);
    assert_eq!(Hammer2::header(&mut image).unwrap().unwrap().version, 1);
    let mut fs = Hammer2::open(image, &mut cx).unwrap();
    let data = fs.lookup(ROOT, "DATA", &mut cx).unwrap().unwrap();
    let var = fs.lookup(data, "var", &mut cx).unwrap().unwrap();
    let log = fs.lookup(var, "log", &mut cx).unwrap().unwrap();
    let long = format!("long-{}.txt", "d".repeat(85));
    let node = fs.lookup(log, &long, &mut cx).unwrap().unwrap();
    let mut out = [0; 25];
    assert_eq!(fs.read_at(node, 0, &mut out, &mut cx).unwrap(), 25);
    assert_eq!(&out, b"External directory name.\n");
}

#[test]
fn published_snapshot_has_a_distinct_pfs_inode_namespace() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut fs = Hammer2::open(Sparse::load(SNAPSHOT), &mut cx).unwrap();
    let data = fs.lookup(ROOT, "DATA", &mut cx).unwrap().unwrap();
    let snapshot = fs.lookup(ROOT, "ARCHIVE", &mut cx).unwrap().unwrap();
    assert_ne!(data, snapshot);
    let hello = fs.lookup(data, "hello", &mut cx).unwrap().unwrap();
    let archived = fs.lookup(snapshot, "hello", &mut cx).unwrap().unwrap();
    assert_ne!(hello, archived);
    assert_eq!(
        fs.inode_number(hello, &mut cx).unwrap(),
        fs.inode_number(archived, &mut cx).unwrap()
    );
    let absolute = fs
        .lookup(snapshot, "absolute-link", &mut cx)
        .unwrap()
        .unwrap();
    assert_eq!(
        fs.read_link(absolute, &mut cx).unwrap().as_deref(),
        Some(&b"/var/log/event.bin"[..])
    );
}

#[test]
fn accepts_dragonfly_sha192_check_and_checks_content() {
    let mut checksum = [0; 64];
    checksum[..24].copy_from_slice(&[
        0x5f, 0x70, 0xbf, 0x18, 0xa0, 0x86, 0x00, 0x70, 0x16, 0xe9, 0x48, 0xb0, 0x4a, 0xed, 0x3b,
        0x82, 0xdd, 0xe5, 0x99, 0xae, 0x08, 0xf4, 0x93, 0x59,
    ]);
    let reference = crate::BlockRef {
        kind: crate::Kind::Data,
        methods: 0x40,
        key: 0,
        key_bits: 10,
        data_offset: 1024 | 10,
        embed: [0; 16],
        check: checksum,
    };
    let source = Sparse {
        length: BLOCK_BYTES as u64,
        blocks: BTreeMap::new(),
    };
    let mut volume = crate::Volume::new(source, crate::Order::Little, BLOCK_BYTES as u64);
    let mut bytes = [0xff; 32];
    volume
        .read_file(std::slice::from_ref(&reference), 1024, 0, &mut bytes)
        .unwrap();
    assert_eq!(bytes, [0; 32]);
    let mut bad = reference;
    bad.check[16] ^= 1;
    assert!(matches!(
        volume.read_file(&[bad], 1024, 0, &mut bytes),
        Err(FoldError::Corrupt(_))
    ));
}
