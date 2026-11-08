# gfs2-reader 0.1.0

An immutable pure Rust GFS2 reader. Existing Rust implementations were
searched before implementation (crates.io, GitHub, and docs.rs). `unearth`
provides a detector, not directory/file reading. Its MIT superblock prefix
interpretation is adapted here, with its original notice in LICENSE-UNEARTH:

https://github.com/MarcelRoozekrans/unearth/blob/bc891261221f5f5134269780d6c206725b21d52e/src/gfs2.rs

Local decoding checks geometry, metadata type/format, source boundaries, and
both 1801 and 1802; it does not treat GFS1 (1309) as GFS2. The volume's upper
source bound is distinct from its resource-group allocation ranges.

New Rust parsing is written against the documented on-disk records, with
Linux revision `7a78f7ba9f24962c8033e0152bf34a7903ff61ba` used for format facts:
`include/uapi/linux/gfs2_ondisk.h` and `fs/gfs2/`. No kernel source is compiled,
linked, mounted, or shipped with the product. Original Rust additions use the
repository's AGPL-3.0-only license. See [GFS2.md](../../docs/formats/GFS2.md) for capabilities and limits.


## Native, independent verification

`scripts/gfs2-native-fixtures.py` runs only on a disposable Linux test host.
Native mkfs.gfs2 3.5.0, Linux 6.1.0-54-amd64 and fsck.gfs2 -n generated and
verified four newly created 512 MiB images. All content is invented CC0 data.
No Rust encoder or reader supplied bytes to these native inputs. They remain
external to the repository; the self-encoded compact fixture is separate.

Decoded image SHA-256:

| Block size / format | SHA-256 |
| --- | --- |
| 512 / 1801 | e0c9ddfff4045a57c704ba71d021b5daa8fe651d2ad895aaca70b19f18ab9b44 |
| 1024 / 1802 | 4c0a9bf0cb1761ef10adbda441a9adca24d50e97b2ee40679bbebc16a34069fb |
| 2048 / 1801 | d5cbb468037ac3f6ae3e9f0e6c15b81b5d51de93f861367aa17a6b4499cc962b |
| 4096 / 1802 | d1be3a3c5ea9a835d4ecb6b34198217fd608a402886712c925e84e4662df2613 |

The external JSON manifests record native kernel names, hashes, modes, sizes,
UTC nanosecond dates, symlink targets and all attribute bytes. Native audit
checks all 1512 names per image, complete versus ranged content, and metadata.
These checks do not prove online cluster coordination or recovery.
