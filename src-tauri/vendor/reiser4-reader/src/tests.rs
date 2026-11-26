use crate::{Header, Reiser4};
use flate2::read::ZlibDecoder;
use lamfold::{
    BlockCache, FileKind, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx,
};
use sha2::{Digest, Sha256};
use std::io::Read;

const NODE40: &[u8] = include_bytes!("../tests/fixtures/node40-large.reiser4z");
const NODE41: &[u8] = include_bytes!("../tests/fixtures/node41-short.reiser4z");
const LZO: &[u8] = include_bytes!("../tests/fixtures/lzo1.reiser4z");
const DEFLATE: &[u8] = include_bytes!("../tests/fixtures/gzip1.reiser4z");
const ZSTD: &[u8] = include_bytes!("../tests/fixtures/zstd1.reiser4z");

fn fixture(data: &[u8]) -> Vec<u8> {
    let mut raw = Vec::new();
    ZlibDecoder::new(data).read_to_end(&mut raw).unwrap();
    raw
}

#[test]
fn native_tree_metadata_and_file_ranges() {
    for (data, protected) in [(NODE40, false), (NODE41, true)] {
        let data = fixture(data);
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let mut fs = Reiser4::open(SliceSource::new(&data), &mut context).unwrap();
        assert_eq!(fs.header().protected_nodes, protected);
        assert_ne!(fs.header().large_keys, protected);
        let root = fs.root();
        let entries = fs.read_dir(root, &mut context).unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.name == "nested" && entry.kind == FileKind::Directory));
        let nested = fs.lookup(root, "nested", &mut context).unwrap().unwrap();
        let tail = fs
            .lookup(nested, "tail.txt", &mut context)
            .unwrap()
            .unwrap();
        assert_eq!(
            fs.lookup(root, "hard-link", &mut context).unwrap(),
            Some(tail)
        );
        let link = fs.lookup(root, "symbolic", &mut context).unwrap().unwrap();
        assert_eq!(
            fs.read_link(link, &mut context).unwrap().as_deref(),
            Some(b"nested/tail.txt".as_slice())
        );
        let file = fs
            .lookup(nested, "native-long-name-for-hash.bin", &mut context)
            .unwrap()
            .unwrap();
        assert_eq!(fs.metadata(file, &mut context).unwrap().size, 11_113);
        assert!(fs.timestamps(file, &mut context).unwrap().changed.is_some());
        let mut reopened = Reiser4::open(SliceSource::new(&data), &mut context).unwrap();
        assert_eq!(reopened.inode_number(file, &mut context).unwrap(), 0x10002);
        let expected: Vec<_> = (0..11_113)
            .map(|n| ((n * 31 + n / 211) % 256) as u8)
            .collect();
        for (offset, length) in [
            (0, 11_113),
            (3103, 271),
            (7000, 491),
            (11_101, 23),
            (11_113, 40),
        ] {
            let mut out = vec![0; length];
            let count = reopened
                .read_at(file, offset as u64, &mut out, &mut context)
                .unwrap();
            assert_eq!(count, length.min(expected.len() - offset));
            assert_eq!(out[..count], expected[offset..offset + count]);
        }
        let sparse = reopened
            .lookup(reopened.root(), "sparse.bin", &mut context)
            .unwrap()
            .unwrap();
        let mut across = [0xcc; 75];
        assert_eq!(
            reopened
                .read_at(sparse, 32768 - 17, &mut across, &mut context)
                .unwrap(),
            across.len()
        );
        assert_eq!(across[..17], [0; 17]);
        assert_eq!(across[17..], expected[..58]);
        let mut complete = vec![0xcc; 32768 + expected.len()];
        assert_eq!(
            reopened
                .read_at(sparse, 0, &mut complete, &mut context)
                .unwrap(),
            complete.len()
        );
        assert!(complete[..32768].iter().all(|byte| *byte == 0));
        assert_eq!(complete[32768..], expected);
    }
}

#[test]
fn kernel_compressed_clusters_match_native_reads() {
    const CLUSTER: usize = 16_384;
    for (data, expected) in [
        (
            LZO,
            "cd45bbcea146424c2affe97c07d381ecdf73aa059d7d9151be3225a2db83d65b",
        ),
        (
            DEFLATE,
            "71db1e58c437c56317c4a3fea3dd30ed6310039f475f71400253d3f3297012f7",
        ),
        (
            ZSTD,
            "36ae9eb7cf5d0fb38f754d0ec28ebe355e5f4e960d689b119e1f82d42a638849",
        ),
    ] {
        let data = fixture(data);
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let mut fs = Reiser4::open(SliceSource::new(&data), &mut context).unwrap();
        let root = fs.root();
        let nested = fs.lookup(root, "nested", &mut context).unwrap().unwrap();
        let file = fs
            .lookup(nested, "payload.bin", &mut context)
            .unwrap()
            .unwrap();
        let mut full = vec![0; 2 * CLUSTER + 1217];
        assert_eq!(
            fs.read_at(file, 0, &mut full, &mut context).unwrap(),
            full.len()
        );
        assert_eq!(hex::encode(Sha256::digest(&full)), expected);
        for offset in [CLUSTER - 31, CLUSTER * 2 - 31, full.len() - 14] {
            let mut range = [0xcc; 63];
            let count = fs
                .read_at(file, offset as u64, &mut range, &mut context)
                .unwrap();
            assert_eq!(count, range.len().min(full.len() - offset));
            assert_eq!(range[..count], full[offset..offset + count]);
            assert!(range[count..].iter().all(|&byte| byte == 0xcc));
        }
        let sparse = fs
            .lookup(root, "sparse.bin", &mut context)
            .unwrap()
            .unwrap();
        let mut hole = [0xcc; 48];
        let start = 5 * CLUSTER - 20;
        assert_eq!(
            fs.read_at(sparse, start as u64, &mut hole, &mut context)
                .unwrap(),
            hole.len()
        );
        assert_eq!(hole[..20], [0; 20]);
        assert_eq!(&hole[20..], b"last bytes of a sparse file\n");
        let mut full = vec![0; 5 * CLUSTER + 28];
        assert_eq!(
            fs.read_at(sparse, 0, &mut full, &mut context).unwrap(),
            full.len()
        );
        assert_eq!(
            hex::encode(Sha256::digest(&full)),
            "e9d3aa0b1f38fc1314e1de780abdc0832afb156291833a658a82be0e4063496e"
        );
    }
}

#[test]
fn refuses_damaged_cluster_payload_and_fragment_shift() {
    let mut data = fixture(LZO);
    // The first payload.bin ctail item starts at byte 925 of node 24.
    let first_cluster = 24 * 4096 + 925;
    for (at, message) in [
        (first_cluster + 1, "reiser4: cluster checksum"),
        (first_cluster, "reiser4: invalid cluster fragment"),
    ] {
        data[at] ^= 1;
        {
            let mut cache = BlockCache::new(0);
            let mut context = SubstrateCtx {
                cache: &mut cache,
                verifier: &NoVerifier,
            };
            let mut fs = Reiser4::open(SliceSource::new(&data), &mut context).unwrap();
            let nested = fs
                .lookup(fs.root(), "nested", &mut context)
                .unwrap()
                .unwrap();
            let file = fs
                .lookup(nested, "payload.bin", &mut context)
                .unwrap()
                .unwrap();
            let mut output = [0xcc; 20];
            assert!(matches!(
                fs.read_at(file, 0, &mut output, &mut context),
                Err(FoldError::Corrupt(reason)) if reason == message
            ));
            assert_eq!(output, [0xcc; 20]);
        }
        data[at] ^= 1;
    }
}

#[test]
fn rejects_truncation_pending_journal_and_invalid_crc() {
    let mut data = fixture(NODE41);
    let header = Header::read(&mut SliceSource::new(&data)).unwrap();
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    assert!(Reiser4::open(SliceSource::new(&data[..data.len() - 4096]), &mut context).is_err());
    data[19 * 4096] = 24;
    assert!(matches!(
        Reiser4::open(SliceSource::new(&data), &mut context),
        Err(FoldError::Unsupported(_))
    ));
    data[19 * 4096] = 0;
    data[header.root as usize * 4096 + 28] ^= 1;
    assert!(matches!(
        Reiser4::open(SliceSource::new(&data), &mut context),
        Err(FoldError::Corrupt("reiser4: node checksum"))
    ));
}
