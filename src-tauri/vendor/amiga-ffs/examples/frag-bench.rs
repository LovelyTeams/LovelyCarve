//! Build ADFs that differ only in block layout, for measuring what layout
//! actually costs a real ROM's FFS.
//!
//! All volumes are `DOS\3`, all contain one file of identical bytes.
//! In `contiguous.adf` its data blocks are one ascending run; in
//! `fragmented.adf` the same file is threaded through holes left by
//! deleting every other file of a filler set, which is what a volume
//! that has been lived in looks like. `defragmented.adf` is
//! `fragmented.adf` run through wave 2's `Mutator::defragment_file`, by
//! hand. `reorged.adf` is `fragmented.adf`'s own file copied out, deleted
//! and recreated through `Mutator` with wave 3's layout policy on (the
//! default) and nothing else done by hand — PLAN.md's "Passive
//! reorganisation" closing Aminet's `PFS2DefragTry` (Martin Steigerwald,
//! 1998, crediting Simon for the idea) loop: that copy-out-copy-back
//! trick does not defragment stock FFS, whose allocator has no notion of
//! contiguous runs to seek (`docs/layout-survey.md` §4), but does work
//! through this crate, which is the writer and chooses placement rather
//! than hoping.
//!
//! `cargo run --example frag-bench -- outdir`
//!
//! Prints each file's run structure, so the experiment can state what it
//! actually compared rather than assuming the builder cooperated.

use amiga_ffs::format::FormatOptions;
use amiga_ffs::mutate::Mutator;
use amiga_ffs::populate::{Metadata, Populator};
use amiga_ffs::{BlockSink, BlockSource, Variant, Volume};

const BLOCKS: u64 = 1760; // DD floppy
const PAYLOAD: usize = 200_000; // big enough for the difference to show

struct MemDisk(Vec<u8>);

impl BlockSource for MemDisk {
    type Error = std::convert::Infallible;
    fn block_size(&self) -> usize {
        512
    }
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let o = lba as usize * 512;
        buf.copy_from_slice(&self.0[o..o + 512]);
        Ok(())
    }
    fn block_count(&self) -> Option<u64> {
        Some((self.0.len() / 512) as u64)
    }
}
impl BlockSink for MemDisk {
    type Error = std::convert::Infallible;
    fn block_size(&self) -> usize {
        512
    }
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
        let o = lba as usize * 512;
        self.0[o..o + 512].copy_from_slice(buf);
        Ok(())
    }
    fn block_count(&self) -> Option<u64> {
        Some((self.0.len() / 512) as u64)
    }
}

fn payload() -> Vec<u8> {
    // Content is irrelevant to layout but must be identical in both
    // images, so the only variable is where the blocks sit.
    (0..PAYLOAD).map(|i| (i % 251) as u8).collect()
}

fn blank() -> MemDisk {
    MemDisk(vec![0u8; (BLOCKS * 512) as usize])
}

/// The full sequence of blocks a streaming read actually touches, in
/// fetch order: data blocks, with each `T_LIST` extension block spliced
/// in exactly where a reader fetches it -- between the last data block
/// of the table it closes and the first data block of the table it
/// opens. Counting runs over `FileChain::blocks` alone is a metric that
/// can lie: an extension block placed *between* two data blocks in LBA
/// order reads as a broken run even when nothing is out of place on
/// disk, and (as this experiment exists to check) an extension block
/// placed *away* from the stream costs real seeks that a data-pointer-
/// only count would miss entirely. This is what a "1 run" claim has to
/// mean instead: the head never seeks, table boundary or not.
fn read_path_sequence(chain: &amiga_ffs::FileChain, block_size: usize) -> Vec<u32> {
    let slots = amiga_ffs::hash_table_size(block_size) as usize;
    let mut out = Vec::with_capacity(chain.blocks.len() + chain.extensions.len());
    for (i, &b) in chain.blocks.iter().enumerate() {
        out.push(b);
        // A table has `slots` data pointers; the block fetched right
        // after the last one in a table is the extension block that
        // opens the next table, if there is one.
        if (i + 1) % slots == 0 {
            let ext_index = (i + 1) / slots - 1;
            if let Some(&ext) = chain.extensions.get(ext_index) {
                out.push(ext);
            }
        }
    }
    out
}

/// Describe a file's on-disk read-path layout: how many ascending runs
/// it takes (data blocks and extension blocks together, in fetch order),
/// and how far the head would travel reading it in order.
fn describe(disk: MemDisk, name: &[u8]) -> (MemDisk, String) {
    let mut vol = Volume::open(disk, None).expect("open");
    let root = vol.root_lba();
    let entry = vol.lookup(root, name).expect("lookup").expect("present");
    let chain = vol.file_chain(entry.lba).expect("chain");
    let blocks = read_path_sequence(&chain, vol.block_size());

    let mut runs = 1usize;
    let mut travel = 0u64;
    for w in blocks.windows(2) {
        if w[1] != w[0] + 1 {
            runs += 1;
        }
        travel += (w[1] as i64 - w[0] as i64).unsigned_abs();
    }
    let desc = format!(
        "{} data blocks + {} ext block(s), {} run(s) in fetch order, first {}, last {}, \
         total head travel {} blocks",
        chain.blocks.len(),
        chain.extensions.len(),
        runs,
        blocks.first().copied().unwrap_or(0),
        blocks.last().copied().unwrap_or(0),
        travel
    );
    assert!(vol.validate().findings.is_empty(), "volume must be clean");
    (vol.into_inner(), desc)
}

/// The volume this experiment cares about, built the way an image
/// actually gets built: [`Populator`], not [`Mutator`]. Wave 1 of the
/// block layout policy (`docs/layout-survey.md` §6a and its wave-1
/// addendum, `src/populate.rs`'s "Block layout policy" section) puts
/// `Populator`'s file-data cursor to work here: the header lands near the
/// root with the rest of its directory's metadata, and every data block
/// *and* `T_LIST` extension block of the file is one contiguous physical
/// run, extension blocks interleaved at their natural position in the
/// stream rather than pulled away to the root cluster -- measured both
/// ways through a real ROM, and interleaved won. `Mutator`'s own
/// placement is unchanged by this wave (it is wave 2/3 territory --
/// passive reorganisation on existing volumes), so building the
/// *contiguous* baseline through it would still show the fragmented
/// number the survey originally measured, not the policy this crate now
/// states.
fn build_contiguous() -> MemDisk {
    let disk = blank();
    let opts = FormatOptions::new(Variant::FfsIntl, BLOCKS, b"Contig");
    let mut pop = Populator::new(disk, &opts).expect("populate");
    let root = pop.root_lba();
    pop.create_file(root, b"Payload", &Metadata::new(), &payload())
        .expect("create");
    pop.finish().expect("finish")
}

/// A volume with one large file threaded through holes left by deleting
/// every other file of a filler set.
///
/// Layout policy off, deliberately: wave 3 gave `Mutator::create_file` an
/// `allocate_run` that would otherwise skip straight past the small holes
/// this function threads the payload through and grab a large clean run
/// instead of a fragmented one — correct behaviour for a real caller,
/// wrong for a fixture whose whole job is to build something fragmented.
/// See `build_reorged` for where the policy is deliberately back on.
fn build_fragmented() -> MemDisk {
    let mut disk = blank();
    let opts = FormatOptions::new(Variant::FfsIntl, BLOCKS, b"Frag");
    amiga_ffs::format(&mut disk, &opts).expect("format");
    let vol = Volume::open(disk, None).expect("open");
    let root = vol.root_lba();
    let mut m = Mutator::open(vol).expect("mutator").layout_policy(false);

    // Lay down a filler set, then punch every other one out. The holes
    // are what the payload will be threaded through: this is wear, not
    // sabotage — it is the shape a volume takes after months of use.
    let filler = vec![0u8; 2_048];
    let mut names = Vec::new();
    for i in 0..180 {
        let name = format!("F{i:03}");
        m.create_file(root, name.as_bytes(), &Metadata::new(), &filler)
            .expect("filler");
        names.push(name);
    }
    for (i, name) in names.iter().enumerate() {
        if i % 2 == 0 {
            m.delete(root, name.as_bytes()).expect("delete");
        }
    }
    m.create_file(root, b"Payload", &Metadata::new(), &payload())
        .expect("create");
    m.into_volume().into_inner()
}

/// The fragmented image, run through wave 2's tier-1 compaction
/// (`Mutator::defragment_file`), so the same `describe` this file already
/// uses to report a run count can report it again afterwards. This is
/// the "defragment, then measure" pairing PLAN.md's compaction entry
/// asks for: boot `defragmented.adf` under Copperline with the same
/// script this file's own module documentation describes for
/// `contiguous.adf`/`fragmented.adf`, and the timing should land back
/// near the contiguous number, not the fragmented one -- the real-ROM
/// half of the claim this crate's compactor makes.
fn build_defragmented() -> MemDisk {
    let disk = build_fragmented();
    let mut vol = Volume::open(disk, None).expect("open");
    let root = vol.root_lba();
    let header = vol
        .lookup(root, b"Payload")
        .expect("lookup")
        .expect("present")
        .lba;
    let mut m = Mutator::open(vol).expect("mutator");
    let report = m.defragment_file(header).expect("defragment_file");
    eprintln!(
        "defragment_file: {} runs before, {} after, {} blocks relocated",
        report.runs_before, report.runs_after, report.blocks_relocated
    );
    m.into_volume().into_inner()
}

/// The PFS2DefragTry pattern itself: `build_fragmented`'s own file,
/// copied out to host memory, deleted, and recreated with the same bytes
/// — nothing else done by hand, and the layout policy left at its
/// default (on). This is wave 3's whole point, and the closed loop this
/// module's own documentation describes: the trick `docs/layout-survey.md`
/// §4 says does not defragment stock FFS works through this crate,
/// because this crate is the writer and `Mutator::create_file` chooses
/// placement instead of leaving it to a rover that was never contiguity-
/// aware to begin with.
fn build_reorged() -> MemDisk {
    let disk = build_fragmented();
    let mut vol = Volume::open(disk, None).expect("open");
    let root = vol.root_lba();
    let bytes = {
        let header = vol
            .lookup(root, b"Payload")
            .expect("lookup")
            .expect("present")
            .lba;
        vol.read_file(header).expect("read")
    };
    // Layout policy at its default (on): this is the one build in this
    // file where that matters, and where leaving it on is the point.
    let mut m = Mutator::open(vol).expect("mutator");
    m.delete(root, b"Payload").expect("delete");
    m.create_file(root, b"Payload", &Metadata::new(), &bytes)
        .expect("recreate");
    m.into_volume().into_inner()
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: frag-bench <outdir>");
    std::fs::create_dir_all(&dir).expect("mkdir");

    for (label, disk) in [
        ("contiguous", build_contiguous()),
        ("fragmented", build_fragmented()),
        ("defragmented", build_defragmented()),
        ("reorged", build_reorged()),
    ] {
        let (disk, desc) = describe(disk, b"Payload");
        println!("{label}: {desc}");
        std::fs::write(format!("{dir}/{label}.adf"), &disk.0).expect("write");
    }
    println!("payload is {PAYLOAD} bytes in every image");
}
