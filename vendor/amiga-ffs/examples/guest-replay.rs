//! The guest leg of the differential mutation suite: replay the exact
//! operation sequence a real ROM's FFS performed on one copy of a volume
//! against a pristine copy through this crate's `Mutator`, then compare
//! the two volumes logically — tree shape, names, kinds, sizes, every
//! byte of every file. Dates and block numbers are the documented
//! don't-cares: two allocators may place blocks differently and still
//! agree about every fact a consumer can observe.
//!
//! `cargo run --example guest-replay -- guest-mutated.adf pristine.adf`

use amiga_ffs::mutate::Mutator;
use amiga_ffs::populate::Metadata;
use amiga_ffs::{BlockSink, BlockSource, EntryKind, Variant, Volume};

#[derive(Clone)]
struct MemDisk(Vec<u8>);

impl BlockSource for MemDisk {
    type Error = std::convert::Infallible;
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
    type Error = std::convert::Infallible;
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

/// (path, kind, content) triples, sorted — the observable facts.
fn tree(vol: &mut Volume<MemDisk>, dir: u64, prefix: &str, out: &mut Vec<(String, String)>) {
    for e in vol.read_dir(dir).expect("read_dir") {
        let name: String = e.name.iter().map(|&b| char::from(b)).collect();
        let path = format!("{prefix}/{}", name.to_lowercase());
        match e.kind {
            EntryKind::Directory => {
                out.push((path.clone(), "dir".into()));
                tree(vol, e.lba, &path, out);
            }
            EntryKind::File => {
                let content = vol.read_file(e.lba).expect("read_file");
                out.push((path, format!("file {} {:x?}", content.len(), fnv(&content))));
            }
            other => out.push((path, format!("{other:?}"))),
        }
    }
}

fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

fn facts(disk: MemDisk) -> Vec<(String, String)> {
    let mut vol = Volume::open(disk, Some(Variant::FfsIntl)).expect("open");
    assert!(
        vol.validate().findings.is_empty(),
        "volume must validate clean"
    );
    let root = vol.root_lba();
    let mut out = Vec::new();
    tree(&mut vol, root, "", &mut out);
    out.sort();
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let guest_path = args
        .next()
        .expect("usage: guest-replay <guest.adf> <pristine.adf>");
    let pristine_path = args.next().expect("need pristine.adf");

    let guest = MemDisk(std::fs::read(&guest_path).expect("read guest image"));

    // Replay, through this crate, what the guest's shell did:
    //   makedir df1:guestdir
    //   copy df1:readme df1:guestdir/rm2
    //   rename df1:readme df1:renamed
    //   delete df1:proof/extension-crosser
    //   echo hello >df1:hw
    let ours = MemDisk(std::fs::read(&pristine_path).expect("read pristine image"));
    let vol = Volume::open(ours, Some(Variant::FfsIntl)).expect("open pristine");
    let root = vol.root_lba();
    let mut m = Mutator::open(vol).expect("mutator");
    let md = Metadata::new();
    m.create_dir(root, b"guestdir", &md).expect("makedir");
    let readme = m
        .volume()
        .lookup(root, b"readme")
        .expect("lookup")
        .expect("readme exists");
    let content = m.volume().read_file(readme.lba).expect("read");
    let gd = m
        .volume()
        .lookup(root, b"guestdir")
        .expect("lookup")
        .expect("guestdir");
    m.create_file(gd.lba, b"rm2", &md, &content).expect("copy");
    m.rename(root, b"readme", root, b"renamed").expect("rename");
    let proof = m
        .volume()
        .lookup(root, b"proof")
        .expect("lookup")
        .expect("proof");
    m.delete(proof.lba, b"extension-crosser").expect("delete");
    m.create_file(root, b"hw", &md, b"hello\n").expect("echo");
    let ours = m.into_volume().into_inner();

    let a = facts(guest);
    let b = facts(ours);
    if a == b {
        println!(
            "MATCH: {} entries agree (tree, kinds, sizes, content hashes)",
            a.len()
        );
        for (p, f) in &a {
            println!("  {p}  {f}");
        }
    } else {
        println!("MISMATCH");
        for (p, f) in &a {
            println!("guest: {p} {f}");
        }
        for (p, f) in &b {
            println!("ours:  {p} {f}");
        }
        std::process::exit(1);
    }
}
