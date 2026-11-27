# LittleFS reader provenance

LovelyCarve implements read-only LittleFS v2 in Rust (AGPL-3.0-only). The runtime uses `lamfold` for the bounded, immutable byte source and `crc32fast` for IEEE CRC32. The tag, metadata, directory, and CTZ rules follow the [official v2.11.0 specification](https://github.com/littlefs-project/littlefs/blob/v2.11.0/SPEC.md) and were compared with its reference implementation. The [official release archive](https://codeload.github.com/littlefs-project/littlefs/tar.gz/refs/tags/v2.11.0) has SHA-256 `54ed6382d75cbd4898fa89ddc0db7bf82abadfd8b209e12950cb10a05a6dc424`.

The fixtures were written and remounted with the original v2.11.0 `lfs.c` and `lfs_util.c`, with `LFS_MULTIVERSION` enabled. A memory-backed device models erased flash as `0xff` and requires programming to clear bits. Minimum read and program sizes and lookahead size were 16 bytes. The cache size was 16 bytes for small blocks and 256 bytes for 4096-byte blocks. Only the generated image bytes are committed.

| Fixture | Erase bytes | Erase blocks | Disk version | `block_cycles` | SHA-256 before zlib |
| --- | ---: | ---: | --- | ---: | --- |
| `128-2.0` | 128 | 317 | 2.0 | 3 | `82d27a28edc23d06831a3602266618c847722f17f9e2d0103b32b4414027fad2` |
| `512-2.1` | 512 | 333 | 2.1 | 3 | `94b03dc415c6290623d95a5690769f2962787c21623e28021901cce6f34a924d` |
| `4096-2.1` | 4096 | 96 | 2.1 | 10 | `457b02650f1ca152df6ab81bb0da86a0faa40fe3058de9f48d1c8b473a6059fc` |
| `interrupted-move` | 512 | 333 | 2.1 | 3 | `e3c31fd8db5ba9b5122494b30dfd31ed3d81d1f5bed9632e3a15501eb19928a1` |

Create `/Logs`, `/Split`, and `/Logs/Nested`. Write `init` and append `-end` to `/tiny`. Write and remove `/obsolete`, rename `/move` into `/Logs/Nested/EmptyLater` and truncate it. Add two files to `/Logs` with raw names `with\backslash%` and `odd-\xff` (the latter ends in byte `0xff`). Write the first 513 bytes of `/Logs/event.bin` and append the rest of the 19,473-byte sequence `(n * 21 + n // 49) % 256`. Add files `entry-%02u-long-file-name-for-split` to `/Split` in order `(k * 7) % 20` for `k` from 0 to 19, each containing `a`.

The interrupted fixture records the first device sync during the cross-directory rename, after the destination commits and before the source is deleted. The official implementation remounts these bytes, reports the pending state, exposes the ten-byte `after move` destination and returns `LFS_ERR_NOENT` for the original source.
