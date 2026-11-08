# nilfs2-reader 0.1.0

Original immutable Rust implementation, AGPL-3.0-only. Existing Rust projects
were researched before implementation. `unearth` only detects NILFS2;
`ahmed-masud/rust-nilfs2` at `d4697e8c8c9310c058f0674729e39aeb95bac0a6`
contains an empty Rust library, a bindgen build dependency and bundled C tools.
`btree-ondisk` is a generic, modified B-tree engine, with separate node flags
and host-memory representations; it is not a NILFS2 image/metadata codec.
None provides a complete directly reusable pure Rust NILFS2 image reader.

The primary superblock prefix positions are adapted from Marcel Roozekrans's
MIT `unearth` at `bc891261221f5f5134269780d6c206725b21d52e/src/nilfs2.rs`.
Its original notice is retained as LICENSE-UNEARTH. New checked parsing uses
format facts from Linux `7a78f7ba9f24962c8033e0152bf34a7903ff61ba`,
`include/uapi/linux/nilfs2_ondisk.h` and `fs/nilfs2/`. No C parser is compiled,
linked or required by the product. Runtime dependencies crc32fast and lamfold
are pure Rust. Capabilities and explicit limits are recorded in
[NILFS2.md](../../docs/formats/NILFS2.md).

## Independent native verification

`scripts/nilfs2-native-fixtures.py` creates new regular images on a disposable
Linux host. Native nilfs-tools 2.2.9-1 and Linux 6.1.0-54-amd64 format, write,
checkpoint and read back the current tree and two retained snapshots. All
contents are invented CC0-1.0 data. NILFS2 has no native fsck; no such check is
claimed. `nilfs-tune -l`, `dumpseg`, kernel manifests and raw image hashes are
retained externally. No Rust encoder contributes to these native inputs.

| Block / segment geometry | Decoded image SHA-256 |
| --- | --- |
| 1024 / 2048, block_count | 5f66eaf135371e8cc4216d07d0e70424ceb21e483b3fe53950ee11524007b374 |
| 2048 / 1024, block_count | 4f94604dc564155c0a90ff9a3ca307c8386e32ddb0152d68984153c88d97e594 |
| 4096 / 512, block_count | de184bd52ab7699ccd6407d3543df18b729249ebb83a6e8dba05b4a582b19f4f |
| 4096 / 2048, no features | 431f5aad37156f2eee8485686c4481454eb53b73f03424f28ebbe77a03e52a01 |
| 4096 / 512, after native GC | 7c622b3843ad28124f05e094da9d012d3bb6789be7f3e179f4b6714d3461ab3f |

Each image is 512 MiB. The first has 10018 current names; the others have 1518.
All current and historical names, inode identities, modes, lengths, recorded
dates, symlinks and complete/ranged file contents are checked against kernel
manifests. GC verification starts with a separate copy, keeps both snapshots,
creates disposable overwritten data and runs the native cleaner. Its segment
usage changes and 12 CRC-valid GC summaries independently witness relocation.

Self-encoded compact fixtures are separate, with their own CC0 generator and
hash manifest. They cover 1 / 4 / 64 KiB blocks and a virtual 8 GiB source with
6 GiB physical logs and a 5 GiB sparse file. These do not substitute for native
formatter/kernel verification or prove other architectures, online recovery,
all possible B-tree depths, or actual Windows driver behavior.
