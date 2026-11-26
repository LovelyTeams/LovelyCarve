# Reiser4 format references and fixtures

The production reader is original AGPL-3.0-only Rust. The format references are the maintainer's [reiser4progs 1.2.2](https://github.com/edward6/reiser4progs/tree/23e8ccb6442e4829479c602574215a6cb58ba6f9) and [Reiser4 kernel source](https://github.com/edward6/reiser4/tree/05021f10e4fceaad2439b84f200a51c65a8fe9b0). The source locations used were `include/reiser4`, `plugin/{format,journal,alloc,node,key,item,sdext}`, and the kernel's `dformat.h`. No upstream C code or driver is built or loaded by LovelyCarve.

## Native fixtures

Generated in a disposable Linux directory with `mkfs.reiser4` from the pinned reiser4progs commit and [libaal](https://github.com/edward6/libaal/tree/8a10deda64c7b37ec356654e531f9cfd6e4ac132). The source archives came from GitHub codeload, with the following SHA-256 digests.

| Archive | SHA-256 |
| --- | --- |
| reiser4progs | `aa0456c9a788a9c671e2571afad0035db5b22acdd5b4675e23b5cf7759349837` |
| libaal | `d1c2befe33c29d4ff9ba8f8a2fda911e531a0b44c204b5085a3496615bff3a68` |
| kernel Reiser4 | `9942a41da824cb139d6c78e807b1e048dedb6a5ae52617664aedcba7e5df7730` |

The local fixture tools retain their upstream licensing. Three scratch-only adjustments let the native tools generate these layouts. In the combined-key build, `ha_get_ordering` in `cde40.h` returns zero for short keys. In `demos/busy.c`, the object-type dispatch checks `T_SLINK` for the `ln-s` command. In `libreiser4/tree.c`, the existing disabled extent-allocation and `cb_nodeptr_adjust` functions are enabled, and `reiser4_tree_sync` passes `cb_nodeptr_adjust` as its item callback so it commits the demonstration program's pending extent data before writing the node. The Linux linker uses `LIBS=' -Wl,--no-as-needed -luuid'` to keep the UUID library on the native tools' link line.

Both images are 16 MiB and use UUID `8ae13701-95da-49db-9661-cf2cce323084`. The first uses `-o create=reg40` and the default smart tail policy. The second uses `-o node=node41 -o key=key_short -o create=reg40 -o formatting=extents`. Each is labeled with its fixture name. `busy mkdir`, `cp`, `ln` and `ln-s` create `/nested`, `/nested/tail.txt` containing `reiser4 inline bytes\n`, `/nested/native-long-name-for-hash.bin` with 11,113 bytes of `(i * 31 + i / 211) % 256` using integer division, `/hard-link` pointing to `tail.txt`, and `/symbolic` with target `nested/tail.txt`. Writing the 11,113-byte payload at offset 32768 creates `/sparse.bin`. `debugfs.reiser4 --print-tree` reports eight explicit hole blocks and a three-block allocated run in the node41 fixture. The node40 fixture records an allocated run including its zero prefix.

`busy ls` listed both directories and `busy cp` exported the long-name and sparse files independently. Their SHA-256 digests were `1ba2c07e284591e221fc293083eba7ea89a5be0000de60ff9792026f1fbda751` and `fa156d414f5af9579eb6d30f917c1629772811682bb39232bc752821fadd2ee6`. The upstream `fsck.reiser4 --check --yes --force` reported a consistent filesystem on both raw volumes. Examples use synthetic file contents. Mkfs IDs and timestamps vary with generation time.

| Fixture | Raw SHA-256 | zlib SHA-256 |
| --- | --- | --- |
| `node40-large.reiser4z` | `a1edc26af8decf4365de31318114420cce681802796c88ed2cae18a729c76330` | `4733518b64364470f58288bd580d1eedfd38b05dbdcef9039771eac54c9b0c1f` |
| `node41-short.reiser4z` | `bfb1a5f2ec70b7ac9cd08fa763db889055724eaf782c7d812f947afbb74ee58f` | `e2a0d472aa42c5718feb7a3d1f2b11fb6c6bf28347a36be7839151268f79b082` |

## Native compressed volumes

Three 128 MiB images were formatted with the same `mkfs.reiser4` using
`-o node=node40,key=key_large,create=ccreg40,compress=<plugin>,compressMode=force,cluster=16K`.
Files were created in QEMU with Linux `5.17.0-3+reiser4.0.2-amd64`, copied from
the boot image's installer initramfs, and verified after unmounting and mounting
the volumes read-only. The boot image came directly from the distributor.
[Metztli Reiser4 Bookworm](https://metztli.it/amatlocuilin)

Each `/nested/payload.bin` has one 16 KiB compressible cluster, 16 KiB of
deterministic pseudorandom bytes stored as an uncompressed cluster across
five leaves, and a 1217-byte compressible final cluster. Each `/sparse.bin`
starts with `first bytes of a sparse file\n`, has four intervening absent
clusters, and ends with `last bytes of a sparse file\n` at offset 81920.
The kernel read back `/sparse.bin` with SHA-256
`e9d3aa0b1f38fc1314e1de780abdc0832afb156291833a658a82be0e4063496e`.
`debugfs.reiser4 --print-tree` identified the ctail40 items and the codec
and cluster size stored on the root. The fixtures are zlib-compressed copies of
the unmounted volumes.

| Fixture | Raw SHA-256 | zlib SHA-256 | Kernel payload SHA-256 |
| --- | --- | --- | --- |
| `lzo1.reiser4z` | `64e02fb56a9edea283c0490f93e01ea76a79326420c4a1bc4cf3703f480002f3` | `9fbc29c664e7a84a4635d618ef8875488968eeffe111c754ef1a51e5a1e828aa` | `cd45bbcea146424c2affe97c07d381ecdf73aa059d7d9151be3225a2db83d65b` |
| `gzip1.reiser4z` | `7b4df48345ff2dc74d2a25b53d63866a577702fdc1aa79cc88a9d146b9a6c649` | `eda854ed751cc2111b16b7d57885f8f991f28740bbfeb8ede71a3b673eeb2632` | `71db1e58c437c56317c4a3fea3dd30ed6310039f475f71400253d3f3297012f7` |
| `zstd1.reiser4z` | `943ed6cf04879b0faf1995d01ad1620b423256569a8e4007bb1f234999b12168` | `cf9f7b921afba6d2fe4a0a0fbb0150cd3c4527fe72c973a958d6ddf2fe7ead33` | `36ae9eb7cf5d0fb38f754d0ec28ebe355e5f4e960d689b119e1f82d42a638849` |
