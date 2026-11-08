//! CC0 virtual invented source: absent allocated bytes fail, while zero tree
//! pointers are true holes. Logical and physical positions both exceed 4 GiB.
use crate::{inode::Inode, stream::Stream, Header};
use lamfold::{BlockSource, FoldError};

struct Source {
    pages: Vec<(u64, Vec<u8>)>,
    data: u64,
    calls: Vec<(u64, usize)>,
}
impl BlockSource for Source {
    fn len(&self) -> u64 {
        8 * 1024 * 1024 * 1024
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> lamfold::Result<()> {
        self.calls.push((offset, bytes.len()));
        for (at, page) in &self.pages {
            if offset >= *at && offset + bytes.len() as u64 <= *at + page.len() as u64 {
                bytes.copy_from_slice(
                    &page[(offset - at) as usize..(offset - at) as usize + bytes.len()],
                );
                return Ok(());
            }
        }
        if offset >= self.data && offset + bytes.len() as u64 <= self.data + 4096 {
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = ((offset - self.data + index as u64) * 31 + 9) as u8;
            }
            return Ok(());
        }
        Err(FoldError::Io("invented allocated span missing"))
    }
}
fn put32(raw: &mut [u8], offset: usize, value: u32) {
    raw[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}
fn put64(raw: &mut [u8], offset: usize, value: u64) {
    raw[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}
#[test]
fn high_ranges_seek_without_reading_prefix_and_holes_need_no_source_bytes() {
    let mut prefix = vec![0; 272];
    for (offset, value) in [
        (0, 0x01161970),
        (4, 1),
        (16, 100),
        (24, 1802),
        (28, 1900),
        (36, 4096),
        (40, 12),
    ] {
        put32(&mut prefix, offset, value);
    }
    for (offset, value) in [(48, 2), (56, 32), (80, 1), (88, 33)] {
        put64(&mut prefix, offset, value);
    }
    let physical = 4 * 1024 * 1024 * 1024u64 + 4096 * 1000;
    let mut source = Source {
        pages: Vec::new(),
        data: physical + 8192,
        calls: Vec::new(),
    };
    let header = Header::inspect_prefix(&prefix, source.len()).unwrap();
    let length = 5 * 1024 * 1024 * 1024u64 + 37;
    let mut inode = vec![0; 4096];
    for (offset, value) in [(0, 0x01161970), (4, 4), (16, 400), (40, 0x81a4), (52, 1)] {
        put32(&mut inode, offset, value);
    }
    for (offset, value) in [(24, 20), (32, 40), (56, length), (64, 4)] {
        put64(&mut inode, offset, value);
    }
    inode[138..140].copy_from_slice(&3u16.to_be_bytes());
    let logical = (length - 1) / 4096;
    put64(
        &mut inode,
        232 + (logical / (509 * 509)) as usize * 8,
        physical / 4096,
    );
    let mut parent = vec![0; 4096];
    let mut child = vec![0; 4096];
    for raw in [&mut parent, &mut child] {
        put32(raw, 0, 0x01161970);
        put32(raw, 4, 5);
        put32(raw, 16, 500);
    }
    put64(
        &mut parent,
        24 + ((logical / 509) % 509) as usize * 8,
        physical / 4096 + 1,
    );
    put64(
        &mut child,
        24 + (logical % 509) as usize * 8,
        physical / 4096 + 2,
    );
    source.pages = vec![
        (40 * 4096, inode),
        (physical, parent),
        (physical + 4096, child),
    ];
    let inode = Inode::read(&mut source, &header, 40).unwrap();
    let stream = Stream::parse(&mut source, &header, &inode).unwrap();
    source.calls.clear();
    let mut bytes = [0; 97];
    let offset = 5 * 1024 * 1024 * 1024u64 + 7;
    assert_eq!(
        stream
            .read(&mut source, &header, offset, &mut bytes)
            .unwrap(),
        30
    );
    assert_eq!(
        &bytes[..30],
        (7..37)
            .map(|index| (index * 31 + 9) as u8)
            .collect::<Vec<_>>()
    );
    assert_eq!(source.calls, [(physical + 8192 + 7, 30)]);
    source.calls.clear();
    assert_eq!(
        stream
            .read(&mut source, &header, 4 * 1024 * 1024 * 1024, &mut bytes)
            .unwrap(),
        97
    );
    assert_eq!(bytes, [0; 97]);
    assert!(source.calls.is_empty());
    assert_eq!(
        stream
            .read(&mut source, &header, length, &mut bytes)
            .unwrap(),
        0
    );
    source.data += 4096;
    assert!(stream
        .read(&mut source, &header, offset, &mut bytes)
        .is_err());
}
