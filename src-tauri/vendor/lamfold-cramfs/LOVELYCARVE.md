# LovelyCarve integration

Source: crates.io `lamfold-cramfs 0.1.0`, package VCS commit
`44f0343b2487f65cfb84508bff375fdda18c18a5` from
https://github.com/lamco-admin/lamfold. MIT OR Apache-2.0; original notices kept.

The lamfold dependency points to `../lamfold`. The original `Cramfs`, `BlockSource`
and `FoldFrontend` design and zlib codec are reused. Local hardening adds:

- LE/BE bitfield and pointer decoding, FSID v2 CRC32 with the CRC field zeroed,
  declared-size/signature/root checks, explicit rejection of v1 / XIP / unknown flags.
- Every read bounded by the filesystem's declared size, not host-file padding.
- Decode only intersecting 4 KiB pages; validate monotone block extents, cap each
  compressed span and require exact decoded length. Only declared HOLES read zero.
- Directory boundaries, entry count, strict UTF-8, invalid/duplicate names.

CRC32 uses the existing pure Rust crc32fast dependency. The host independently
tests images encoded by test_data/rom-images/generate.py (including BE and holes),
mutated corrupt structures, MBR/TRX containers and range/stream equivalence.
The host rejects ancestor directory reuse and never extracts onto the host.
