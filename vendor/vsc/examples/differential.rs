//! Differential check against pyvshadow dumps produced by
//! `tests/oracle/random_images.py` (see `tests/oracle/README.md`).
//!
//! Usage: `cargo run --release --example differential -- <dir> <count>`
//!
//! For each `rand_<k>.raw` whose `rand_<k>.status` is `ok <stores>`, every
//! store is reconstructed in full and over random unaligned ranges and compared
//! with `rand_<k>.store<i>.oracle.bin`. An image libvshadow rejected must be
//! rejected here too. Exits non-zero on any difference.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;

use vsc::VssVolume;

/// xorshift64* for reproducible ranges.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, count) = (&args[1], args[2].parse::<usize>().expect("image count"));
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut stores_checked, mut ranges_checked, mut failures) = (0usize, 0usize, 0usize);

    for k in 0..count {
        let status = std::fs::read_to_string(format!("{dir}/rand_{k}.status")).unwrap();
        let image = std::fs::read(format!("{dir}/rand_{k}.raw")).unwrap();
        let mut read_volume = |offset: u64, out: &mut [u8]| -> std::io::Result<()> {
            let start = usize::try_from(offset).unwrap();
            out.copy_from_slice(&image[start..start + out.len()]);
            Ok(())
        };
        let mut volume = match VssVolume::open(Cursor::new(image.clone())) {
            Ok(volume) => volume,
            Err(error) => {
                if status.starts_with("error ") {
                    continue;
                }
                failures += 1;
                println!("rand_{k}: catalog rejected: {error}");
                continue;
            }
        };

        if let Some(message) = status.strip_prefix("error ") {
            let accepted = (0..volume.store_count()).all(|i| volume.snapshot_layout(i).is_ok());
            if accepted {
                failures += 1;
                println!("rand_{k}: libvshadow rejected it ({message}) but vsc accepted it");
            }
            continue;
        }
        let expected: usize = status.trim_start_matches("ok ").trim().parse().unwrap();
        if volume.store_count() != expected {
            failures += 1;
            println!(
                "rand_{k}: {} stores, libvshadow saw {expected}",
                volume.store_count()
            );
            continue;
        }
        for index in 0..expected {
            let oracle = std::fs::read(format!("{dir}/rand_{k}.store{index}.oracle.bin")).unwrap();
            let layout = match volume.snapshot_layout(index) {
                Ok(layout) => layout,
                Err(error) => {
                    failures += 1;
                    println!("rand_{k} store {index}: rejected: {error}");
                    continue;
                }
            };
            let mut whole = vec![0u8; usize::try_from(layout.volume_size()).unwrap()];
            layout.read_at(0, &mut whole, &mut read_volume).unwrap();
            if whole != oracle {
                failures += 1;
                let first = whole.iter().zip(&oracle).position(|(a, b)| a != b);
                println!("rand_{k} store {index}: volume differs, first byte {first:?}");
                continue;
            }
            for _ in 0..500 {
                let length = usize::try_from(rng.next() % 40_000).unwrap() + 1;
                let offset = rng.next() % u64::try_from(oracle.len() - length).unwrap();
                let start = usize::try_from(offset).unwrap();
                let mut piece = vec![0u8; length];
                layout
                    .read_at(offset, &mut piece, &mut read_volume)
                    .unwrap();
                ranges_checked += 1;
                if piece[..] != oracle[start..start + length] {
                    failures += 1;
                    println!("rand_{k} store {index}: range {offset:#x}+{length} differs");
                    break;
                }
            }
            stores_checked += 1;
        }
    }
    println!("{stores_checked} stores and {ranges_checked} ranges identical, {failures} failures");
    if failures != 0 {
        std::process::exit(1);
    }
}
