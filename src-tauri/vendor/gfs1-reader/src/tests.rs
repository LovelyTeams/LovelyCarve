use std::{collections::BTreeMap, io::Read};

use flate2::read::ZlibDecoder;
use lamfold::{
    BlockCache, BlockSource, FileKind, FoldError, FoldFrontend, NoVerifier, Result, SubstrateCtx,
};

use crate::{Gfs1, Header};

const NATIVE_512: &[u8] = include_bytes!("../tests/fixtures/gfs1-512.gfs1z");
const NATIVE_4096: &[u8] = include_bytes!("../tests/fixtures/gfs1-4096.gfs1z");
const PACKED_BLOCK: u64 = 65536;

struct Image {
    length: u64,
    blocks: BTreeMap<u64, Vec<u8>>,
}

impl Image {
    fn open(packed: &[u8]) -> Self {
        let mut archive = Vec::new();
        ZlibDecoder::new(packed).read_to_end(&mut archive).unwrap();
        let length = u64::from_le_bytes(archive[..8].try_into().unwrap());
        let mut blocks = BTreeMap::new();
        let mut raw = &archive[8..];
        while !raw.is_empty() {
            let offset = u64::from_le_bytes(raw[..8].try_into().unwrap());
            let count = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
            assert!(
                offset.is_multiple_of(PACKED_BLOCK) && (1..=PACKED_BLOCK as usize).contains(&count)
            );
            assert!(offset + count as u64 <= length);
            let mut block = vec![0; PACKED_BLOCK as usize];
            block[..count].copy_from_slice(&raw[12..12 + count]);
            assert!(blocks.insert(offset, block).is_none());
            raw = &raw[12 + count..];
        }
        Self { length, blocks }
    }

    fn patch(&mut self, offset: u64, bytes: &[u8]) {
        let start = (offset % PACKED_BLOCK) as usize;
        self.blocks.get_mut(&(offset - start as u64)).unwrap()[start..start + bytes.len()]
            .copy_from_slice(bytes);
    }
}

impl BlockSource for Image {
    fn len(&self) -> u64 {
        self.length
    }

    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<()> {
        assert!(offset + out.len() as u64 <= self.length);
        let mut done = 0;
        while done < out.len() {
            let at = offset + done as u64;
            let within = (at % PACKED_BLOCK) as usize;
            let count = (PACKED_BLOCK as usize - within).min(out.len() - done);
            let target = &mut out[done..done + count];
            if let Some(block) = self.blocks.get(&(at - within as u64)) {
                target.copy_from_slice(&block[within..within + count]);
            } else {
                target.fill(0);
            }
            done += count;
        }
        Ok(())
    }
}

#[test]
fn native_versions_have_linear_and_hashed_directories_and_full_sparse_bytes() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    for (fixture, block, index) in [(NATIVE_512, 512, 42), (NATIVE_4096, 4096, 520)] {
        let mut image = Image::open(fixture);
        assert_eq!(
            Header::inspect(&mut image).unwrap().block_size,
            block as u32
        );
        let mut fs = Gfs1::open(image, &mut cx).unwrap();
        let root = fs.root();
        let data = fs.lookup(root, "data", &mut cx).unwrap().unwrap();
        let names = fs
            .read_dir(data, &mut cx)
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["empty", "latest", "payload.bin", "same-inode"]);
        let link = fs.lookup(data, "latest", &mut cx).unwrap().unwrap();
        assert_eq!(fs.metadata(link, &mut cx).unwrap().kind, FileKind::Symlink);
        assert_eq!(
            fs.read_link(link, &mut cx).unwrap().unwrap(),
            b"payload.bin"
        );
        let file = fs.lookup(data, "payload.bin", &mut cx).unwrap().unwrap();
        assert_eq!(fs.lookup(data, "same-inode", &mut cx).unwrap(), Some(file));
        assert_eq!(
            fs.timestamps(file, &mut cx).unwrap().modified,
            Some((1700000000, 0))
        );
        let mut expected: Vec<_> = (0..block).map(|i| (i * 11 + i / 43) as u8).collect();
        expected.resize(index * block + 7, 0);
        expected.extend_from_slice(b"GFS1 sparse tail");
        let mut payload = vec![0; expected.len() + 16];
        assert_eq!(
            fs.read_at(file, 0, &mut payload, &mut cx).unwrap(),
            expected.len()
        );
        assert_eq!(&payload[..expected.len()], expected);
        assert_eq!(
            fs.read_at(file, (block - 11) as u64, &mut payload[..73], &mut cx)
                .unwrap(),
            73
        );
        assert_eq!(&payload[..73], &expected[block - 11..block + 62]);
    }
}

#[test]
fn dirty_latest_transaction_wins_over_older_clean_headers() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut image = Image::open(NATIVE_4096);
    let start = 20480_u64;
    let (_, latest) = (0..512)
        .map(|i| {
            let offset = (start + 16 * i) * 4096;
            let mut raw = [0; 48];
            image.read_at(offset, &mut raw).unwrap();
            (u64::from_be_bytes(raw[40..48].try_into().unwrap()), offset)
        })
        .max()
        .unwrap();
    image.patch(latest + 24, &[0; 4]);
    image.patch(latest + 384 + 24, &[0; 4]);
    assert!(matches!(
        Gfs1::open(image, &mut cx),
        Err(FoldError::Unsupported("GFS1 journal requires replay"))
    ));
}
