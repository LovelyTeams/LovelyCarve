//! Emit a synthetic VMDK fixture to a file, for cross-validation against qemu-img.
//!
//! Usage: cargo run --example `emit_fixture` --features test-helpers -- <kind> <`out_path`>
//! where <kind> is one of: cowd, sesparse
//!
//! The grain holds a recognisable pattern (bytes `0..grain_size`) so that an
//! independent reader (qemu-img convert -O raw) can be diffed against this crate.

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: emit_fixture <cowd|sesparse> <out_path>");
        std::process::exit(2);
    }
    let kind = &args[1];
    let out_path = &args[2];

    // A recognisable 4 KiB pattern: byte i = (i % 251).
    let pattern: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();

    let bytes = match kind.as_str() {
        "cowd" => vmdk::testutil::test_cowd_vmdk(&pattern),
        "sesparse" => vmdk::testutil::test_sesparse_vmdk(&pattern),
        other => {
            eprintln!("unknown kind: {other}");
            std::process::exit(2);
        }
    };

    let mut f = std::fs::File::create(out_path).expect("create output file");
    f.write_all(&bytes).expect("write fixture");
    eprintln!("wrote {} bytes to {out_path}", bytes.len());
}
