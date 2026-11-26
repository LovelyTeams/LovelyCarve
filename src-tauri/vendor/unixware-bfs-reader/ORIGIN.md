# UnixWare BFS reader origin

LovelyCarve implements the reader in Rust under AGPL-3.0-only. On-disk fields were checked against the Linux v6.12 [BFS disk definitions](https://github.com/torvalds/linux/blob/v6.12/include/uapi/linux/bfs_fs.h), [inode reader](https://github.com/torvalds/linux/blob/v6.12/fs/bfs/inode.c), and [directory reader](https://github.com/torvalds/linux/blob/v6.12/fs/bfs/dir.c). The classic disklabel layout and tags were checked against util-linux v2.41.1 [UnixWare probing](https://github.com/util-linux/util-linux/blob/v2.41.1/libblkid/src/partitions/unixware.c), Linux v6.12 [MBR parsing](https://github.com/torvalds/linux/blob/v6.12/block/partitions/msdos.c), and the [UnixWare VTOC manual](https://uw714doc.sco.com/en/man/html.1M/prtvtoc.1M.html). Build and runtime require only Rust code.

`util-linux-empty.zlib` holds the raw 1 MiB volume produced with util-linux 2.37.2 `mkfs.bfs -F TESTFS -V STAND -N 128` in Ubuntu. The volume was confirmed with `blkid -p`. [The upstream formatter](https://github.com/util-linux/util-linux/blob/v2.41.1/disk-utils/mkfs.bfs.c) documents the independently produced root directory, reserved blocks, and idle relocation state. `python scripts/make-unixware-bfs-fixture.py` populates a copy with documented inode and directory records. The public CLI test constructs the nested VTOC independently of the Rust parser.

| Raw image | SHA-256 |
| --- | --- |
| `util-linux-empty` | `4825c574599610df26c65f70efbbc62b951991cea35695e3dd5a460fe25b13e7` |
| `populated` | `7070eb04c5ade6da766937385894e568e8d24d93de3700650aac597ded7e4726` |
