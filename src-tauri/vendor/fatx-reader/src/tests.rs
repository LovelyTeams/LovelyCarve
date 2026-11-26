use std::io::Read;

use flate2::read::ZlibDecoder;
use lamfold::{BlockCache, BlockSource, FoldError, FoldFrontend, NoVerifier, Result, SubstrateCtx};

use crate::Fatx;

const XBOX16: &[u8] = include_bytes!("../tests/fixtures/xbox-fat16.zlib");
const XBOX32: &[u8] = include_bytes!("../tests/fixtures/xbox-fat32.zlib");
const XTAF16: &[u8] = include_bytes!("../tests/fixtures/xtaf-fat16-compact.zlib");
const XTAF32: &[u8] = include_bytes!("../tests/fixtures/xtaf-fat32-compact.zlib");

struct Image {
    bytes: Vec<u8>,
    size: u64,
}

impl Image {
    fn load(fixture: &[u8]) -> Self {
        let size = u64::from(u32::from_le_bytes(fixture[..4].try_into().unwrap()));
        let mut bytes = Vec::new();
        ZlibDecoder::new(&fixture[4..])
            .read_to_end(&mut bytes)
            .unwrap();
        assert!(bytes.len() as u64 <= size);
        Self { bytes, size }
    }
}

impl BlockSource for Image {
    fn len(&self) -> u64 {
        self.size
    }

    fn read_at(&mut self, offset: u64, dst: &mut [u8]) -> Result<()> {
        dst.fill(0);
        if offset + dst.len() as u64 > self.size {
            return Err(FoldError::Io("short FATX fixture"));
        }
        let offset = offset as usize;
        let count = self.bytes.len().saturating_sub(offset).min(dst.len());
        if count != 0 {
            dst[..count].copy_from_slice(&self.bytes[offset..offset + count]);
        }
        Ok(())
    }
}

fn payload() -> Vec<u8> {
    (0..13013).map(|n| (n * 17 + n / 117) as u8).collect()
}

#[test]
fn reads_both_native_table_widths_and_endiannesses() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    for (bytes, root_offset, width) in [
        (XBOX16, 0xa000, 2),
        (XBOX32, 0x52000, 4),
        (XTAF16, 0x3000, 2),
        (XTAF32, 0x49000, 4),
    ] {
        let mut fs = Fatx::open(Image::load(bytes), &mut cx).unwrap();
        assert_eq!(fs.geometry.cluster_offset(1).unwrap(), root_offset);
        assert_eq!(fs.geometry.entry_size, width);
        let folder = fs.lookup(fs.root(), "cache", &mut cx).unwrap().unwrap();
        let file = fs.lookup(folder, "DATA.BIN", &mut cx).unwrap().unwrap();
        let mut span = [0; 4096];
        assert_eq!(
            fs.read_at(file, 2041, &mut span, &mut cx).unwrap(),
            span.len()
        );
        assert_eq!(span, payload()[2041..6137]);
        if matches!(root_offset, 0xa000 | 0x52000) {
            // The original Xbox fixture allocates the empty file in cluster 4.
            assert_eq!(fs.file_chain.as_ref().unwrap().1, [3, 5, 6, 7, 8, 9, 10]);
        }
        assert_eq!(fs.read_at(file, 13003, &mut span, &mut cx).unwrap(), 10);
        assert_eq!(span[..10], payload()[13003..]);
        let empty = fs.lookup(fs.root(), "Empty", &mut cx).unwrap().unwrap();
        assert_eq!(fs.read_at(empty, 0, &mut span, &mut cx).unwrap(), 0);
        assert!(
            fs.local_timestamps(file, &mut cx)
                .unwrap()
                .created
                .is_some()
        );
        assert!(fs.timestamps(file, &mut cx).unwrap().created.is_none());
    }
}

#[test]
fn distinguishes_timestamp_roles_and_epochs() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    for (fixture, at, big_endian, epoch) in
        [(XBOX16, 0xa800, false, 2000), (XTAF16, 0x3800, true, 1980)]
    {
        let mut image = Image::load(fixture);
        for (index, day) in [29u32, 28, 27].into_iter().enumerate() {
            let date = (24 << 9) | (2 << 5) | day;
            let packed = (date << 16) | (12 << 11) | (15 << 5) | 13;
            let encoded = if big_endian {
                packed.to_be_bytes()
            } else {
                packed.to_le_bytes()
            };
            let start = at + 52 + index * 4;
            image.bytes[start..start + 4].copy_from_slice(&encoded);
        }
        let mut fs = Fatx::open(image, &mut cx).unwrap();
        let dir = fs.lookup(fs.root(), "Cache", &mut cx).unwrap().unwrap();
        let file = fs.lookup(dir, "Data.bin", &mut cx).unwrap().unwrap();
        let local = fs.local_timestamps(file, &mut cx).unwrap();
        let civil = |day| {
            let seconds = chrono::NaiveDate::from_ymd_opt(epoch + 24, 2, day)
                .unwrap()
                .and_hms_opt(12, 15, 26)
                .unwrap()
                .and_utc()
                .timestamp();
            Some((seconds, 0))
        };
        let (created, modified) = if big_endian { (29, 28) } else { (28, 29) };
        assert_eq!(local.created, civil(created));
        assert_eq!(local.modified, civil(modified));
        assert_eq!(local.accessed, civil(27));
    }
}

#[test]
fn supports_canonical_xtaf_root_and_refuses_malformed_file_chains() {
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut canonical = Image::load(XTAF16);
    canonical.bytes[12..16].copy_from_slice(&1u32.to_be_bytes());
    canonical.bytes[0x1000..0x1002].copy_from_slice(&0xfff8u16.to_be_bytes());
    canonical.bytes.splice(0x3000..0x3000, [0; 4096]);
    let mut fs = Fatx::open(canonical, &mut cx).unwrap();
    assert_eq!(fs.geometry.cluster_offset(1).unwrap(), 0x4000);
    let dir = fs.lookup(fs.root(), "Cache", &mut cx).unwrap().unwrap();
    let file = fs.lookup(dir, "Data.bin", &mut cx).unwrap().unwrap();
    let mut read = [0; 100];
    fs.read_at(file, 2000, &mut read, &mut cx).unwrap();
    assert_eq!(read, payload()[2000..2100]);

    for next in [0, 3, 0xffff, 0xfff7] {
        let mut damaged = Image::load(XBOX16);
        let start = u32::from_le_bytes(damaged.bytes[0xa82c..0xa830].try_into().unwrap());
        assert_eq!(start, 3);
        let fat_entry = 0x1000 + start as usize * 2;
        damaged.bytes[fat_entry..fat_entry + 2].copy_from_slice(&u16::to_le_bytes(next));
        let mut fs = Fatx::open(damaged, &mut cx).unwrap();
        let dir = fs.lookup(fs.root(), "Cache", &mut cx).unwrap().unwrap();
        let file = fs.lookup(dir, "Data.bin", &mut cx).unwrap().unwrap();
        assert!(matches!(
            fs.read_at(file, 0, &mut read, &mut cx),
            Err(FoldError::Corrupt(_))
        ));
    }
}
