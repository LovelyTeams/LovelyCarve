# UFS1 native fixtures

These invented read-only images were built with NetBSD `makefs` from Ubuntu's
[official source archive](https://archive.ubuntu.com/ubuntu/pool/universe/m/makefs/makefs_20190105-3_amd64.deb).
The fixture contents are CC0-1.0. The reader does not run makefs.

The input tree has `sub/readme.txt` containing `UFS1 native test\n`,
a symbolic link `link` to `sub/readme.txt`, and `big.bin` containing 164,000 bytes
with byte `i` equal to `(i * 7 + i // 257) % 256`. `alias.bin` is a hard link
to `big.bin`. Build each raw image with:

```sh
makefs -t ffs -o version=1,bsize=4096,fsize=512 -B little -s 2097152 -T 1720000000 ufs1-little.img source
makefs -t ffs -o version=1,bsize=4096,fsize=512 -B big -s 2097152 -T 1720000000 ufs1-big.img source
```

The checked-in copies are zlib-compressed at level 9.

| Byte order | Raw SHA-256 | Stored SHA-256 |
| --- | --- | --- |
| Little | ddcb61c2a40fec52ee439d7cc1fdcac73bfd35034539865b6e04a81def160af0 | a8f761f7d417b716e2bc2b3965c062b824348243e340ccf663c60e037b7e4d9b |
| Big | 073b25d46c2c7a6da5aa2a67b0268a36a8a32ad297a9d648544b56d928e3360d | 5dc64a05d8a655c53c6c7c9d29c8861475d5cf16971dc6a3235c1b417880577f |
