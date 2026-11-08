//! Generate the fuzzer's seed corpus: one populated volume per variant,
//! at each block size the suite uses.
//!
//! ```text
//! cargo run --example seed-corpus -- fuzz/corpus/fuzz_read
//! ```
//!
//! The corpus is **generated, not committed**, for the same reason no ADF
//! is checked into `tests/`: a binary blob in a repository is a thing
//! nobody can review, and one that drifts out of step with the writer
//! that made it is worse than no seed at all. `fuzz/corpus` is
//! `.gitignore`d and this example is how it comes back.
//!
//! The first byte of each file is the geometry byte `fuzz_read` reads —
//! the block size is a mount parameter and is not in the image — followed
//! by a variant byte and then the volume itself, so a seed is a valid
//! input rather than merely a valid image.

use std::path::PathBuf;

use amiga_ffs::populate::Populator;
use amiga_ffs::{BlockSink, BlockSource, FormatOptions, Metadata, Variant};

const VARIANTS: [Variant; 8] = [
    Variant::Ofs,
    Variant::Ffs,
    Variant::OfsIntl,
    Variant::FfsIntl,
    Variant::OfsIntlDircache,
    Variant::FfsIntlDircache,
    Variant::OfsIntlLongname,
    Variant::FfsIntlLongname,
];

struct MemDisk {
    bs: usize,
    data: Vec<u8>,
}

#[derive(Debug)]
struct Oops(&'static str);

impl std::fmt::Display for Oops {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Oops {}

impl BlockSource for MemDisk {
    type Error = Oops;
    fn block_size(&self) -> usize {
        self.bs
    }
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Oops> {
        let off = lba as usize * self.bs;
        if off + self.bs > self.data.len() {
            return Err(Oops("read past end"));
        }
        buf.copy_from_slice(&self.data[off..off + self.bs]);
        Ok(())
    }
    fn block_count(&self) -> Option<u64> {
        Some(self.data.len() as u64 / self.bs as u64)
    }
}

impl BlockSink for MemDisk {
    type Error = Oops;
    fn block_size(&self) -> usize {
        self.bs
    }
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), Oops> {
        let off = lba as usize * self.bs;
        if off + self.bs > self.data.len() {
            return Err(Oops("write past end"));
        }
        self.data[off..off + self.bs].copy_from_slice(buf);
        Ok(())
    }
    fn block_count(&self) -> Option<u64> {
        Some(self.data.len() as u64 / self.bs as u64)
    }
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 7 + i / 251) % 251) as u8).collect()
}

fn main() {
    let dir: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fuzz/corpus/fuzz_read".into())
        .into();
    std::fs::create_dir_all(&dir).expect("corpus directory");

    for (v, variant) in VARIANTS.into_iter().enumerate() {
        for (b, bs) in [512usize, 1024, 4096].into_iter().enumerate() {
            let nblocks = 262_144 / bs as u64;
            let disk = MemDisk {
                bs,
                data: vec![0u8; bs * nblocks as usize],
            };
            let opts = FormatOptions::new(variant, nblocks, b"Seed");
            let mut pop = Populator::new(disk, &opts).expect("format");
            let root = pop.root_lba();

            let meta = Metadata::new().comment(b"a seed comment");
            let sub = pop.create_dir(root, b"S", &meta).expect("dir");
            pop.create_file(sub, b"Startup-Sequence", &meta, b"Echo \"seed\"\n")
                .expect("file");
            pop.create_file(root, b"empty", &Metadata::new(), b"")
                .expect("file");
            // Past one header's pointer table at 512 bytes a block, so a
            // seed exercises the extension-block path from the start.
            pop.create_file(root, b"big.dat", &Metadata::new(), &pattern(40_000))
                .expect("file");
            if variant.has_long_names() {
                let long = vec![b'n'; 100];
                let comment = vec![b'c'; 79];
                pop.create_file(root, &long, &Metadata::new().comment(&comment), b"lnfs")
                    .expect("file");
            }
            let disk = pop.finish().expect("finish");

            let mut out = Vec::with_capacity(2 + disk.data.len());
            out.push(b as u8); // the block size the caller would supply
            out.push(v as u8); // the variant to fall back on
            out.extend_from_slice(&disk.data);
            let path = dir.join(format!("dos{}-{bs}.bin", variant.dostype() & 0xFF));
            std::fs::write(&path, &out).expect("write seed");
            println!("{} ({} bytes)", path.display(), out.len());
        }
    }
}
