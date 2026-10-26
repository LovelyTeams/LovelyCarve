//! CC0 invented sparse source; all undeclared allocated reads fail.
use crate::{extents::Stream, inode::Inode, Header};
use lamfold::{BlockSource, FoldError};
struct Source {
    inode: Vec<u8>,
    data: u64,
    calls: Vec<(u64, usize)>,
}
impl BlockSource for Source {
    fn len(&self) -> u64 {
        8 * 1024 * 1024 * 1024
    }
    fn read_at(&mut self, offset: u64, raw: &mut [u8]) -> lamfold::Result<()> {
        self.calls.push((offset, raw.len()));
        if offset == 513 * 1024 && raw.len() == self.inode.len() {
            raw.copy_from_slice(&self.inode);
            return Ok(());
        }
        if offset >= self.data && offset + raw.len() as u64 <= self.data + 4096 {
            for (index, byte) in raw.iter_mut().enumerate() {
                *byte = ((offset - self.data + index as u64) * 31 + 9) as u8;
            }
            return Ok(());
        }
        Err(FoldError::Io("undeclared allocated source bytes"))
    }
}
#[test]
fn high_logical_and_physical_offsets_seek_directly_and_preserve_errors() {
    let original = super::unit_tests::fixture("plain");
    let mut source = Source {
        inode: original[513 * 1024..514 * 1024].to_vec(),
        data: 6 * 1024 * 1024 * 1024,
        calls: Vec::new(),
    };
    let mut sb = original[2048..3072].to_vec();
    sb[20..24].copy_from_slice(&((source.len() / 4096) as u32).to_le_bytes());
    let header = Header::inspect_block(&sb, 1024, source.len()).unwrap();
    let length = 5 * 1024 * 1024 * 1024u64 + 37;
    let physical = source.data / 1024;
    let raw = &mut source.inode;
    raw[20..24].copy_from_slice(&1u32.to_le_bytes());
    raw[32..40].copy_from_slice(&length.to_le_bytes());
    raw[118..120].fill(0);
    raw[120..128].fill(0);
    raw[192..].fill(0);
    raw[194..196].copy_from_slice(&51u16.to_le_bytes());
    raw[196..198].copy_from_slice(&1u16.to_le_bytes());
    raw[208..212].copy_from_slice(&((length / 4096) as u32).to_le_bytes());
    raw[212..214].copy_from_slice(&1u16.to_le_bytes());
    raw[216..224].copy_from_slice(&physical.to_le_bytes());
    let inode = Inode::read(&mut source, &header, 513).unwrap();
    let stream = Stream::inode(&mut source, &header, &inode).unwrap();
    source.calls.clear();
    let offset = length - 30;
    let mut raw = [0; 97];
    assert_eq!(stream.read(&mut source, offset, &mut raw).unwrap(), 30);
    assert_eq!(
        &raw[..30],
        (7..37)
            .map(|index| (index * 31 + 9) as u8)
            .collect::<Vec<_>>()
    );
    assert_eq!(source.calls, [(physical * 1024 + 7, 30)]);
    source.calls.clear();
    assert_eq!(
        stream
            .read(&mut source, 4 * 1024 * 1024 * 1024, &mut raw)
            .unwrap(),
        97
    );
    assert_eq!(raw, [0; 97]);
    assert!(source.calls.is_empty());
    assert_eq!(stream.read(&mut source, length, &mut raw).unwrap(), 0);
    source.data += 4096;
    assert!(stream.read(&mut source, offset, &mut raw).is_err());
}
