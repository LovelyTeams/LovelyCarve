# HAMMER reader provenance

LovelyCarve implements the runtime reader in Rust under AGPL-3.0-only. The on-disk structures and transaction rules were compared with the [DragonFly BSD HAMMER design](https://www.dragonflybsd.org/hammer/hammer.pdf), [kernel disk structures](https://gitweb.dragonflybsd.org/dragonfly.git/?p=dragonfly.git;a=blob_plain;f=sys/vfs/hammer/hammer_disk.h;hb=HEAD), and [kernel recovery logic](https://gitweb.dragonflybsd.org/dragonfly.git/?p=dragonfly.git;a=blob_plain;f=sys/vfs/hammer/hammer_recover.c;hb=HEAD).

The fixtures use [lh1](https://github.com/kusumi/lh1/tree/6eaec82738c72a326d12fdd13f08446c2fda7ba3), the Linux port of the DragonFly tools, from the author's repository. Source archive SHA-256 is `8d7e0290e5af07065297c88e78b2cdd4a11732b5c05bb8038e52ede860107714`. Create a 512 MiB sparse file and format it with `newfs_hammer -f -L LC_H1 -V 7 -b 0m -m 0m -u 16m image` for the empty variant. The two populated variants use label `LC_CONTENT` and `-V 1` or `-V 7`. A fixture-only hook added to `newfs_hammer` inserts records using its own B-tree and media allocators and CRC routines. The v1 tree is a single leaf using legacy directory localization, and the v7 tree has two leaves under an internal root and an explicit snapshot at transaction `0000000100000001`.

Each populated image has `/payload.bin`, another link `/same-inode` to inode 2, and a symlink `/latest` targeting `payload.bin`. The first 16 KiB of file data is `((i * 17 + i // 47) & 255)` in the current tree and `((i * 49 + i // 71) & 255)` in the published snapshot. Both have a 32 KiB hole and the 18-byte tail `HAMMER sparse tail`. The snapshot also includes the former hard link `/retired`. Upstream `hammer -f image show` validated the structure, and the upstream `hammer -f image recover output` recovered the complete current file with SHA-256 `4e04f28fd9244bb9f8d53e63a825952d3796ebc9cc5bd94fd572215e7be74d59`. The populated fixtures were generated in userspace.

The zlib-compressed fixture stream contains the little-endian `u64` image size followed by sparse records with a `u64` byte offset, `u32` length and payload. Stored extents start at 64 KiB boundaries. Gaps and omitted trailing bytes in a block are zero.

| Fixture | SHA-256 of `.h1z` |
| --- | --- |
| `hammer1-v7-empty` | `551f91bd54893b166ef7fa379b3af71ff8243771421a59ecdc340dfbae96f921` |
| `hammer1-v1-content` | `b8f11d2f6fffbd7905114af851508615512d44d152c7519092b769d8eaa6ceec` |
| `hammer1-v7-content` | `1948074b37eb5cf186480299f0cba2525e44b16ca2f46be7beecc4008d9857c4` |
