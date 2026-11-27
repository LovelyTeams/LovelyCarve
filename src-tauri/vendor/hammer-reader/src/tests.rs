use std::{collections::BTreeMap, io::Read};

use flate2::read::ZlibDecoder;
use lamfold::{
    BlockCache, BlockSource, FileKind, FoldError, FoldFrontend, NoVerifier, SubstrateCtx,
};

use crate::{Hammer, ROOT};

const EMPTY: &[u8] = include_bytes!("../tests/fixtures/hammer1-v7-empty.h1z");
const LEGACY: &[u8] = include_bytes!("../tests/fixtures/hammer1-v1-content.h1z");
const CURRENT: &[u8] = include_bytes!("../tests/fixtures/hammer1-v7-content.h1z");
const PACKED_BLOCK: usize = 65536;

struct Sparse {
    length: u64,
    blocks: BTreeMap<u64, Vec<u8>>,
}

impl Sparse {
    fn open(compressed: &[u8]) -> Self {
        let mut archive = Vec::new();
        ZlibDecoder::new(compressed)
            .read_to_end(&mut archive)
            .unwrap();
        let length = u64::from_le_bytes(archive[..8].try_into().unwrap());
        let mut blocks = BTreeMap::new();
        let mut input = &archive[8..];
        while !input.is_empty() {
            let start = u64::from_le_bytes(input[..8].try_into().unwrap());
            let count = u32::from_le_bytes(input[8..12].try_into().unwrap()) as usize;
            assert!(
                start.is_multiple_of(PACKED_BLOCK as u64) && (1..=PACKED_BLOCK).contains(&count)
            );
            assert!(start + count as u64 <= length);
            let mut block = vec![0; PACKED_BLOCK];
            block[..count].copy_from_slice(&input[12..12 + count]);
            assert!(blocks.insert(start, block).is_none());
            input = &input[12 + count..];
        }
        Self { length, blocks }
    }

    fn change(&mut self, at: u64) {
        let within = (at % PACKED_BLOCK as u64) as usize;
        self.blocks.get_mut(&(at - within as u64)).unwrap()[within] ^= 0x80;
    }

    fn patch(&mut self, at: u64, bytes: &[u8]) {
        let within = (at % PACKED_BLOCK as u64) as usize;
        self.blocks.get_mut(&(at - within as u64)).unwrap()[within..within + bytes.len()]
            .copy_from_slice(bytes);
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
            return Err(FoldError::Io("short HAMMER fixture read"));
        }
        let mut done = 0;
        while done < out.len() {
            let at = offset + done as u64;
            let within = (at % PACKED_BLOCK as u64) as usize;
            let count = (PACKED_BLOCK - within).min(out.len() - done);
            let output = &mut out[done..done + count];
            if let Some(block) = self.blocks.get(&(at - within as u64)) {
                output.copy_from_slice(&block[within..within + count]);
            } else {
                output.fill(0);
            }
            done += count;
        }
        Ok(())
    }
}

fn body(multiplier: usize, divisor: usize) -> Vec<u8> {
    let mut bytes: Vec<_> = (0..16384)
        .map(|i| (i * multiplier + i / divisor) as u8)
        .collect();
    bytes.resize(3 * 16384, 0);
    bytes.extend_from_slice(b"HAMMER sparse tail");
    bytes
}

#[test]
fn opens_original_empty_volume() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut source = Sparse::open(EMPTY);
    let header = Hammer::header(&mut source).unwrap().unwrap();
    assert_eq!(header.label, "LC_H1");
    let mut format = Hammer::open(source, &mut cx).unwrap();
    assert!(format.read_dir(ROOT, &mut cx).unwrap().is_empty());
}

#[test]
fn opens_v1_v7_and_distinguishes_live_and_published_snapshot_data() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let expected = body(17, 47);
    for (fixture, version) in [(LEGACY, 1), (CURRENT, 7)] {
        let mut source = Sparse::open(fixture);
        assert_eq!(
            Hammer::header(&mut source).unwrap().unwrap().version,
            version
        );
        let mut fs = Hammer::open(source, &mut cx).unwrap();
        let file = fs.lookup(ROOT, "payload.bin", &mut cx).unwrap().unwrap();
        assert_eq!(fs.lookup(ROOT, "same-inode", &mut cx).unwrap(), Some(file));
        assert!(fs.lookup(ROOT, "retired", &mut cx).unwrap().is_none());
        let link = fs.lookup(ROOT, "latest", &mut cx).unwrap().unwrap();
        assert_eq!(fs.metadata(link, &mut cx).unwrap().kind, FileKind::Symlink);
        assert_eq!(
            fs.read_link(link, &mut cx).unwrap().unwrap(),
            b"payload.bin"
        );
        assert_eq!(fs.inode_number(file, &mut cx).unwrap(), 2);
        assert_eq!(
            fs.timestamps(file, &mut cx).unwrap().modified,
            Some((1700000000, 123456000))
        );
        let mut out = vec![0; expected.len() + 16];
        assert_eq!(
            fs.read_at(file, 0, &mut out, &mut cx).unwrap(),
            expected.len()
        );
        assert_eq!(&out[..expected.len()], expected);
        assert_eq!(
            fs.read_at(file, 16000, &mut out[..1537], &mut cx).unwrap(),
            1537
        );
        assert_eq!(&out[..1537], &expected[16000..17537]);

        let snapshots = fs.lookup(ROOT, "@snapshots", &mut cx).unwrap();
        if version == 1 {
            assert!(snapshots.is_none());
            continue;
        }
        let snapshot = fs
            .lookup(snapshots.unwrap(), "0000000100000001", &mut cx)
            .unwrap()
            .unwrap();
        let archived = fs
            .lookup(snapshot, "payload.bin", &mut cx)
            .unwrap()
            .unwrap();
        assert_ne!(file, archived);
        assert_eq!(
            fs.lookup(snapshot, "retired", &mut cx).unwrap(),
            Some(archived)
        );
        assert_eq!(
            fs.read_at(archived, 0, &mut out, &mut cx).unwrap(),
            expected.len()
        );
        assert_eq!(&out[..expected.len()], body(49, 71));
    }
}

#[test]
fn corrupt_header_and_btree_fail_instead_of_exposing_stale_records() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut corrupt_header = Sparse::open(EMPTY);
    let bytes = &corrupt_header.blocks[&0][..super::PROBE_BYTES];
    let checksum = crc32c::crc32c(&bytes[..156]) ^ crc32c::crc32c(&bytes[160..]);
    corrupt_header.patch(156, &checksum.to_le_bytes());
    assert!(
        Hammer::header(&mut corrupt_header)
            .unwrap()
            .unwrap()
            .checksum_present
    );
    corrupt_header.change(90);
    assert!(matches!(
        Hammer::header(&mut corrupt_header),
        Err(FoldError::Corrupt(_))
    ));
    let mut corrupt_tree = Sparse::open(CURRENT);
    let header = Hammer::header(&mut corrupt_tree).unwrap().unwrap();
    let root = header.physical(header.root, 4096, &[8], 4096).unwrap();
    corrupt_tree.change(root + 150);
    assert!(matches!(
        Hammer::open(corrupt_tree, &mut cx),
        Err(FoldError::Corrupt(_))
    ));
}

#[test]
fn refuses_active_undo_even_when_header_indexes_match() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut source = Sparse::open(CURRENT);
    let header = Hammer::header(&mut source).unwrap().unwrap();
    let raw = header.order.u64(&source.blocks[&0][904..]);
    let at = header.physical(raw, 512, &[2], 512).unwrap();
    let mut record = [0; 512];
    source.read_at(at, &mut record).unwrap();
    // The preceding native dummy at the end of the ring has sequence 32767.
    record[8..12].copy_from_slice(&32768_u32.to_le_bytes());
    let checksum = crc32c::crc32c(&record[..12]) ^ crc32c::crc32c(&record[16..]);
    record[12..16].copy_from_slice(&checksum.to_le_bytes());
    source.patch(at, &record);
    assert!(matches!(
        Hammer::open(source, &mut cx),
        Err(FoldError::Unsupported(_))
    ));
}
