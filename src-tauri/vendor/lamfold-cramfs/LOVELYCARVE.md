# LovelyCarve integration

Source: crates.io `lamfold-cramfs 0.1.0`, package VCS commit
`44f0343b2487f65cfb84508bff375fdda18c18a5` from
https://github.com/lamco-admin/lamfold. MIT OR Apache-2.0; original notices kept.

The lamfold dependency points to `../lamfold`. The original `Cramfs`, `BlockSource`
and `FoldFrontend` design and zlib codec are reused. Local hardening adds:

- LE/BE bitfield and pointer decoding, optional 512-byte boot padding, FSID v2
  CRC32 from the superblock with the CRC field zeroed, declared-size/signature/root
  checks, and errors for unknown flags.
- FSID v1 uses the source boundary because its size field is a placeholder and
  there is no CRC.
- Reads respect the source boundary and the declared filesystem length in v2.
- Decode only intersecting 4 KiB pages. Extended pointers support raw pages and
  absolute offsets to raw XIP or length-prefixed zlib pages, with bounds and
  exact length checks. Zero-length spans are holes in v1, or in v2 with HOLES set.
- Directory boundaries, entry count, strict UTF-8, invalid/duplicate names.

CRC32 uses the existing pure Rust crc32fast dependency. The host independently
tests images encoded by test_data/rom-images/generate.py (including BE and holes),
mutated corrupt structures, MBR/TRX containers and range/stream equivalence.
The host rejects ancestor directory reuse and never extracts onto the host.

The extended layout follows the [upstream format notes](https://github.com/npitre/cramfs-tools/blob/a95eecbf899479f8b3d65fb2822c5b7cbcdacfdd/NOTES).
The host tests also use native `mkcramfs` images with boot padding and XIP and
an original FSID v1 volume from the formatter in Linux 2.4.0.
