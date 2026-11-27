# GFS1 reader provenance

LovelyCarve implements the runtime reader in Rust under AGPL-3.0-only. The media layout was compared with the [Red Hat GFS guide](https://docs.redhat.com/en-us/documentation/red_hat_enterprise_linux/5/pdf/global_file_system/Red_Hat_Enterprise_Linux-5-Global_File_System-en-US.pdf), the [GFS1 on-disk definitions](https://sourceware.org/git/?p=cluster.git;a=blob;f=gfs-kernel/src/gfs/gfs_ondisk.h;hb=78a78bc47644465fbc9cf7acb5d781daf7cb4e5b) and the [journal header checks in the GFS kernel code](https://sourceware.org/git/?p=cluster.git;a=blob;f=gfs-kernel/src/gfs/recovery.c;hb=78a78bc47644465fbc9cf7acb5d781daf7cb4e5b).

The native tools came from the [Red Hat Cluster 2.03.05 release archive](https://sourceware.org/pub/cluster/releases/cluster-2.03.05.tar.gz) with SHA-256 `e4c7c0b5b0e3cab7f7274b2c6d5365cd37b7c5050aa6883fe496055d77804669`. The formatter and debugger were built in userspace. A compatibility stub covers the retired `libvolume_id` interactive confirmation path, which is skipped by `-O`. Each 192 MiB sparse image was formatted with `gfs_mkfs -O -p lock_nolock -b 512|4096 -j 1 -J 32 image`.

A fixture-only program calls the release's `fs_mkdir`, `fs_createi`, `fs_dir_add` and `writei` routines for `/data`, its files, the hard link and the symlink. After those calls the program serializes the allocation map and updates resource-group counters. A fixture post-processing step marks the indirect blocks with the types specified by the GFS kernel. The program constructs a height-two data map before writing the first bytes because the standalone userspace library leaves the existing-pointer growth path incomplete. The populated fixtures were generated and checked in userspace.

The payload starts with one block of `((i * 11 + i // 43) & 255)`. The final 16 bytes are `GFS1 sparse tail`, beginning at offset `42 * 512 + 7` or `520 * 4096 + 7`. `gfs_debug -d image -b 159 readfile` for the 512-byte variant and `-b 24` for the 4096-byte variant read the full bytes. `gfs_debug` also lists the root and `/data` in both cases. The 512-byte resource index uses journaled-data blocks, and its `/data` stores an exhash table. The 4096-byte variant uses a linear `/data` directory and verifies two separate indirect branches for the sparse file.

The zlib-compressed fixtures store a little-endian `u64` image length and a sequence of `(u64 offset, u32 length, bytes)` records. Gaps represent zeroes.

| Fixture | Compressed SHA-256 | Native `payload.bin` SHA-256 |
| --- | --- | --- |
| `gfs1-512.gfs1z` | `663c6984fe83a4e09cd7782a2c5467d467d0d51d551021a851c346ddd9f730ef` | `11a0adc308355c21cc464bbf3f1ad6860838fc19ca38b3cd164213a69fd20e37` |
| `gfs1-4096.gfs1z` | `72b2347c502ae1765d24ce184249c7da16ae3db1ee798d064ffa7b3eee9210e3` | `30b1d0589647cd7829b5ed17b747edd101f3973cc5c6932f7f5a9d9e47f2dc9b` |
