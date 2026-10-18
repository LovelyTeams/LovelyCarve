//! Large reads must agree with the cached serial path, including unaligned
//! ranges, segment boundaries, final chunks and EWF2 compression.
use std::path::Path;

use ewf::EwfReader;
use md5::{Digest, Md5};
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};

struct TracedSource {
    bytes: Vec<u8>,
    reads: AtomicUsize,
    last_offset: AtomicU64,
    fail_offset: AtomicU64,
}

impl ewf::SegmentBacking for TracedSource {
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.last_offset.store(offset, Ordering::Relaxed);
        if offset == self.fail_offset.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("injected source failure"));
        }
        let start = offset.min(self.bytes.len() as u64) as usize;
        let count = buffer.len().min(self.bytes.len() - start);
        buffer[..count].copy_from_slice(&self.bytes[start..start + count]);
        Ok(count)
    }
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
}

#[test]
fn bulk_reads_preserve_hot_cache_and_propagate_worker_failures(
) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ntfs_sample.E01");
    let source = Arc::new(TracedSource {
        bytes: std::fs::read(path)?,
        reads: AtomicUsize::new(0),
        last_offset: AtomicU64::new(0),
        fail_offset: AtomicU64::new(u64::MAX),
    });
    let reader =
        EwfReader::open_from_sources(vec![ewf::SegmentSource::from_backing(source.clone())])?;
    let mut hot = [0; 4096];
    reader.read_at(&mut hot, 0)?;
    let mut bulk = vec![0; 5 * 1024 * 1024];
    reader.read_at(&mut bulk, 0)?;
    source.reads.store(0, Ordering::Relaxed);
    let mut again = [0; 4096];
    reader.read_at(&mut again, 0)?;
    assert_eq!(again, hot);
    assert_eq!(
        source.reads.load(Ordering::Relaxed),
        0,
        "bulk scan evicted hot metadata"
    );

    // Locate a stored chunk through a real positioned read. The bulk path must
    // revalidate source bytes even if that chunk already exists in the LRU.
    reader.read_at(&mut again, 33 * reader.chunk_size())?;
    source.fail_offset.store(
        source.last_offset.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );
    assert!(
        reader.read_at(&mut bulk, 0).is_err(),
        "worker I/O failure was swallowed"
    );
    assert_eq!(reader.read_at(&mut again, 0)?, again.len());
    assert_eq!(again, hot);
    Ok(())
}

#[test]
fn bulk_reads_match_serial_bytes_and_acquisition_hashes() -> Result<(), Box<dyn std::error::Error>>
{
    for name in [
        "ntfs_sample.E01",
        "multiseg_v1.E01",
        "ewfacquire_clean.E01",
        "zeros_128s.Ex01",
        "zeros_128s_compressed.Ex01",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(name);
        for lazy in [false, true] {
            let reader = if lazy {
                EwfReader::open_lazy(&path)?
            } else {
                EwfReader::open(&path)?
            };
            let size = reader.total_size() as usize;
            let mut expected = vec![0; size];
            // Below the bulk threshold: exercises the original serial path.
            for (index, destination) in expected.chunks_mut(65521).enumerate() {
                assert_eq!(
                    reader.read_at(destination, (index * 65521) as u64)?,
                    destination.len()
                );
            }
            if let Some(stored) = reader.stored_hashes().md5 {
                assert_eq!(<[u8; 16]>::from(Md5::digest(&expected)), stored, "{name}");
            }
            for offset in [0, 1, 32767, 32768, size.saturating_sub(513), size, size + 1] {
                let mut output = vec![0xa5; 1024 * 1024 + 17];
                let count = reader.read_at(&mut output, offset as u64)?;
                let count_expected = output.len().min(size.saturating_sub(offset));
                assert_eq!(count, count_expected, "{name} lazy={lazy} offset={offset}");
                if count > 0 {
                    assert_eq!(&output[..count], &expected[offset..offset + count]);
                }
                assert!(output[count..].iter().all(|byte| *byte == 0xa5));
            }
            let mut digest = Md5::new();
            let mut offset = 0;
            let mut buffer = vec![0; 3 * 1024 * 1024 + 71];
            while offset < size {
                let count = reader.read_at(&mut buffer, offset as u64)?;
                assert!(count > 0);
                digest.update(&buffer[..count]);
                offset += count;
            }
            assert_eq!(digest.finalize()[..], Md5::digest(&expected)[..]);
        }
    }
    Ok(())
}
