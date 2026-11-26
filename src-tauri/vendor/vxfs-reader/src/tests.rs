use std::io::Read;

use flate2::read::ZlibDecoder;
use lamfold::{BlockCache, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};

use crate::{Endian, Vxfs};

const LE2: &[u8] = include_bytes!("../tests/fixtures/le2.zlib");
const LE3: &[u8] = include_bytes!("../tests/fixtures/le3.zlib");
const BE4: &[u8] = include_bytes!("../tests/fixtures/be4.zlib");

fn decompress(compressed: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    ZlibDecoder::new(compressed)
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[test]
fn both_endians_bootstrap_linked_olts_and_read_exact_file_bytes() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    for (image, version, endian, block_size) in [
        (LE2, 2, Endian::Little, 1024),
        (LE3, 3, Endian::Little, 2048),
        (BE4, 4, Endian::Big, 8192),
    ] {
        let image = decompress(image);
        let mut fs = Vxfs::open(SliceSource::new(&image), &mut cx).unwrap();
        assert_eq!(
            (
                fs.header().layout,
                fs.header().endian,
                fs.header().block_size
            ),
            (version, endian, block_size)
        );
        assert!(fs.header().clean);
        let root = fs.root();
        let entries = fs.read_dir(root, &mut cx).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["tiny", "dir", "link", "linear", "empty"]
        );
        let tiny = fs.lookup(root, "tiny", &mut cx).unwrap().unwrap();
        let link = fs.lookup(root, "link", &mut cx).unwrap().unwrap();
        assert_eq!(fs.read_link(link, &mut cx).unwrap().unwrap(), b"tiny");
        let times = fs.timestamps(tiny, &mut cx).unwrap();
        assert_eq!(times.modified, Some((1712300002, 321000)));
        assert_eq!(times.accessed, Some((1712300001, 12000)));
        assert_eq!(times.changed, Some((1712300003, 9876000)));
        assert_eq!(times.created, None);

        let dir = fs.lookup(root, "dir", &mut cx).unwrap().unwrap();
        assert_eq!(fs.read_dir(dir, &mut cx).unwrap().len(), 2);
        assert_eq!(fs.lookup(dir, "alias", &mut cx).unwrap(), Some(tiny));
        let large = fs.lookup(dir, "large", &mut cx).unwrap().unwrap();
        let marker_offset = (5 << 30) + block_size;
        assert_eq!(
            fs.metadata(large, &mut cx).unwrap().size,
            marker_offset + block_size
        );
        let mut bytes = [0; 10];
        assert_eq!(
            fs.read_at(large, marker_offset - 2, &mut bytes, &mut cx)
                .unwrap(),
            10
        );
        assert_eq!(bytes, [0, 0, 0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(
            fs.read_at(large, marker_offset + block_size - 3, &mut bytes, &mut cx)
                .unwrap(),
            3
        );
        assert_eq!(&bytes[..3], &[253, 254, 255]);

        let linear = fs.lookup(root, "linear", &mut cx).unwrap().unwrap();
        fs.read_at(linear, block_size - 3, &mut bytes, &mut cx)
            .unwrap();
        assert_eq!(&bytes[..3], b"DDD");
        assert_eq!(&bytes[3..], &[0; 7]);
        fs.read_at(linear, block_size * 2 - 3, &mut bytes, &mut cx)
            .unwrap();
        assert_eq!(&bytes[..3], &[0; 3]);
        assert_eq!(&bytes[3..], b"IIIIIII");
    }
}

#[test]
fn rejects_broken_metadata_and_never_treats_allocated_ranges_as_holes() {
    const BLOCK: usize = 1024;
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };

    let mut cycle = decompress(LE2);
    cycle[21 * BLOCK + 40..21 * BLOCK + 44].copy_from_slice(&20u32.to_le_bytes());
    cycle[21 * BLOCK + 48..21 * BLOCK + 52].copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(
        Vxfs::open(SliceSource::new(&cycle), &mut cx),
        Err(FoldError::Corrupt(_))
    ));

    let mut unsupported = decompress(LE2);
    unsupported[BLOCK + 4..BLOCK + 8].copy_from_slice(&5u32.to_le_bytes());
    assert!(Vxfs::probe(&mut SliceSource::new(&unsupported)).unwrap());
    assert!(matches!(
        Vxfs::open(SliceSource::new(&unsupported), &mut cx),
        Err(FoldError::Unsupported(_))
    ));

    let mut broken = decompress(LE2);
    broken[39 * BLOCK + 8..39 * BLOCK + 12].copy_from_slice(&96u32.to_le_bytes());
    let mut fs = Vxfs::open(SliceSource::new(&broken), &mut cx).unwrap();
    let mut bytes = [1; 4];
    assert!(matches!(
        fs.read_at(8, (5 << 30) + BLOCK as u64, &mut bytes, &mut cx),
        Err(FoldError::Corrupt(_))
    ));
    assert_eq!(bytes, [1; 4]);

    let mut broken = decompress(LE2);
    broken[34 * BLOCK + 12..34 * BLOCK + 14].copy_from_slice(&1024u16.to_le_bytes());
    let mut fs = Vxfs::open(SliceSource::new(&broken), &mut cx).unwrap();
    assert!(matches!(
        fs.read_dir(4, &mut cx),
        Err(FoldError::Corrupt(_))
    ));
}
