use std::io::Read;

use flate2::read::ZlibDecoder;
use lamfold::{BlockCache, FoldError, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};

use crate::{ByteOrder, ROOT, Signature, Spiffs};

const ESP32: &[u8] = include_bytes!("../tests/fixtures/esp32-default.zlib");
const NO_MAGIC: &[u8] = include_bytes!("../tests/fixtures/esp32-no-magic.zlib");
const BIG_ENDIAN: &[u8] = include_bytes!("../tests/fixtures/be-aligned.zlib");
const CONSTANT: &[u8] = include_bytes!("../tests/fixtures/constant.zlib");
const NATIVE: &[u8] = include_bytes!("../tests/fixtures/esp32-native.zlib");

fn image(zlib: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    ZlibDecoder::new(zlib).read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn recognizes_independently_written_images_and_validates_all_spans() {
    let mut cache = BlockCache::new(0);
    let mut ctx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let expected: Vec<_> = (0..49281).map(|n| (n * 23 + n / 61) as u8).collect();
    for (fixture, block, page, order, signature) in [
        (ESP32, 4096, 256, ByteOrder::Little, Signature::VolumeLength),
        (NO_MAGIC, 4096, 256, ByteOrder::Little, Signature::None),
        (
            BIG_ENDIAN,
            8192,
            512,
            ByteOrder::Big,
            Signature::VolumeLength,
        ),
        (CONSTANT, 4096, 256, ByteOrder::Little, Signature::Constant),
        (NATIVE, 4096, 256, ByteOrder::Little, Signature::VolumeLength),
    ] {
        let bytes = image(fixture);
        let mut fs = Spiffs::open(SliceSource::new(&bytes), &mut ctx).unwrap();
        let layout = fs.geometry;
        assert_eq!(
            (
                layout.block_bytes,
                layout.page_bytes,
                layout.order,
                layout.signature
            ),
            (block, page, order, signature)
        );
        assert_eq!(
            fs.read_dir(ROOT, &mut ctx)
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect::<Vec<_>>(),
            ["%2Fempty", "%2Fhello.txt", "%2Flogs%2Fevent.bin"]
        );
        assert!(fs.lookup(ROOT, "%2FHello.txt", &mut ctx).unwrap().is_none());
        let nested = fs
            .lookup(ROOT, "%2Flogs%2Fevent.bin", &mut ctx)
            .unwrap()
            .unwrap();
        let empty = fs.lookup(ROOT, "%2Fempty", &mut ctx).unwrap().unwrap();
        assert_eq!(fs.metadata(empty, &mut ctx).unwrap().size, 0);
        assert_eq!(fs.read_at(empty, 0, &mut [0; 8], &mut ctx).unwrap(), 0);
        let mut out = vec![0; expected.len() + 21];
        assert_eq!(
            fs.read_at(nested, 0, &mut out, &mut ctx).unwrap(),
            expected.len()
        );
        assert_eq!(&out[..expected.len()], expected);
        assert_eq!(
            fs.read_at(nested, 26301, &mut out[..13087], &mut ctx)
                .unwrap(),
            13087
        );
        assert_eq!(&out[..13087], &expected[26301..39388]);
    }
}

#[test]
fn missing_one_block_signature_is_allowed_and_index_corruption_is_explicit() {
    let mut cache = BlockCache::new(0);
    let mut ctx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut bytes = image(ESP32);
    bytes[252..254].fill(0xff);
    let header = Spiffs::detect(&mut SliceSource::new(&bytes))
        .unwrap()
        .unwrap();
    assert_eq!(header.signature, Signature::VolumeLength);
    // Point a continuation index at a page with the wrong object and span.
    bytes[115 * 256 + 8..115 * 256 + 10].copy_from_slice(&3u16.to_le_bytes());
    let mut fs = Spiffs::open(SliceSource::new(&bytes), &mut ctx).unwrap();
    let large = fs
        .lookup(ROOT, "%2Flogs%2Fevent.bin", &mut ctx)
        .unwrap()
        .unwrap();
    let small = fs.lookup(ROOT, "%2Fhello.txt", &mut ctx).unwrap().unwrap();
    assert!(matches!(
        fs.read_at(large, 0, &mut [0; 16], &mut ctx),
        Err(FoldError::Corrupt(_))
    ));
    let mut out = [0; 19];
    assert_eq!(fs.read_at(small, 0, &mut out, &mut ctx).unwrap(), 19);
    assert_eq!(&out, b"Embedded evidence.\n");
    assert!(!Spiffs::probe(&mut SliceSource::new(&vec![0xff; bytes.len()])).unwrap());
}
