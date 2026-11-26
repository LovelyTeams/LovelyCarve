use super::*;
use lamfold::{BlockCache, NoVerifier};
use std::{
    io::Read,
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) fn fixture() -> Vec<u8> {
    let mut image = Vec::new();
    flate2::read::ZlibDecoder::new(&include_bytes!("../tests/fixtures/invented.zlib")[..])
        .read_to_end(&mut image)
        .unwrap();
    image
}

struct Source {
    bytes: Vec<u8>,
    fail_at: Arc<AtomicU64>,
    bytes_read: Arc<AtomicU64>,
}
impl BlockSource for Source {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buffer.len() as u64)
            .ok_or(FoldError::Io("overflow"))?;
        let failure = self.fail_at.load(Ordering::Relaxed);
        if offset <= failure && failure < end {
            return Err(FoldError::Io("injected source failure"));
        }
        let bytes = self
            .bytes
            .get(offset as usize..end as usize)
            .ok_or(FoldError::Io("short source"))?;
        buffer.copy_from_slice(bytes);
        self.bytes_read
            .fetch_add(buffer.len() as u64, Ordering::Relaxed);
        Ok(())
    }
}
fn source(bytes: Vec<u8>) -> Source {
    Source {
        bytes,
        fail_at: Arc::new(AtomicU64::new(u64::MAX)),
        bytes_read: Arc::new(AtomicU64::new(0)),
    }
}

#[test]
fn snapshots_aliases_native_inodes_times_ranges_and_checksums() {
    let mut cache = BlockCache::new(0);
    let context = &mut SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut volume = Bcachefs::open(source(fixture()), context).unwrap();
    let entries = volume.read_dir(volume.root(), context).unwrap();
    assert_eq!(entries.len(), 10);
    let hello = volume.lookup(0, "hello.txt", context).unwrap().unwrap();
    assert_eq!(volume.lookup(0, "alias.txt", context).unwrap(), Some(hello));
    assert_eq!(
        volume.lookup(0, "中文\\证据.txt", context).unwrap(),
        Some(hello)
    );
    assert_eq!(volume.inode_number(hello, context).unwrap(), 5000);
    let mut bytes = [0; 100];
    let count = volume.read_at(hello, 5, &mut bytes, context).unwrap();
    assert_eq!(&bytes[..count], b"Rust bcachefs evidence\n");
    let times = volume.timestamps(hello, context).unwrap();
    assert_eq!(
        (times.accessed, times.changed, times.modified, times.created),
        (
            Some((-315619200, 123456789)),
            Some((1700000000, 987654321)),
            Some((1600000000, 456789123)),
            Some((1500000000, 111222333))
        )
    );
    let live = volume.lookup(0, "sv", context).unwrap().unwrap();
    let past = volume.lookup(0, "snap", context).unwrap().unwrap();
    assert_ne!(live, past);
    assert_eq!(
        volume.inode_number(live, context).unwrap(),
        volume.inode_number(past, context).unwrap()
    );
    for (directory, expected) in [(live, b"LIVE"), (past, b"PAST")] {
        let file = volume
            .lookup(directory, "version.txt", context)
            .unwrap()
            .unwrap();
        let count = volume.read_at(file, 0, &mut bytes, context).unwrap();
        assert_eq!(&bytes[..count], expected);
    }
    let symlink = volume.lookup(0, "link", context).unwrap().unwrap();
    assert_eq!(
        volume.read_link(symlink, context).unwrap(),
        Some(b"hello.txt".to_vec())
    );
    let special = volume.lookup(0, "pipe", context).unwrap().unwrap();
    assert!(volume.read_at(special, 0, &mut bytes, context).is_err());
    let checked = volume.lookup(0, "checksum.bin", context).unwrap().unwrap();
    let reflink = volume.lookup(0, "reflink.bin", context).unwrap().unwrap();
    for offset in [0, 499, 512, 513, u64::MAX] {
        let count = volume
            .read_at(checked, offset, &mut bytes, context)
            .unwrap();
        let mut shared = [0; 100];
        assert_eq!(
            volume
                .read_at(reflink, offset, &mut shared, context)
                .unwrap(),
            count
        );
        assert_eq!(&bytes[..count], &shared[..count]);
        for (index, byte) in bytes[..count].iter().enumerate() {
            assert_eq!(*byte, (((offset as usize + index) * 17 + 3) % 256) as u8);
        }
    }
}

#[test]
fn sparse_and_unwritten_reads_are_bounded_and_faults_cannot_be_hidden() {
    let mut cache = BlockCache::new(0);
    let context = &mut SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let source = source(fixture());
    let reads = Arc::clone(&source.bytes_read);
    let fail_at = Arc::clone(&source.fail_at);
    let mut volume = Bcachefs::open(source, context).unwrap();
    let sparse = volume.lookup(0, "sparse.bin", context).unwrap().unwrap();
    let mut bytes = [0xff; 80];
    let before = reads.load(Ordering::Relaxed);
    assert_eq!(
        volume
            .read_at(sparse, 1024 * 1024 - 40, &mut bytes, context)
            .unwrap(),
        80
    );
    assert_eq!(bytes, [0; 80]);
    assert!(reads.load(Ordering::Relaxed) - before < 128 * 1024);
    let tail = 5 * 1024u64.pow(3);
    assert_eq!(
        volume.read_at(sparse, tail, &mut bytes, context).unwrap(),
        37
    );
    assert_eq!(&bytes[..5], b"TAIL!");
    fail_at.store(4096 * 512, Ordering::Relaxed);
    assert!(volume
        .read_at(sparse, tail, &mut bytes, context)
        .unwrap_err()
        .to_string()
        .contains("injected source failure"));
    fail_at.store(u64::MAX, Ordering::Relaxed);
    assert_eq!(
        volume
            .read_at(sparse, tail + 37, &mut bytes, context)
            .unwrap(),
        0
    );
}

#[test]
fn bad_checksums_backup_copy_and_truncated_source() {
    let mut cache = BlockCache::new(0);
    let context = &mut SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut bytes = fixture();
    bytes[4096] ^= 1;
    assert!(Bcachefs::probe(&mut source(bytes.clone())).unwrap());
    let mut volume = Bcachefs::open(source(bytes.clone()), context).unwrap();
    let checked = volume.lookup(0, "checksum.bin", context).unwrap().unwrap();
    bytes[4100 * 512] ^= 1;
    let mut corrupt = Bcachefs::open(source(bytes.clone()), context).unwrap();
    let corrupt_checked = corrupt.lookup(0, "checksum.bin", context).unwrap().unwrap();
    assert!(corrupt
        .read_at(corrupt_checked, 0, &mut [0; 50], context)
        .is_err());
    assert_eq!(
        volume.read_at(checked, 0, &mut [0; 50], context).unwrap(),
        50
    );
    bytes.truncate(bytes.len() - 512);
    assert!(Bcachefs::open(source(bytes), context).is_err());
    let mut broken = fixture();
    broken[1024 * 512 + 170] ^= 1;
    assert!(Bcachefs::open(source(broken), context).is_err());
}

#[test]
fn a_checked_extent_can_use_a_replica_on_another_member() {
    let mut image = fixture();
    let extent = key_at(&image, 0, 5004, 2);
    let end = extent + usize::from(image[extent]) * 8;
    let mut pointer = <[u8; 8]>::try_from(&image[end - 8..end]).unwrap();
    pointer[6] = 1;
    insert_node_bytes(&mut image, 0, end, &pointer);
    image[extent] += 1;
    stamp_node(&mut image, 0);

    let first = key_at(&image, 3, 0, 16);
    let last = key_at(&image, 3, 0, 19);
    let mut allocation = image[first..first + usize::from(image[first]) * 8].to_vec();
    allocation[32] = 1;
    let end = last + usize::from(image[last]) * 8;
    insert_node_bytes(&mut image, 3, end, &allocation);
    stamp_node(&mut image, 3);

    let mut replica = image.clone();
    for (device, member) in [&mut image, &mut replica].into_iter().enumerate() {
        for sector in [8, 2056] {
            let base = sector * 512;
            let words = u32::from_le_bytes(member[base + 124..base + 128].try_into().unwrap());
            let end = base + SB_HEADER_BYTES + words as usize * 8;
            let table = base + SB_HEADER_BYTES;
            let first = table + 16;
            let second = first + 48;
            member.copy_within(second..end, second + 48);
            member.copy_within(first..second, second);
            member[second] ^= 1;
            member[table..table + 4].copy_from_slice(&14u32.to_le_bytes());
            member[base + 122] = device as u8;
            member[base + 123] = 2;
            member[base + 124..base + 128].copy_from_slice(&(words + 6).to_le_bytes());
            let csum = fs_bcachefs::csum::crc32c_nonzero(&member[base + 16..end + 48]);
            member[base..base + 4].copy_from_slice(&csum.to_le_bytes());
        }
    }

    let mut cache = BlockCache::new(0);
    let context = &mut SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let data = 4100 * 512;
    let device = source(image.clone());
    device.fail_at.store(data as u64, Ordering::Relaxed);
    let mut faulted = Bcachefs::open_members(vec![device, source(replica.clone())]).unwrap();
    let file = faulted.lookup(0, "checksum.bin", context).unwrap().unwrap();
    assert!(faulted
        .read_at(file, 0, &mut [0; 30], context)
        .unwrap_err()
        .to_string()
        .contains("injected source failure"));

    image[data] ^= 1;
    let mut volume =
        Bcachefs::open_members(vec![source(replica.clone()), source(image.clone())]).unwrap();
    let file = volume.lookup(0, "checksum.bin", context).unwrap().unwrap();
    let mut bytes = [0; 30];
    assert_eq!(volume.read_at(file, 500, &mut bytes, context).unwrap(), 13);
    assert_eq!(bytes[..13], replica[data + 500..data + 513]);

    replica[data] ^= 1;
    let mut damaged = Bcachefs::open_members(vec![source(image), source(replica)]).unwrap();
    let file = damaged.lookup(0, "checksum.bin", context).unwrap().unwrap();
    assert!(damaged
        .read_at(file, 500, &mut bytes, context)
        .unwrap_err()
        .to_string()
        .contains("checksum mismatch"));
}

fn insert_node_bytes(image: &mut [u8], tree_index: usize, at: usize, bytes: &[u8]) {
    let base = (1024 + tree_index * 64) * 512;
    let words = u16::from_le_bytes(image[base + 158..base + 160].try_into().unwrap());
    let end = base + 160 + usize::from(words) * 8;
    image.copy_within(at..end, at + bytes.len());
    image[at..at + bytes.len()].copy_from_slice(bytes);
    let new_words = words + (bytes.len() / 8) as u16;
    image[base + 158..base + 160].copy_from_slice(&new_words.to_le_bytes());
}

pub(super) fn key_at(bytes: &[u8], tree_index: usize, inode: u64, offset: u64) -> usize {
    let base = (1024 + tree_index * 64) * 512;
    let end = base
        + 160
        + u16::from_le_bytes(bytes[base + 158..base + 160].try_into().unwrap()) as usize * 8;
    let mut at = base + 160;
    while at < end {
        if u64::from_le_bytes(bytes[at + 32..at + 40].try_into().unwrap()) == inode
            && u64::from_le_bytes(bytes[at + 24..at + 32].try_into().unwrap()) == offset
        {
            return at;
        }
        at += bytes[at] as usize * 8;
    }
    panic!("fixture key missing");
}

pub(super) fn stamp_node(bytes: &mut [u8], tree_index: usize) {
    let base = (1024 + tree_index * 64) * 512;
    let end = base
        + 160
        + u16::from_le_bytes(bytes[base + 158..base + 160].try_into().unwrap()) as usize * 8;
    let checksum = fs_bcachefs::csum::crc32c_nonzero(&bytes[base + 16..end]);
    bytes[base..base + 4].copy_from_slice(&checksum.to_le_bytes());
}

#[test]
fn valid_crc_cannot_hide_stale_allocation_snapshot_cycles_or_source_faults() {
    let mut cache = BlockCache::new(0);
    let context = &mut SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut bytes = fixture();
    let at = key_at(&bytes, 3, 0, 16);
    bytes[at + 40 + 12] ^= 1;
    stamp_node(&mut bytes, 3);
    let mut stale = Bcachefs::open(source(bytes), context).unwrap();
    let checked = stale.lookup(0, "checksum.bin", context).unwrap().unwrap();
    assert!(stale
        .read_at(checked, 0, &mut [0; 1], context)
        .unwrap_err()
        .to_string()
        .contains("stale pointer"));
    let mut bytes = fixture();
    let at = key_at(&bytes, 6, 0, u32::MAX as u64 - 1);
    bytes[at + 40..at + 48].copy_from_slice(&(((u32::MAX - 1) as u64) << 32).to_le_bytes());
    stamp_node(&mut bytes, 6);
    assert!(Bcachefs::open(source(bytes), context)
        .err()
        .unwrap()
        .to_string()
        .contains("cycle"));
    let device = source(fixture());
    device.fail_at.store(4096, Ordering::Relaxed);
    assert!(Bcachefs::open(device, context)
        .err()
        .unwrap()
        .to_string()
        .contains("injected source failure"));
    assert!(!Bcachefs::probe(&mut source(vec![0; 100])).unwrap());
}

#[test]
fn a_reflink_extent_cannot_start_before_logical_zero() {
    let mut bytes = fixture();
    let file_extent = key_at(&bytes, 0, 5005, 2);
    bytes[file_extent + 16..file_extent + 20].copy_from_slice(&1u32.to_le_bytes());
    bytes[file_extent + 24..file_extent + 32].copy_from_slice(&1u64.to_le_bytes());
    bytes[file_extent + 40..file_extent + 48].fill(0);
    stamp_node(&mut bytes, 0);
    let shared = key_at(&bytes, 4, 0, 102);
    bytes[shared + 24..shared + 32].copy_from_slice(&1u64.to_le_bytes());
    stamp_node(&mut bytes, 4);
    let mut cache = BlockCache::new(0);
    let context = &mut SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut volume = Bcachefs::open(source(bytes), context).unwrap();
    let file = volume.lookup(0, "reflink.bin", context).unwrap().unwrap();
    // The stored data CRC is still correct and a one-byte window would fit
    // the physical payload. Its shared extent nevertheless starts at -1.
    assert!(volume.read_at(file, 0, &mut [0; 1], context).is_err());
}
