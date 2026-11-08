//! Hex-peek tool: read 4 bytes at a given offset from an image and print them.
use lovelycarve_core::evidence::image::DiskImage;
use std::env;
fn main() -> anyhow::Result<()> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .expect("usage: probe_xfs_magic <image> <offset>");
    let off: u64 = args.next().expect("offset").parse()?;
    let image = DiskImage::open(&path)?;
    let bytes = image.read_range(off, 32)?;
    print!("offset={off} (0x{off:x}) bytes:");
    for b in &bytes {
        print!(" {:02x}", b);
    }
    println!();
    if bytes.len() >= 4 {
        let magic = std::str::from_utf8(&bytes[..4]).unwrap_or("???");
        println!("ascii: {magic:?}");
    }
    Ok(())
}
