# amiga-ffs 0.4.0

Simon Dick, Copyright 2026. MIT OR Apache-2.0; original license files retained.
https://github.com/sidick/amiga-ffs-rs
Published crate: https://static.crates.io/crates/amiga-ffs/amiga-ffs-0.4.0.crate
SHA-256: `9fc1d1ea66f80d3a991047832c6e49f66f9e2b2b1fdeb4412345f95b4f73fbd3`.
Published VCS revision: `b774aaa65dcf3017e1a263c369efbcc0d08a3b70`.

The published package is retained with a standalone workspace marker. One
maintained source patch in `src/file.rs` bounds pointer collection by the
logical byte size before vector growth, rejecting oversized hostile chains.
The LovelyCarve adapter implements BlockSource only and cannot invoke the
upstream writer, formatter or repair APIs (which require BlockSink).
