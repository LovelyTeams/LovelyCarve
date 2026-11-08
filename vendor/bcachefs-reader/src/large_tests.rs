//! A virtual 8 GiB evidence source places allocated data above the u32 byte
//! boundary; only the described bytes exist in memory, not an 8 GiB buffer.
use super::tests::{fixture, key_at, stamp_node};
use super::*;
use lamfold::{BlockCache, NoVerifier};
use std::sync::atomic::{AtomicU64, Ordering};

const LENGTH: u64 = 8 * 1024 * 1024 * 1024;
const DATA_AT: u64 = 6 * 1024 * 1024 * 1024;

struct LargeSource {
    prefix: Vec<u8>,
    content: Vec<u8>,
    transferred: Arc<AtomicU64>,
}
impl BlockSource for LargeSource {
    fn len(&self) -> u64 {
        LENGTH
    }
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buffer.len() as u64)
            .ok_or(FoldError::Io("overflow"))?;
        if end > LENGTH {
            return Err(FoldError::Io("source bounds"));
        }
        self.transferred
            .fetch_add(buffer.len() as u64, Ordering::Relaxed);
        if end <= self.prefix.len() as u64 {
            buffer.copy_from_slice(&self.prefix[offset as usize..end as usize]);
        } else if offset >= DATA_AT && end <= DATA_AT + self.content.len() as u64 {
            buffer.copy_from_slice(
                &self.content[(offset - DATA_AT) as usize..(end - DATA_AT) as usize],
            );
        } else {
            return Err(FoldError::Io("unmapped invented source region"));
        }
        Ok(())
    }
}

#[test]
fn allocated_data_above_four_gib_uses_bounded_direct_reads() {
    let mut bytes = fixture();
    for sector in [8usize, 2056] {
        let base = sector * 512;
        bytes[base + 784..base + 792].copy_from_slice(&(LENGTH / (256 * 512)).to_le_bytes());
        let end = base
            + 752
            + u32::from_le_bytes(bytes[base + 124..base + 128].try_into().unwrap()) as usize * 8;
        let crc = fs_bcachefs::csum::crc32c_nonzero(&bytes[base + 16..end]);
        bytes[base..base + 4].copy_from_slice(&crc.to_le_bytes());
    }
    let allocation = key_at(&bytes, 3, 0, 19);
    bytes[allocation + 24..allocation + 32].copy_from_slice(&(DATA_AT / (256 * 512)).to_le_bytes());
    stamp_node(&mut bytes, 3);
    let extent = key_at(&bytes, 0, 5004, 2);
    let pointer = 1 | (DATA_AT / 512) << 4 | 3u64 << 56;
    bytes[extent + 48..extent + 56].copy_from_slice(&pointer.to_le_bytes());
    stamp_node(&mut bytes, 0);
    let content = bytes[4100 * 512..4102 * 512].to_vec();
    let transferred = Arc::new(AtomicU64::new(0));
    let source = LargeSource {
        prefix: bytes,
        content,
        transferred: Arc::clone(&transferred),
    };
    let mut cache = BlockCache::new(0);
    let context = &mut SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut volume = Bcachefs::open(source, context).unwrap();
    let file = volume.lookup(0, "checksum.bin", context).unwrap().unwrap();
    let before = transferred.load(Ordering::Relaxed);
    let mut data = [0; 14];
    assert_eq!(volume.read_at(file, 499, &mut data, context).unwrap(), 14);
    assert_eq!(
        data,
        std::array::from_fn(|index| (((499 + index) * 17 + 3) % 256) as u8)
    );
    assert!(transferred.load(Ordering::Relaxed) - before < 32 * 1024);
}
