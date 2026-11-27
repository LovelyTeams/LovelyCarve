# HPFS reader origin

This is LovelyCarve's AGPL-3.0-only read-only Rust implementation. On-disk layouts were checked against the [Linux HPFS documentation and definitions](https://github.com/torvalds/linux/tree/68a74490629eb606ad77a88dd84515f35525e267/fs/hpfs) and [nepx/hpfsutils](https://github.com/nepx/hpfsutils/tree/ceeb8170a88a11d11e00a5a75deb97f7e2eda93e). No code from the Linux driver or the C utilities is shipped with the reader. The simple OEM character mappings were generated using the standard Python `cpNNN` codecs, with undefined byte values kept invalid. East Asian filename encodings use `encoding_rs`.

## Independent fixture

The fixture was made with `mkhpfs` and `hpfsimg` from the pinned BSD-3-Clause `hpfsutils` source archive with SHA-256 `6cdee7a57daa5d505766e801e293fc8b0b50ff9948b884ad634550fcb53170ee`. A temporary copy of `mkhpfs.c` returned the initialized `fnode` from `mk_fnode` to correct the upstream missing return. A blank boot sector carrying `EB 3C 90` was supplied with `-b` so the independent population tool could verify the BPB. The volume used a 24 MiB MBR partition beginning at LBA 2048. The fixture retains the whole volume starting at its boot sector and excludes the MBR.

The tool was populated with `Read Me.txt`, `empty`, `payload.bin` and `tree/Item 0000.txt` through `tree/Item 0512.txt`. Their expected content lives in the tests. The fixture has its original bytes, compressed with zlib. Allocator tree and hotfix fault-injection tests modify only a private test copy.

| Bytes | Length | SHA-256 |
| --- | --- | --- |
| `tests/fixtures/hpfsutils.zlib` | 38,953 | `049af3a0bb2173d387979a4485b63d59a465d4e1422a12e237e6e72f79213432` |
| Uncompressed volume | 25,165,824 | `8d1486fd2643972176d0a54885889c761e3698d7d45f7b01d8e4b2ddf12655bb` |
