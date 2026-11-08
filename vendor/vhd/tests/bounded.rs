//! Regression tests for LovelyCarve's bounded VHD opening path.
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use vhd::VhdReader;

struct FooterOnly {
    footer: [u8; 512],
    payload_size: u64,
    position: u64,
}

impl Read for FooterOnly {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let offset = self
            .position
            .checked_sub(self.payload_size)
            .ok_or_else(|| io::Error::other("open attempted to read virtual payload"))?
            as usize;
        let count = output.len().min(512usize.saturating_sub(offset));
        if count > 0 {
            output[..count].copy_from_slice(&self.footer[offset..offset + count]);
        }
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for FooterOnly {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        let next = match offset {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.payload_size + 512) + i128::from(delta),
        };
        self.position = u64::try_from(next).map_err(|_| io::Error::other("invalid seek"))?;
        Ok(self.position)
    }
}

#[test]
fn opens_terabyte_fixed_disk_without_reading_payload_and_checks_seek_overflow() {
    let size = 1u64 << 40;
    let mut footer = [0u8; 512];
    footer[..8].copy_from_slice(b"conectix");
    footer[12..16].copy_from_slice(&0x10000u32.to_be_bytes());
    footer[16..24].copy_from_slice(&u64::MAX.to_be_bytes());
    footer[40..48].copy_from_slice(&size.to_be_bytes());
    footer[48..56].copy_from_slice(&size.to_be_bytes());
    footer[60..64].copy_from_slice(&2u32.to_be_bytes());
    let checksum = !footer.iter().map(|value| u32::from(*value)).sum::<u32>();
    footer[64..68].copy_from_slice(&checksum.to_be_bytes());
    let mut reader = VhdReader::open_reader(Box::new(FooterOnly {
        footer,
        payload_size: size,
        position: 0,
    }))
    .unwrap();
    assert_eq!(reader.virtual_disk_size(), size);
    assert_eq!(reader.seek(SeekFrom::Start(u64::MAX)).unwrap(), u64::MAX);
    assert!(reader.seek(SeekFrom::Current(1)).is_err());
    reader.seek(SeekFrom::Start(0)).unwrap();
    assert!(reader.seek(SeekFrom::Current(-1)).is_err());

    let mut truncated = vec![0; 512];
    truncated.extend_from_slice(&footer);
    assert!(VhdReader::open_reader(Box::new(io::Cursor::new(truncated))).is_err());
}

struct Counted {
    inner: io::Cursor<Vec<u8>>,
    bytes: Arc<AtomicUsize>,
}
impl Read for Counted {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.bytes.load(Ordering::Relaxed) + output.len() > 4096 {
            return Err(io::Error::other("open exceeded metadata read budget"));
        }
        let count = self.inner.read(output)?;
        self.bytes.fetch_add(count, Ordering::Relaxed);
        Ok(count)
    }
}
impl Seek for Counted {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        self.inner.seek(offset)
    }
}

#[test]
fn dynamic_open_reads_only_footer_header_and_bat() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ntfs_dynamic.vhd");
    let data = std::fs::read(path).unwrap();
    let bytes = Arc::new(AtomicUsize::new(0));
    let reader = VhdReader::open_reader(Box::new(Counted {
        inner: io::Cursor::new(data),
        bytes: bytes.clone(),
    }))
    .unwrap();
    assert_eq!(reader.virtual_disk_size(), 4_194_304);
    assert!(bytes.load(Ordering::Relaxed) <= 4096);
}
