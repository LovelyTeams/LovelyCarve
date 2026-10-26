use super::*;
use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use std::io::Read;
fn inflate(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .read_to_end(&mut output)
        .unwrap();
    output
}
#[test]
fn independent_amitools_variants_extensions_names_dates_and_ranges() {
    for (fixture, variant, block_size, blocks) in [
        (
            include_bytes!("../tests/fixtures/dos0-512-1760.zlib").as_slice(),
            0,
            512,
            1760,
        ),
        (
            include_bytes!("../tests/fixtures/dos1-512-1760.zlib").as_slice(),
            1,
            512,
            1760,
        ),
        (
            include_bytes!("../tests/fixtures/dos2-512-1760.zlib").as_slice(),
            2,
            512,
            1760,
        ),
        (
            include_bytes!("../tests/fixtures/dos3-512-1760.zlib").as_slice(),
            3,
            512,
            1760,
        ),
        (
            include_bytes!("../tests/fixtures/dos4-512-1760.zlib").as_slice(),
            4,
            512,
            1760,
        ),
        (
            include_bytes!("../tests/fixtures/dos5-512-1760.zlib").as_slice(),
            5,
            512,
            1760,
        ),
        (
            include_bytes!("../tests/fixtures/dos6-512-1760.zlib").as_slice(),
            6,
            512,
            1760,
        ),
        (
            include_bytes!("../tests/fixtures/dos7-512-1760.zlib").as_slice(),
            7,
            512,
            1760,
        ),
        (
            include_bytes!("../tests/fixtures/dos3-1024-4096.zlib").as_slice(),
            3,
            1024,
            4096,
        ),
        (
            include_bytes!("../tests/fixtures/dos7-4096-1760.zlib").as_slice(),
            7,
            4096,
            1760,
        ),
        (
            include_bytes!("../tests/fixtures/dos1-512-110000.zlib").as_slice(),
            1,
            512,
            110000,
        ),
    ] {
        let bytes = inflate(fixture);
        let mut cache = BlockCache::new(1024);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let mut reader = Amiga::open(SliceSource::new(&bytes), &mut context)
            .unwrap_or_else(|error| panic!("DOS{variant}, block {block_size}: {error}"));
        assert_eq!(
            (reader.geometry().block_size, reader.geometry().blocks),
            (block_size, blocks)
        );
        assert_eq!(reader.geometry().variant.dostype(), 0x444f5300 + variant);
        let root = reader.root();
        let entries = reader.read_dir(root, &mut context).unwrap();
        let big = reader
            .lookup(root, "café.bin", &mut context)
            .unwrap()
            .unwrap();
        assert_eq!(
            reader
                .lookup(root, "CAFÉ.BIN", &mut context)
                .unwrap()
                .is_some(),
            variant >= 2
        );
        assert_eq!(reader.metadata(big, &mut context).unwrap().size, 200013);
        let expected: Vec<_> = (0..200013).map(|index| (index * 17 + 5) as u8).collect();
        let mut whole = vec![0; expected.len()];
        assert_eq!(
            reader.read_at(big, 0, &mut whole, &mut context).unwrap(),
            expected.len()
        );
        assert_eq!(whole, expected);
        for offset in [0, 487, 511, 35871, 36863, 73721, 199999, 200013, u64::MAX] {
            let mut window = [0x55; 73];
            let count = reader
                .read_at(big, offset, &mut window, &mut context)
                .unwrap();
            let start = offset.min(200013) as usize;
            assert_eq!(
                &window[..count],
                &expected[start..(start + 73).min(expected.len())]
            );
            assert!(window[count..].iter().all(|byte| *byte == 0x55));
        }
        let times = reader.local_timestamps(big, &mut context).unwrap();
        assert_eq!(
            times.modified,
            Some((252460800 + 12345 * 86400 + 678 * 60 + 59, 980_000_000))
        );
        assert!(times.accessed.is_none() && times.created.is_none());
        assert!(reader
            .timestamps(big, &mut context)
            .unwrap()
            .modified
            .is_none());
        let sub = reader.lookup(root, "SUB", &mut context).unwrap().unwrap();
        let children = reader.read_dir(sub, &mut context).unwrap();
        assert_eq!(children.len(), 101);
        let name = reader
            .lookup(root, "back\\slash", &mut context)
            .unwrap()
            .unwrap();
        let mut body = [0; 100];
        let count = reader.read_at(name, 0, &mut body, &mut context).unwrap();
        assert_eq!(&body[..count], b"Unix-safe evidence name");
        if variant >= 6 {
            let long = entries
                .iter()
                .find(|entry| entry.name.chars().count() == 107)
                .unwrap();
            assert_eq!(long.kind, FileKind::Regular);
            let count = reader
                .read_at(long.node, 0, &mut body, &mut context)
                .unwrap();
            assert_eq!(&body[..count], b"long filename");
        }
    }
}

fn context(cache: &mut BlockCache) -> SubstrateCtx<'_> {
    SubstrateCtx {
        cache,
        verifier: &NoVerifier,
    }
}
fn open(bytes: &[u8]) -> lamfold::Result<Amiga<SliceSource<'_>>> {
    Amiga::open(
        SliceSource::new(bytes),
        &mut context(&mut BlockCache::new(0)),
    )
}
fn set32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}
fn seal(bytes: &mut [u8]) {
    set32(bytes, 20, 0);
    let sum = bytes.as_chunks::<4>().0.iter().fold(0u32, |sum, word| {
        sum.wrapping_add(u32::from_be_bytes(*word))
    });
    set32(bytes, 20, 0u32.wrapping_sub(sum));
}
#[test]
fn native_link_headers_keep_alias_identity_own_dates_and_raw_latin1_targets() {
    let bytes = inflate(include_bytes!("../tests/fixtures/links-dos3.zlib"));
    let mut reader = open(&bytes).unwrap();
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let root = reader.root();
    let original = reader.lookup(root, "Café.bin", &mut cx).unwrap().unwrap();
    let alias = reader.lookup(root, "alias.bin", &mut cx).unwrap().unwrap();
    assert_ne!(original, alias);
    assert_eq!(
        reader.inode_identity(original, &mut cx).unwrap(),
        reader.inode_identity(alias, &mut cx).unwrap()
    );
    assert_eq!(reader.metadata(alias, &mut cx).unwrap().size, 200013);
    assert_ne!(
        reader.local_timestamps(original, &mut cx).unwrap().modified,
        reader.local_timestamps(alias, &mut cx).unwrap().modified
    );
    let mut first = vec![0; 200013];
    let mut second = first.clone();
    reader.read_at(original, 0, &mut first, &mut cx).unwrap();
    reader.read_at(alias, 0, &mut second, &mut cx).unwrap();
    assert_eq!(first, second);
    let sub = reader.lookup(root, "sub", &mut cx).unwrap().unwrap();
    let directory_link = reader.lookup(root, "dirlink", &mut cx).unwrap().unwrap();
    assert_eq!(
        reader.inode_identity(sub, &mut cx).unwrap(),
        reader.inode_identity(directory_link, &mut cx).unwrap()
    );
    let direct = reader.read_dir(sub, &mut cx).unwrap();
    let linked = reader.read_dir(directory_link, &mut cx).unwrap();
    assert_eq!(direct.len(), linked.len());
    assert_eq!(direct[0].node, linked[0].node);
    let soft = reader.lookup(root, "softlink", &mut cx).unwrap().unwrap();
    assert_eq!(
        reader.read_link(soft, &mut cx).unwrap().unwrap(),
        b"Work:Caf\xe9.bin"
    );
    assert_eq!(
        reader.read_link_text(soft, &mut cx).unwrap().unwrap(),
        "Work:Café.bin"
    );
    let mut raw = [0; 100];
    let length = reader.read_at(soft, 0, &mut raw, &mut cx).unwrap();
    assert_eq!(&raw[..length], b"Work:Caf\xe9.bin");
}
#[test]
fn dirty_root_truncation_and_invalid_calendar_fields_fail_without_zero_times() {
    let original = inflate(include_bytes!("../tests/fixtures/dos1-512-1760.zlib"));
    let mut bytes = original.clone();
    set32(&mut bytes, 880 * 512 + 312, 0);
    seal(&mut bytes[880 * 512..881 * 512]);
    assert!(Geometry::inspect(&mut SliceSource::new(&bytes), None).is_ok());
    assert!(matches!(open(&bytes), Err(FoldError::Unsupported(_))));
    assert!(open(&original[..original.len() - 1]).is_err());
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut reader = open(&original).unwrap();
    let node = reader
        .lookup(reader.root(), "Café.bin", &mut cx)
        .unwrap()
        .unwrap();
    for (days, minutes, ticks, valid) in [(0, 0, 0, true), (1, 1440, 0, false), (1, 0, 3000, false)]
    {
        let mut bytes = original.clone();
        let start = node as usize * 512;
        set32(&mut bytes, start + 420, days);
        set32(&mut bytes, start + 424, minutes);
        set32(&mut bytes, start + 428, ticks);
        seal(&mut bytes[start..start + 512]);
        let mut reader = open(&bytes).unwrap();
        let result = reader.local_timestamps(node, &mut cx);
        assert_eq!(result.is_ok(), valid);
        if valid {
            assert_eq!(result.unwrap().modified, Some((252460800, 0)));
        }
    }
}
#[test]
fn duplicate_free_outside_and_short_file_maps_are_never_complete_reads() {
    let original = inflate(include_bytes!("../tests/fixtures/dos1-512-1760.zlib"));
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut reader = open(&original).unwrap();
    let node = reader
        .lookup(reader.root(), "Café.bin", &mut cx)
        .unwrap()
        .unwrap();
    let start = node as usize * 512;
    let first = amiga_ffs::be32(&original, start + 308);
    for (offset, value) in [(start + 304, first), (start + 308, 1760), (start + 324, 1)] {
        let mut bytes = original.clone();
        set32(&mut bytes, offset, value);
        seal(&mut bytes[start..start + 512]);
        let mut reader = open(&bytes).unwrap();
        assert!(reader.read_at(node, 0, &mut [0; 100], &mut cx).is_err());
    }
    let mut bytes = original.clone();
    let bitmap = amiga_ffs::be32(&bytes, 880 * 512 + 316) as usize * 512;
    let bit = first - 2;
    let position = bitmap + 4 + (bit / 32) as usize * 4;
    let word = amiga_ffs::be32(&bytes, position);
    set32(&mut bytes, position, word | (1 << (bit % 32)));
    set32(&mut bytes, bitmap, 0);
    let sum = bytes[bitmap..bitmap + 512]
        .as_chunks::<4>()
        .0
        .iter()
        .fold(0u32, |sum, word| {
            sum.wrapping_add(u32::from_be_bytes(*word))
        });
    set32(&mut bytes, bitmap, 0u32.wrapping_sub(sum));
    let mut reader = open(&bytes).unwrap();
    assert!(reader.read_at(node, 0, &mut [0; 100], &mut cx).is_err());
}
#[test]
fn hardlink_cycles_and_unterminated_softlinks_are_explicit_errors() {
    let original = inflate(include_bytes!("../tests/fixtures/links-dos3.zlib"));
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut reader = open(&original).unwrap();
    let root = reader.root();
    let alias = reader.lookup(root, "alias.bin", &mut cx).unwrap().unwrap();
    let soft = reader.lookup(root, "softlink", &mut cx).unwrap().unwrap();
    let mut bytes = original.clone();
    let start = alias as usize * 512;
    set32(&mut bytes, start + 468, alias as u32);
    seal(&mut bytes[start..start + 512]);
    let mut reader = open(&bytes).unwrap();
    assert!(reader.metadata(alias, &mut cx).is_err());
    let mut bytes = original;
    let start = soft as usize * 512;
    bytes[start + 24..start + 312].fill(b'x');
    seal(&mut bytes[start..start + 512]);
    let mut reader = open(&bytes).unwrap();
    assert!(reader.read_link(soft, &mut cx).is_err());
}

#[test]
fn ofs_owner_sequence_payload_and_checksum_corruption_fail() {
    let original = inflate(include_bytes!("../tests/fixtures/dos0-512-1760.zlib"));
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut reader = open(&original).unwrap();
    let node = reader
        .lookup(reader.root(), "Café.bin", &mut cx)
        .unwrap()
        .unwrap();
    let data = amiga_ffs::be32(&original, node as usize * 512 + 308) as usize * 512;
    for (offset, value) in [(4, node as u32 + 1), (8, 2), (12, 489)] {
        let mut bytes = original.clone();
        set32(&mut bytes, data + offset, value);
        seal(&mut bytes[data..data + 512]);
        let mut reader = open(&bytes).unwrap();
        assert!(reader.read_at(node, 0, &mut [0; 17], &mut cx).is_err());
    }
    let mut bytes = original;
    bytes[data + 25] ^= 1;
    let mut reader = open(&bytes).unwrap();
    assert!(reader.read_at(node, 0, &mut [0; 17], &mut cx).is_err());
}
#[test]
fn source_errors_propagate_and_ffs_ranges_coalesce_without_reading_the_prefix() {
    use std::{cell::RefCell, rc::Rc};
    let bytes = inflate(include_bytes!("../tests/fixtures/dos1-512-1760.zlib"));
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut reader = open(&bytes).unwrap();
    let node = reader
        .lookup(reader.root(), "Café.bin", &mut cx)
        .unwrap()
        .unwrap();
    let fault = u64::from(amiga_ffs::be32(&bytes, node as usize * 512 + 308)) * 512;
    struct AuditSource<'a> {
        bytes: &'a [u8],
        reads: Rc<RefCell<Vec<(u64, usize)>>>,
        fault: Rc<RefCell<Option<u64>>>,
    }
    impl BlockSource for AuditSource<'_> {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(&mut self, offset: u64, output: &mut [u8]) -> lamfold::Result<()> {
            if self
                .fault
                .borrow()
                .is_some_and(|fault| offset <= fault && offset + output.len() as u64 > fault)
            {
                return Err(FoldError::Io("injected evidence read failure"));
            }
            self.reads.borrow_mut().push((offset, output.len()));
            let offset = offset as usize;
            output.copy_from_slice(&self.bytes[offset..offset + output.len()]);
            Ok(())
        }
    }
    let reads = Rc::new(RefCell::new(Vec::new()));
    let fault_switch = Rc::new(RefCell::new(None));
    let mut reader = Amiga::open(
        AuditSource {
            bytes: &bytes,
            reads: Rc::clone(&reads),
            fault: Rc::clone(&fault_switch),
        },
        &mut cx,
    )
    .unwrap();
    reader.read_at(node, 0, &mut [0; 1], &mut cx).unwrap();
    reads.borrow_mut().clear();
    let mut window = [0; 97];
    assert_eq!(
        reader.read_at(node, 100003, &mut window, &mut cx).unwrap(),
        97
    );
    assert_eq!(reads.borrow().len(), 1);
    assert_eq!(
        window.as_slice(),
        (100003..100100)
            .map(|index| (index * 17 + 5) as u8)
            .collect::<Vec<_>>()
    );
    *fault_switch.borrow_mut() = Some(fault);
    assert!(matches!(
        reader.read_at(node, 0, &mut window, &mut cx),
        Err(FoldError::Io(_))
    ));
}
