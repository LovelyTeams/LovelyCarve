# Macintosh File System reader provenance

LovelyCarve implements the MFS and Disk Copy 4.2 readers in Rust, under AGPL-3.0-only. The reader uses `lamfold` for the block source and `classic-hfs` for the existing MacRoman table, filename collation and fork type.

- Apple published the directory and allocation structures in `MFSVolumeFormat.pdf` and `MFSCore.c` within the [original MFSLives sample archive](https://developer.apple.com/library/archive/samplecode/MFSLives/MFSLives.zip). The archive SHA-256 is `5cde0d9b492e4a05537741a2613ec3befd71076bf65288db77d399f1bb85989f`. Its real 400K Disk Copy `Sample.img` has SHA-256 `7bfdd393234bc76e5d04cc6b857d9aa5972c54e4c86bb0b1a6f757176f8aa2b8`. It includes sector tags and a second MDB. Apple code and media are external to the checkout.
- The [CiderPress II MFS format notes](https://ciderpress2.com/formatdoc/MFS-notes.html) document the reserved allocation blocks used when the directory lies inside the allocation area.
- The fixture generator is [macfs 0.1.0](https://crates.io/crates/macfs/0.1.0) (MIT OR Apache-2.0), revision `ccdcec5369096f95465fe75ff12d3b42cbbd5f21`, published crate SHA-256 `f33e790d7717bde4265254854045d52ec9471687218d36d5f6b7d002395d3881`. It also provides an independent extraction of the Apple sample's `TN.002.Compatibility`. The 13,130-byte data fork has SHA-256 `bc23503a695764123759f006dfc4fa0052d96ec293c35168090aedbc9e497a8a` and the 833-byte resource fork has SHA-256 `65d4a524d31c5c9c76579ef3656d592266bd11bd073c255c331029c20ea55cf5`.
- [fs-mac 0.1.0](https://crates.io/crates/fs-mac/0.1.0), revision `3f4107758fbe5b89fe039530964a36141e1fa6cf`, published crate SHA-256 `b66ba62ead3e030e68be4646a54e64d29796ddad6669ff1f810d9ee53f254ad9`, independently checks both forks in the four committed fixtures. Neither oracle crate is a runtime dependency.

## Fixtures

Use `mfs mkfs --size 400k --name "MFS Work" <image>` and repeat for `800k` and with `--dc42`. Add `Résumé` with type `TEXT`, creator `ttxt` and both forks using `mfs add`. Add `Only Resource` with an empty data fork and the same resource bytes, and `Empty` with both forks empty. Byte `n` is `(n * 13 + n // 47) % 256` in the 8,199-byte data fork and `(n * 7 + n // 81) % 256` in the 4,673-byte resource fork.

| Decompressed fixture | SHA-256 |
| --- | --- |
| `raw-400` | `65bbade457beaea208b6ced4b216eebee1ddbec34478c235b59d3ba87b2274f1` |
| `raw-800` | `8a66df16ba6581c5fee3e5c7ae9f659e6ebfa36a53b729236993499a4e301f1f` |
| `dc42-400` | `7c696921cea8b770255fa42d3088c55ca3512da611eb118f7d682557e8459b27` |
| `dc42-800` | `8da842d995f0730ae8b2b18ac9c0e656186f09a265ce6f1d900bc36041848e88` |
