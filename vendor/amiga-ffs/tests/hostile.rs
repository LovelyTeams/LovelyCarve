//! Adversarial probes: the image is hostile.
//!
//! These are not fuzz targets — they are specific, hand-built reproducers
//! for suspicions raised during a manual security review, kept here so
//! the review's claims are checkable rather than merely asserted.
//!
//! A `MemDisk` identical in spirit to the fuzz targets' backs every test:
//! strict about buffer length and LBA range, so a test that passes says
//! something about the crate, not about a forgiving harness.

use amiga_ffs::layout::*;
use amiga_ffs::*;

#[derive(Debug)]
pub enum MemError {
    BadBufferLen { got: usize, want: usize },
    OutOfRange { lba: u64, blocks: u64 },
}

impl core::fmt::Display for MemError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MemError {}

pub struct MemDisk {
    bs: usize,
    data: Vec<u8>,
}

impl MemDisk {
    pub fn blank(bs: usize, nblocks: u64) -> Self {
        Self {
            bs,
            data: vec![0u8; bs * nblocks as usize],
        }
    }
    pub fn blocks(&self) -> u64 {
        self.data.len() as u64 / self.bs as u64
    }
    fn raw_block(&self, lba: u64) -> &[u8] {
        let off = lba as usize * self.bs;
        &self.data[off..off + self.bs]
    }
    fn raw_block_mut(&mut self, lba: u64) -> &mut [u8] {
        let off = lba as usize * self.bs;
        &mut self.data[off..off + self.bs]
    }
}

impl BlockSource for MemDisk {
    type Error = MemError;
    fn block_size(&self) -> usize {
        self.bs
    }
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), MemError> {
        if buf.len() != self.bs {
            return Err(MemError::BadBufferLen {
                got: buf.len(),
                want: self.bs,
            });
        }
        if lba >= self.blocks() {
            return Err(MemError::OutOfRange {
                lba,
                blocks: self.blocks(),
            });
        }
        buf.copy_from_slice(self.raw_block(lba));
        Ok(())
    }
    fn block_count(&self) -> Option<u64> {
        Some(self.blocks())
    }
}

impl BlockSink for MemDisk {
    type Error = MemError;
    fn block_size(&self) -> usize {
        self.bs
    }
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), MemError> {
        if buf.len() != self.bs {
            return Err(MemError::BadBufferLen {
                got: buf.len(),
                want: self.bs,
            });
        }
        if lba >= self.blocks() {
            return Err(MemError::OutOfRange {
                lba,
                blocks: self.blocks(),
            });
        }
        self.raw_block_mut(lba).copy_from_slice(buf);
        Ok(())
    }
    fn block_count(&self) -> Option<u64> {
        Some(self.blocks())
    }
}

fn write_checked(disk: &mut MemDisk, lba: u64, mut block: Vec<u8>) {
    let ck = checksum_compute(&block, CHECKSUM_INDEX);
    block[OFF_CHECKSUM..OFF_CHECKSUM + 4].copy_from_slice(&ck.to_be_bytes());
    assert!(checksum_ok(&block));
    disk.write_block(lba, &block).unwrap();
}

/// Build a blank, correctly-formatted volume, then splice `n` extra file
/// header blocks into the root's hash slot 0 as one long chain — a shape
/// that is not a cycle (`guard_chain`'s block_count bound will let it run
/// to completion) but is long, and legal-looking: each block has a good
/// checksum, `T_HEADER`, `ST_FILE`.
///
/// Returns the opened volume and the root LBA.
fn volume_with_long_chain(bs: usize, block_count: u64, n: u64) -> (Volume<MemDisk>, u64) {
    let disk = MemDisk::blank(bs, block_count);
    let opts = FormatOptions::new(Variant::Ffs, block_count, b"Hostile");
    let mut disk = disk;
    let layout = format(&mut disk, &opts).expect("format a blank image");
    let root_lba = layout.root_lba;

    // Blocks used by the format (root + bitmap pages) are off limits;
    // everything else in the volume is free for our raw chain, and
    // `read_dir` never consults the bitmap, so "free" only matters for
    // not clobbering the root/bitmap themselves.
    let mut used: std::collections::HashSet<u64> = layout.allocated().into_iter().collect();
    used.insert(root_lba);
    for r in 0..DEFAULT_RESERVED {
        used.insert(r);
    }
    let mut free = (0..block_count).filter(|b| !used.contains(b));

    let mut chain_lbas = Vec::with_capacity(n as usize);
    for _ in 0..n {
        chain_lbas.push(free.next().expect("enough blocks for the chain"));
    }

    // Link tail -> head, writing hash_chain fields as we go.
    for (i, &lba) in chain_lbas.iter().enumerate() {
        let mut block = vec![0u8; bs];
        wr32(&mut block, OFF_TYPE, T_HEADER);
        wr32(&mut block, OFF_OWN_KEY, lba as u32);
        wr32(&mut block, tail(bs, TL_SECONDARY_TYPE), ST_FILE as u32);
        let next = chain_lbas.get(i + 1).copied().unwrap_or(0);
        wr32(&mut block, tail(bs, TL_HASH_CHAIN), next as u32);
        wr32(&mut block, tail(bs, TL_PARENT), root_lba as u32);
        write_checked(&mut disk, lba, block);
    }

    // Point the root's hash slot 0 at the chain head, and fix the root's
    // checksum so the volume still opens cleanly.
    let mut root_block = vec![0u8; bs];
    disk.read_block(root_lba, &mut root_block).unwrap();
    wr32(&mut root_block, OFF_HASH_TABLE, chain_lbas[0] as u32);
    write_checked(&mut disk, root_lba, root_block);

    let vol = Volume::open_with(disk, None, block_count, DEFAULT_RESERVED)
        .expect("the volume we just built");
    (vol, root_lba)
}

fn wr32(block: &mut [u8], off: usize, v: u32) {
    block[off..off + 4].copy_from_slice(&v.to_be_bytes());
}

/// `read_dir`'s cycle guard (`guard_chain`) used to use a `Vec` for
/// `visited`, so checking whether a block had been seen was
/// O(chain-length-so-far) and walking a chain of `n` distinct,
/// legal-looking blocks was O(n^2), even though the chain has no cycle
/// and terminates. Fixed by switching `visited` to a `BTreeSet`
/// (`read.rs`'s `guard_chain`, and every other chain walk in the crate
/// that had reimplemented the same `Vec`-based pattern inline), which
/// makes each check O(log n) and the whole walk O(n log n).
///
/// This is the same reproducer the earlier review used to demonstrate
/// the bug, with the assertion inverted: it now checks growth stays
/// close to linear (n log n over a 4x range is close enough to linear
/// that a generous bound still catches a regression back to O(n^2)),
/// instead of checking that it doesn't. Still `#[ignore]`d because it
/// deliberately takes real wall-clock time and is a benchmark, not a
/// correctness check — run with `cargo test --test hostile -- --ignored`.
#[test]
#[ignore = "timing-based regression check that the O(n^2) chain walk stays fixed; slow by design"]
fn long_hash_chain_is_quadratic_not_linear() {
    let bs = 512usize;
    let block_count = 40_000u64;

    let small_n = 2_000u64;
    let big_n = 8_000u64; // 4x the length

    let (mut small_vol, small_root) = volume_with_long_chain(bs, block_count, small_n);
    let t0 = std::time::Instant::now();
    let small = small_vol.read_dir(small_root).expect("read_dir");
    let small_time = t0.elapsed();
    assert_eq!(small.len() as u64, small_n);

    let (mut big_vol, big_root) = volume_with_long_chain(bs, block_count, big_n);
    let t0 = std::time::Instant::now();
    let big = big_vol.read_dir(big_root).expect("read_dir");
    let big_time = t0.elapsed();
    assert_eq!(big.len() as u64, big_n);

    // Linear would give ~4x; quadratic gave ~16x (the pre-fix numbers
    // from the review that found this were 26ms at n=2000 vs 242ms at
    // n=8000, a 9.2x ratio for a 4x input). O(n log n) over this range
    // is ~4.4x -- we assert well under quadratic, generously above the
    // O(n log n) prediction so the bound isn't flaky.
    let ratio = big_time.as_secs_f64() / small_time.as_secs_f64().max(1e-9);
    eprintln!(
        "chain {small_n} -> {small_time:?}, chain {big_n} -> {big_time:?}, ratio {ratio:.1}x \
         (linear would be ~4x, O(n log n) ~4.4x, quadratic ~16x)"
    );
    assert!(
        ratio < 8.0,
        "expected close-to-linear growth (guard_chain is O(n log n) now), got {ratio:.1}x for a \
         4x longer chain -- either the fix regressed, or the machine is unusually noisy"
    );
}

/// A cheaper, non-timing version of the same shape: build a chain long
/// enough that a *quadratic* walk would be measurably multi-second on
/// any reasonable machine, and check it now completes well within that
/// bound. Still `#[ignore]`d: it is slow-ish on purpose (proving the
/// bound holds for a big `n`), and the bound is deliberately loose so it
/// isn't a flaky CI gate -- it exists so a human can
/// `cargo test -- --ignored --nocapture` and watch the clock.
#[test]
#[ignore = "regression check for the real-world cost of the (now fixed) O(n^2) walk"]
fn very_long_hash_chain_read_dir_timing() {
    let bs = 512usize;
    let block_count = 60_000u64;
    let n = 20_000u64;
    let (mut vol, root) = volume_with_long_chain(bs, block_count, n);
    let t0 = std::time::Instant::now();
    let entries = vol.read_dir(root).expect("read_dir");
    let elapsed = t0.elapsed();
    assert_eq!(entries.len() as u64, n);
    eprintln!("read_dir over a {n}-entry single hash chain took {elapsed:?}");
    // A quadratic walk over 20,000 blocks would be multi-second (the
    // n=8,000 case above already took ~240ms pre-fix, and this is 2.5x
    // that chain length again squared); a linear-ish one finishes in
    // low tens of milliseconds. One second is a generous ceiling that
    // only a regression back to O(n^2) would come near.
    assert!(
        elapsed.as_secs_f64() < 1.0,
        "read_dir over a {n}-entry chain took {elapsed:?} -- consistent with a regression back \
         to the O(n^2) guard_chain walk"
    );
}
