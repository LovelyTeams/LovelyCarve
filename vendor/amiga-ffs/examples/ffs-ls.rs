//! List and validate a volume image: the host end of the guest-mutation
//! differential, and a usable little tool besides.
//!
//! `cargo run --example ffs-ls -- image.adf`

use amiga_ffs::{BlockSource, EntryKind, Volume};

struct FileDisk {
    data: Vec<u8>,
}

impl BlockSource for FileDisk {
    type Error = std::convert::Infallible;
    fn block_size(&self) -> usize {
        512
    }
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let off = lba as usize * 512;
        buf.copy_from_slice(&self.data[off..off + 512]);
        Ok(())
    }
    fn block_count(&self) -> Option<u64> {
        Some((self.data.len() / 512) as u64)
    }
}

fn walk(vol: &mut Volume<FileDisk>, dir: u64, prefix: &str) {
    for e in vol.read_dir(dir).expect("read_dir") {
        let name = String::from_utf8_lossy(
            &e.name
                .iter()
                .flat_map(|&b| char::from(b).to_string().into_bytes())
                .collect::<Vec<_>>(),
        )
        .into_owned();
        match e.kind {
            EntryKind::Directory => {
                println!("{prefix}{name}/");
                walk(vol, e.lba, &format!("{prefix}{name}/"));
            }
            EntryKind::File => {
                let content = vol.read_file(e.lba).expect("read_file");
                println!("{prefix}{name}  {} bytes", content.len());
            }
            other => println!("{prefix}{name}  [{other:?}]"),
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: ffs-ls <image>");
    let data = std::fs::read(&path).expect("read image");
    let disk = FileDisk { data };
    // `None` means "believe the disk": the boot block's dostype decides.
    let mut vol = Volume::open(disk, None).expect("open");
    println!(
        "volume {:?}, {:?}",
        String::from_utf8_lossy(&vol.root().name.clone()),
        vol.variant()
    );
    let root = vol.root_lba();
    walk(&mut vol, root, "");
    let report = vol.validate();
    println!(
        "validate: {} findings{}",
        report.findings.len(),
        if report.findings.is_empty() {
            " (clean)"
        } else {
            ""
        }
    );
    for f in &report.findings {
        println!("  {f}");
    }
}
