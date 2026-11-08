//! Build the ADF the guest-mount proof feeds to a real Amiga ROM:
//! a `DOS\3` DD floppy formatted and populated entirely by this crate.
//!
//! `cargo run --example guest-adf -- /path/to/out.adf`

use std::io::Write as _;

use amiga_ffs::format::FormatOptions;
use amiga_ffs::populate::{Metadata, Populator};
use amiga_ffs::read::DateStamp;
use amiga_ffs::{BlockSink, BlockSource, Variant};

struct MemDisk(Vec<u8>);

impl BlockSource for MemDisk {
    type Error = core::convert::Infallible;
    fn block_size(&self) -> usize {
        512
    }
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let off = lba as usize * 512;
        buf.copy_from_slice(&self.0[off..off + 512]);
        Ok(())
    }
    fn block_count(&self) -> Option<u64> {
        Some((self.0.len() / 512) as u64)
    }
}

impl BlockSink for MemDisk {
    type Error = core::convert::Infallible;
    fn block_size(&self) -> usize {
        512
    }
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
        let off = lba as usize * 512;
        self.0[off..off + 512].copy_from_slice(buf);
        Ok(())
    }
    fn block_count(&self) -> Option<u64> {
        Some((self.0.len() / 512) as u64)
    }
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: guest-adf <out.adf>");
    let disk = MemDisk(vec![0u8; 901_120]); // DD floppy, 1760 blocks

    let opts = FormatOptions::new(Variant::FfsIntl, 1760, b"FfsRsProof").created(DateStamp {
        days: 17781,
        mins: 600,
        ticks: 0,
    });
    let mut p = Populator::new(disk, &opts).expect("format+wrap");

    let root = p.root_lba();
    let md = Metadata::new();
    let dir = p.create_dir(root, b"Proof", &md).expect("mkdir");
    p.create_file(
        root,
        b"ReadMe",
        &md.comment(b"written by amiga-ffs-rs"),
        b"This volume was formatted and populated by amiga-ffs-rs.\n",
    )
    .expect("readme");
    // A file big enough to cross into an extension block: the chain the
    // guest has to walk correctly to type it.
    let big: Vec<u8> = (0..40_000u32).map(|i| b'A' + (i % 26) as u8).collect();
    p.create_file(dir, b"Extension-Crosser", &md, &big)
        .expect("big file");
    p.create_file(
        dir,
        b"Caf\xE9-Latin1",
        &md,
        b"a Latin-1 name the intl fold table owns\n",
    )
    .expect("latin1");
    let disk = p.finish().expect("finish");

    let mut f = std::fs::File::create(&out).expect("create output");
    f.write_all(&disk.0).expect("write");
    println!("wrote {} ({} bytes)", out, disk.0.len());
}
