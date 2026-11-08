//! A virtual invented source keeps 64-bit addressing tests small. Reads of
//! unknown allocated bytes fail; only explicit metadata and pattern runs exist.
use super::*;
use lamfold::{BlockCache, FoldFrontend, NoVerifier, SubstrateCtx};
use std::{cell::RefCell, collections::BTreeMap, io::Read, rc::Rc};

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
fn run(bytes: &mut [u8], at: usize, group: u32, start: u16, length: u16) {
    put32(bytes, at, group);
    bytes[at + 4..at + 6].copy_from_slice(&start.to_le_bytes());
    bytes[at + 6..at + 8].copy_from_slice(&length.to_le_bytes());
}
fn pattern(logical: u64) -> u8 {
    ((logical * 31 + (logical >> 32) * 53 + (logical >> 15) + 7) & 255) as u8
}
struct Invented {
    metadata: BTreeMap<u64, Vec<u8>>,
    reads: Rc<RefCell<Vec<(u64, usize)>>>,
}
impl BlockSource for Invented {
    fn len(&self) -> u64 {
        10_000_000 * 1024
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        require(
            offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= self.len()),
            "large BFS source range",
        )?;
        self.reads.borrow_mut().push((offset, bytes.len()));
        for (index, byte) in bytes.iter_mut().enumerate() {
            let position = offset + index as u64;
            let block = position / 1024;
            let within = (position % 1024) as usize;
            *byte = if let Some(raw) = self.metadata.get(&block) {
                raw[within]
            } else if (1..=1221).contains(&block) {
                0xff
            } else {
                let group = block / 65536;
                let start = block % 65536;
                require(
                    (20..102).contains(&group) && start > 0,
                    "large BFS read of unspecified source bytes",
                )?;
                pattern(((group - 20) * 65535 + start - 1) * 1024 + within as u64)
            };
        }
        Ok(())
    }
}
#[test]
fn ranges_beyond_four_gib_use_64_bit_logical_and_physical_offsets() {
    let mut original = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../tests/fixtures/le-1024-2560.zlib").as_slice(),
    )
    .read_to_end(&mut original)
    .unwrap();
    let mut boot = original[..1024].to_vec();
    let sb = 512;
    put64(&mut boot, sb + 48, 10_000_000);
    put64(&mut boot, sb + 56, 10_000_000);
    put32(&mut boot, sb + 72, 8);
    put32(&mut boot, sb + 76, 16);
    put32(&mut boot, sb + 80, 153);
    run(&mut boot, sb + 88, 0, 1222, 32);
    put64(&mut boot, sb + 96, 1222);
    put64(&mut boot, sb + 104, 1222);
    run(&mut boot, sb + 116, 0, 2000, 1);
    let mut root = original[65 * 1024..66 * 1024].to_vec();
    run(&mut root, 4, 0, 2000, 1);
    run(&mut root, 44, 0, 2000, 1);
    run(&mut root, 72, 0, 2001, 2);
    let mut tree = original[160 * 1024..162 * 1024].to_vec();
    for (index, value) in [2000, 2000, 2003].into_iter().enumerate() {
        put64(&mut tree, 1024 + 54 + index * 8, value);
    }
    let mut file = original[67 * 1024..68 * 1024].to_vec();
    run(&mut file, 4, 0, 2003, 1);
    run(&mut file, 44, 0, 2000, 1);
    file[52..60].fill(0);
    file[72..216].fill(0);
    for index in 0..12 {
        run(&mut file, 72 + index * 8, 20 + index as u32, 1, 65535);
    }
    let length = 5 * 1024 * 1024 * 1024u64 + 12345;
    put64(&mut file, 72 + 96, 12 * 65535 * 1024);
    run(&mut file, 72 + 104, 0, 2004, 1);
    put64(&mut file, 72 + 112, 82 * 65535 * 1024);
    put64(&mut file, 72 + 136, length);
    let mut array = vec![0; 1024];
    for index in 0..70 {
        run(&mut array, index * 8, 32 + index as u32, 1, 65535);
    }
    let metadata = BTreeMap::from([
        (0, boot),
        (2000, root),
        (2001, tree[..1024].to_vec()),
        (2002, tree[1024..].to_vec()),
        (2003, file),
        (2004, array),
    ]);
    let reads = Rc::new(RefCell::new(Vec::new()));
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = BeFs::open(
        Invented {
            metadata,
            reads: reads.clone(),
        },
        &mut cx,
    )
    .unwrap();
    assert_eq!(reader.metadata(2003, &mut cx).unwrap().size, length);
    reader.read_at(2003, 0, &mut [0; 1], &mut cx).unwrap();
    reads.borrow_mut().clear();
    let offset = 4 * 1024 * 1024 * 1024u64 + 11;
    let mut raw = [0; 97];
    assert_eq!(reader.read_at(2003, offset, &mut raw, &mut cx).unwrap(), 97);
    assert_eq!(
        raw.as_slice(),
        (offset..offset + 97).map(pattern).collect::<Vec<_>>()
    );
    assert_eq!(reads.borrow().len(), 1);
    for offset in [12 * 65535 * 1024 - 7, length - 3, length, u64::MAX] {
        let count = reader.read_at(2003, offset, &mut raw, &mut cx).unwrap();
        assert_eq!(count, length.saturating_sub(offset).min(97) as usize);
        assert_eq!(
            &raw[..count],
            (offset..offset + count as u64)
                .map(pattern)
                .collect::<Vec<_>>()
        );
    }
}
