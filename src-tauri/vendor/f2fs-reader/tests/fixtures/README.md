# F2FS fixtures

`synthetic.f2fs.zlib` is an invented 32 MiB byte-addressed reader fixture,
compressed using zlib. Original data and `scripts/f2fs-fixtures.py` are CC0-1.0.
It deliberately builds only the structures under test; not a full mkfs output.

`syzkaller_0.f2fs.zlib` retains the original compressed payload of Google's
[syzkaller corpus](https://github.com/google/syzkaller/blob/6d8cd38e2e11f13f9c5d662d48fb1bf82a7644cb/sys/linux/test/syz_mount_image_f2fs_0).
Apache-2.0; see LICENSE-syzkaller. `syz-imagegen` generated the 64 MiB filesystem
with `mkfs.f2fs -e cold -a 0 -s 1` and mounted/populated it. Its source's
`populateDir` specifies repeated `syzkaller` bytes in file1 (10), file.cold (100),
and file0/file0 (1050); file2 (9000 zero bytes) and file3 are hard links.
The test verifies those independent expected bytes, not a Rust formatter round-trip.
Checksums and compressed sizes are in manifest.json. No private evidence is included.
