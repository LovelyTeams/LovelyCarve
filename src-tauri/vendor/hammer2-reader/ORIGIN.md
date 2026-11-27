# HAMMER2 reader provenance

LovelyCarve implements a read-only HAMMER2 reader in Rust (AGPL-3.0-only). The format was compared with the [DragonFly BSD HAMMER2 design](https://www.dragonflybsd.org/hammer/), [DragonFly block checksum implementation](https://gitweb.dragonflybsd.org/dragonfly.git/?p=dragonfly.git;a=blob_plain;f=sys/vfs/hammer2/hammer2_chain.c;hb=HEAD), and [libhammer2](https://github.com/kusumi/libhammer2/tree/e0b88b9f40709637625d9256735268129bf6c96f). Runtime dependencies are Rust implementations of bounded image access, CRC32C, xxHash64, SHA-256, LZ4 and zlib.

The native fixtures use the [makefs HAMMER2 implementation](https://github.com/kusumi/makefs/tree/607449619ef5f85b6a2b282e16182e342fdebeaa). The downloaded source archive SHA-256 is `4e043a71825599d678166412873e4f853d3ff5777b991a4a686db271dcb44cc1`. It writes a 196,613-byte `/var/log/event.bin` with byte `n` given by `(n * 17 + n // 51) % 256`, plus `/hello`, its hard link `/same-inode`, a relative link `/latest` to `var/log/event.bin`, and `/sparse.bin` with `hole-tail` at byte `5 * 65536 + 93`.

Build with `make USE_EXFAT=0` and run `makefs -t hammer2 -Z -T 1700000000 -s 2g -M 2g -o v=1,L=DATA,c=lz4 output.h2 input`. For each variant below override the indicated flags. The CLI and unit tests materialize the same logical bytes from the fixture, which stores a little-endian `u64` image length followed by nonzero 64 KiB extents, each with a `u64` offset, a `u32` length and the payload. The record stream is zlib-compressed.

| Fixture | Writer options and coverage | SHA-256 of `.h2z` |
| --- | --- | --- |
| `lz4` | Default above, v2 and LZ4 | `343c275caa61309c2e46575b0dca25d189663416bdfd3fe13a97bff628a5d701` |
| `zlib` | `c=zlib` | `6e1cacda23052336ab8174bcdf253753169699f8665e8ba5188c41132b796225` |
| `mirror` | `-s 4g -M 4g -o v=2,L=DATA,c=none,C=iscsi32`, with the newest header in zone 1 | `f4e3aa8504b1862dd85fcabdcb940d10cd81bec301c4044468382de4cdfb3b12` |
| `v1` | `V=1,c=lz4,C=iscsi32`, with an additional long name `long-` + 85 `d` bytes + `.txt` | `c4ce4a0f9e5507ff7c1137dfbc4e5464991f2612be2ef4d0fa56d949c586b7a5` |
| `snapshot` | Default above, with the long name and `/absolute-link` → `/var/log/event.bin`. Publish `ARCHIVE` with `makefs -t hammer2 -o P=snapshot:ARCHIVE,m=DATA output.h2 input`. | `21d75d309f5b617e34cc6485d63409c217b3b952f061402689b5092e1314b4a0` |

The upstream utility additionally reads native inode metadata and lists the published snapshot. The test for the DragonFly SHA192 checksum uses a separate SHA-256 known answer with the last two 64-bit words folded together as in the kernel.
