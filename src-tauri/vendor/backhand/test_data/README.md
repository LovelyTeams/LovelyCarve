# Synthetic DD-WRT fixture

`ddwrt-v3.squashfs` is generated entirely by `generate_ddwrt.py`, using Python's
standard `struct` and `lzma` modules. It contains one file `/hello` with the
literal `Independent DD-WRT fixture.\n` repeated 64 times. Data, inode and directory
blocks use different LZMA parameters. There is no real firmware or user data.

Layout reference: Firmware Mod Kit commit
`c72f45d7a062156125ae00cb867d8a614b50b963`, SquashFS 3.0 variant headers.
The generator is an independent implementation, not a backhand writer round trip.
These original synthetic fixture files are dedicated to the public domain under CC0-1.0.

SHA-256: `adf312d8c6d9e7ae5fadcfec851b960a4e729a3495caa5c277dcc71e978db7eb`.

## Independent v3 LZMA and gzip fixtures

`generate_lzma.py` uses Python stdlib `struct`, `lzma` and `zlib` to encode
SquashFS 3.0 LE/BE images. Each `/hello` file is the literal
`Independent Rust LZMA fixture.\n` repeated 350 times, across three 4096-byte
blocks. Raw LZMA, LZMA-alone, five-byte properties and gzip variants contain the
same bytes. LZMA blocks use different lc/pb values to exercise cache changes.
Separate `.block` fixtures contain the first file block. These are original
CC0-1.0 fixtures, independent of backhand writers and public firmware content.

| File | SHA-256 |
| --- | --- |
| lzma-alone.block | `75b280fb070c5d652ecea2e387da99a54e177112f64fc2be6a5133177cf10d7b` |
| lzma-props.block | `833f7516258e5552c968bb1fba95dba411abce8eb6c2c52fa342b6838869a921` |
| lzma-raw.block | `8f5b3c7b10535183d9b297b7956b5546471289eda69f6968cad589b440945afb` |
| lzma-v3-alone-be.squashfs | `867f8a9621f30af71d9c839afc731e291338d5861c226430be0a0e305b5c604b` |
| lzma-v3-alone-le.squashfs | `e82261323d8f8f53366fc99960fe1d3bdd4f7e80eaa24c271dd0b7cf4e15f3f6` |
| lzma-v3-gzip-be.squashfs | `00a01382c6102d0f12659f82f48a991c8a673e914f4b92d0d1140bdc41256411` |
| lzma-v3-gzip-le.squashfs | `269467a023c4cb0b0e68ec0ce668e0600c2e2a12a3bd371a84ba3d7971b7d324` |
| lzma-v3-props-be.squashfs | `b89bdfc4044d6aab5eb444e722bb7deb94c5e8d066801f65ff8080d70318625a` |
| lzma-v3-props-le.squashfs | `ee5265d095f922106b158bfef2a7e91fa2b0adb13a0823bd0fe099aabe012835` |
| lzma-v3-raw-be.squashfs | `a9bb3ddde7c6a02770dcf93495e21783838a38cfa030ff60e753c4cfd0f9702e` |
| lzma-v3-raw-le.squashfs | `c009301ed065e18c2b904004bb1dd0fa9145730cb2d503afa41401f1b77ae4a5` |
