# FATX reader origin

LovelyCarve implements this read-only Rust reader under AGPL-3.0-only. The FAT geometry, native root cluster and timestamp roles were checked against [XboxDev/fatx `c83ba2340bd2805d1832bf2071e51ebe6b99e723`](https://github.com/XboxDev/fatx/tree/c83ba2340bd2805d1832bf2071e51ebe6b99e723), [Velocity](https://github.com/hetelek/Velocity/blob/master/XboxInternals/Fatx/FatxDrive.cpp) and [Aerosoul's FATX documentation](https://aerosoul94.github.io/blog/2020/02/25/fatx-reading-and-recovery.html). The Xbox disk layouts also use the original [XBPartitioner table fields](https://xboxdevwiki.net/Xbox_Linux_Issues) and the [Free60 Xbox 360 partition map](https://github.com/gligli/Free60-Wiki/blob/MoreFixes/FATX.md).

The original Xbox test volumes were formatted and populated with XboxDev's `libfatx` on Linux. The 32 MiB and 160 MiB volumes use 2 KiB clusters and were reopened with `libfatx`; the extracted file matched the source byte for byte. The committed fixtures clear unused name-field padding because the C writer can copy uninitialized bytes there. The remaining bytes are preserved. The 8 MiB and 144 MiB XTAF fixtures were produced with [`fatx-rs` `efdb8b35504964765628525cc35471c423ecc63e`](https://github.com/joshuareisbord/fatx-rs/tree/efdb8b35504964765628525cc35471c423ecc63e), `fatx mkimage <image> --format xtaf --spc 4`, then populated via `fatx mkdir` and `fatx write`. `fatx read` reproduced the input on both volumes. The C and Rust tools are independent test sources. LovelyCarve builds and runs with its own Rust reader.

Each fixture starts with a little-endian 32-bit raw byte length followed by a zlib stream of the bytes through the final nonzero byte. Tests restore the trailing zeros from the recorded length. The full test images contain `/Cache/Data.bin`, which has 13,013 bytes given by `(n * 17 + n / 117) % 256` with integer division starting at `n = 0`, and `/Empty`.

For a separate end-to-end check, the same `libfatx` writer was used to format a native-size Xbox cache partition. Sparse disk wrappers placed it at `0x80000` and the independently produced XTAF volume at the Xbox 360 content offset `0x130eb0000`. The CLI located both regions and matched the input file's range and SHA-256.

| Expanded fixture | SHA-256 |
| --- | --- |
| `xbox-fat16` | `d70886eab24782da257c69debd1c83c5709cce3669f9676eca329d6e1b7fe7af` |
| `xbox-fat32` | `1f8bd6d871a0fc9521011025ce7b6ec252ddd89f44dd3e3927a09c6886daa0de` |
| `xtaf-fat16-compact` | `fe658a38f9fc1ef310a6c1cd78c54096b9173f6586858e453a5a3b0fa7e33982` |
| `xtaf-fat32-compact` | `5bd725161a766c204ae46e7a6e9ae4b6c13afb766d1dc86ed2c9b2b7b98bf9bb` |
