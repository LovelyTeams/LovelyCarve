# VxFS reader origin

The reader adapts the MIT `fs-unix` 0.2.1 VxFS read algorithms by Dani Sarfati, [revision `4969de813a1ec4439ccce69a1247c2ecf3f50cb4`](https://github.com/danifunker/rusty-backup/blob/4969de813a1ec4439ccce69a1247c2ecf3f50cb4/crates/fs-unix/src/vxfs.rs). The official crates.io archive has SHA-256 `86b61279b0d3904552f0e900919d79be30cd0d151a7bf2e50437c25233c56a3e`. `LICENSE-MIT` retains the upstream copyright and license. The local crate uses LovelyCarve's AGPL-3.0-only license. The bounded block reader, strict structure checks, metadata and file range adapters are local changes.

The public [Linux v6.12 layouts](https://github.com/torvalds/linux/tree/v6.12/fs/freevxfs) and [upstream HP-UX layout 4 notes](https://github.com/danifunker/rusty-backup/blob/4969de813a1ec4439ccce69a1247c2ecf3f50cb4/docs/vxfs-layout4-write-notes.md) were used as reference. No Linux driver code, foreign runtime, or proprietary filesystem utilities are bundled. Upstream's redistributable crate does not contain its real HP-UX image corpus. Tests here use generated fixtures and issue no claim of native-media equivalence.

`python scripts/make-vxfs-fixtures.py` regenerates the synthetic samples and prints the SHA-256 of their raw bytes. The generator treats the published disk structures as input and does not use the Rust reader to create its expectations.

For a reference comparison, copies with both required object records in one OLT extent were opened with the unmodified upstream library. All three copies agreed on the root and nested entries, inline bytes, link target, file size, and allocated data on both sides of the legacy indirect gap. The upstream read for the zero-address run in the 2 KiB image returned physical block zero, which includes the superblock at byte 1024. The local tests require logical zeroes for this hole.

| Raw image | Size | SHA-256 |
| --- | ---: | --- |
| `le2` | 98,304 | `f750e7d8bfc156bf753b936d5f9352a2cfc0bcd4d0532b39246af5743d259815` |
| `le3` | 196,608 | `97bd98756ac5ce7135cefbf06ddc0de362c7a9aff3b68d30664198a2a90e6091` |
| `be4` | 786,432 | `3d92de1bbd901f2a8c850fe98843a5cc28fd737c0b4fbf39acee0a1990812f6f` |
