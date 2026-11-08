# Independent UBIFS fixtures

Original CC0-1.0 data; no third-party firmware or user content is redistributed.
`generate_ubifs.py` encodes the Linux v6.12 UBIFS layouts using Python stdlib.
Run it once, run `node generate_zstd.mjs`, then run it again to regenerate all
fixtures. The independent Node encoder uses libzstd 1.5.7 (Node 24.13.1) and
checksummed compressed frames; the shipped Rust decoder is ms-compress-ruzstd.

The v5 filesystem has a multilevel index, two min-I/O-aligned master copies,
Chinese names, an R5 hash collision (Ak / B`), five paths to one inode, an inline
symlink, an xattr inode, an empty file, signed/nanosecond timestamps, and a
9001-byte sparse file with an absent middle 4096-byte block. PAYLOAD bytes are
generated from an explicit arithmetic pattern, zeros and a literal tail.

Codec variants use stored bytes, independent LZO literal streams, Python raw
DEFLATE, independent Zstandard raw-block frames, and actual compressed,
checksummed Zstandard frames from the separate Node/libzstd encoder. A codec is
allowed to store a block when the independent encoding exceeds UBIFS's block
size bound; other blocks still exercise the named codec.

| File | SHA-256 |
| --- | --- |
| payload.bin | `4c0649d8a5ad1c3a69691a75c5c1a5e0e4c6c00fb18219da56a1b0f777003ced` |
| synthetic-deflate.ubifs | `05c9a608f0af78eb0d3753832338d1f3e1e1b131dd2d78cca561944ff9a0f323` |
| synthetic-lzo.ubifs | `ac8466f6bddefa463ae6a9e0c592f81bdfa5e7b8287444c201243148d0fbc9ef` |
| synthetic-stored.ubifs | `cc808e0890d4e19d61eff14b51e38948737290eaec0833d198238643a16e2562` |
| synthetic-zstd-compressed.ubifs | `f2ab58f1120e975a2615fe41e292f780feb6e4117960825c867c1cfbec39c0db` |
| synthetic-zstd.ubifs | `c77530582a8e638b395d634f4ed1981ea6fcacf947ddfb632814e48ac1a665f9` |
| zstd-0.frame | `bf137cacaa3018b050d06ef016bb53362fba9f2d14301c20cc8b35f1cd2e837d` |
| zstd-2.frame | `92b4d06ea091e4bda9db282c3ea6fce2e31dbe7391255534db905e08c4f24299` |
