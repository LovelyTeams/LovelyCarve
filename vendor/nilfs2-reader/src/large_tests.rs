use crate::{tests::inflate, Nilfs2};
use lamfold::{BlockCache, BlockSource, FoldError, FoldFrontend, NoVerifier, SubstrateCtx};
use std::{cell::Cell, rc::Rc};

struct Source {
    length: u64,
    chunks: Vec<(u64, Vec<u8>)>,
    calls: Rc<Cell<usize>>,
    bytes: Rc<Cell<usize>>,
    fault: Rc<Cell<bool>>,
}
impl BlockSource for Source {
    fn len(&self) -> u64 {
        self.length
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> lamfold::Result<()> {
        self.calls.set(self.calls.get() + 1);
        self.bytes.set(self.bytes.get() + bytes.len());
        if self.fault.get() {
            return Err(FoldError::Io("invented high-source read fault"));
        }
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or(FoldError::Io("offset overflow"))?;
        if end > self.length {
            return Err(FoldError::Io("invented source bound"));
        }
        bytes.fill(0);
        for (start, data) in &self.chunks {
            let first = offset.max(*start);
            let last = end.min(*start + data.len() as u64);
            if first < last {
                bytes[(first - offset) as usize..(last - offset) as usize]
                    .copy_from_slice(&data[(first - start) as usize..(last - start) as usize]);
            }
        }
        Ok(())
    }
}
#[test]
fn high_physical_and_logical_offsets_seek_directly_and_preserve_source_faults() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/manifest.json")).unwrap();
    let high = &manifest["virtual_source"];
    let packed: [&[u8]; 3] = [
        include_bytes!("../tests/fixtures/high-chunk-0.zlib"),
        include_bytes!("../tests/fixtures/high-chunk-1.zlib"),
        include_bytes!("../tests/fixtures/high-chunk-2.zlib"),
    ];
    let chunks = high["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .zip(packed)
        .map(|(row, packed)| (row["offset"].as_u64().unwrap(), inflate(packed)))
        .collect();
    let calls = Rc::new(Cell::new(0));
    let bytes = Rc::new(Cell::new(0));
    let fault = Rc::new(Cell::new(false));
    let source = Source {
        length: high["length"].as_u64().unwrap(),
        chunks,
        calls: Rc::clone(&calls),
        bytes: Rc::clone(&bytes),
        fault: Rc::clone(&fault),
    };
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = Nilfs2::open(source, &mut context).unwrap();
    assert!(
        reader.header().last_partial_segment * reader.header().block_size as u64 > u32::MAX as u64
    );
    let root = reader.root();
    assert_eq!(reader.lookup(root, "big", &mut context).unwrap(), Some(17));
    let size = 5 * 1024u64.pow(3) + 37;
    assert_eq!(reader.metadata(17, &mut context).unwrap().size, size);
    calls.set(0);
    bytes.set(0);
    let mut output = [0; 19];
    let offset = 5 * 1024u64.pow(3) + 13;
    assert_eq!(
        reader
            .read_at(17, offset, &mut output, &mut context)
            .unwrap(),
        output.len()
    );
    assert_eq!(
        output,
        std::array::from_fn(|index| ((index + 13) * 17 + 5) as u8)
    );
    assert!(
        calls.get() < 16 && bytes.get() < 65536,
        "seek must not scan a multi-GiB prefix"
    );
    calls.set(0);
    assert_eq!(
        reader
            .read_at(17, 4 * 1024u64.pow(3), &mut output, &mut context)
            .unwrap(),
        output.len()
    );
    assert_eq!(output, [0; 19]);
    assert_eq!(calls.get(), 0);
    assert_eq!(
        reader.read_at(17, size, &mut output, &mut context).unwrap(),
        0
    );
    fault.set(true);
    assert!(reader.read_at(17, 0, &mut output, &mut context).is_err());
}
