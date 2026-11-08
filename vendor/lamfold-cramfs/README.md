# lamfold-cramfs

> The cramfs frontend of the [`lamfold`](../lamfold) read-only media stack —
> clean-room Compressed ROM filesystem (per-page zlib).

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`.
cramfs is the small per-page-zlib read-only format for embedded firmware and
rescue images — compressed like squashfs, but with a far terser structure (a
12-byte bitfield inode, a flat per-page block index) for where squashfs is too
heavy. Spec: `the lamfold design spec` §4.

## Status

| Layer | State |
| ----- | ----- |
| **Superblock** (magic `0x28CD3D45`, root inode at byte 64) | ✅ done + tested |
| **Inodes** (12-byte bitfield: mode/uid, size/gid, namelen/offset) | ✅ done + tested |
| **Directory records** (`inode + name` run over the inode size) | ✅ done + tested |
| **Per-page zlib block index** (u32 end-offsets → substrate `Codec::Zlib`, hole zero-fill) | ✅ done + tested |
| **Symlinks** via `read_link` (zlib-stored target) | ✅ done + tested |
| big-endian images, `EXT_BLOCK_POINTERS` extension | rejected `Unsupported` (not mis-read) |

**Complete.** Verified against a **real** `mkcramfs` (cramfs-tools) image,
cross-checked with `cramfsck`: probe, tree walk, a 300 KB file across 74
zlib-compressed pages (full + partial across a page boundary + EOF), a nested
directory, and a symlink. Builds default + `--no-default-features` (`no_std`),
clippy `-D warnings` clean, `#![forbid(unsafe_code)]` — every page inflated
through the shared substrate codec.

## Clean-room posture

Derived only from the kernel cramfs format docs (`cramfs_fs.h` is the on-disk
structure). The GPL-2 `fs/cramfs` driver is fenced off. The upstream `cramfs`
crate is unbuildable, so there was nothing permissive to reuse.

## Build / test

```bash
cargo build
cargo test                                   # walks tests/fixtures/cramfs.img
cargo build --no-default-features            # no_std check
```

MIT OR Apache-2.0.
