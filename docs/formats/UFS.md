# UFS1 / UFS2 只读文件系统

LovelyCarve 使用 vendored `rufs` 读取 BSD Fast File System。UFS1 支持
4.4BSD 格式的 128 字节 inode、32 位块指针，以及大小端。UFS2 使用 256 字节
inode 和 64 位块指针。裸卷和磁盘分区均按主超级块识别。

- UFS1 使用 8192 字节位置的主超级块，按柱面组的历史偏移定位备份超级块和 inode。UFS2 也接受 NetBSD makefs 使用的 8192 字节位置。
- 浏览目录、硬链接、符号链接和稀疏文件。普通文件支持完整、流式和范围读取，统一验证直接及三级间接块的边界。
- 保留 atime、mtime 和 ctime。UFS1 inode 缺少创建时间字段，创建与出生时间留空。
- Windows 上遇到非 UTF-8 目录名时明确报错，避免名称替换导致路径指向其他文件。
- 4.2BSD inode 格式、专用 Solaris / Apple UFS 变体、日志重放、写入与删除恢复未验证或未接入。

布局参考 [FreeBSD UFS inode 定义](https://cgit.freebsd.org/src/tree/sys/ufs/ufs/dinode.h?id=6a7405f5a6b639682cacf01e35d561411ff556aa)
与 [OpenBSD FFS 柱面组寻址](https://github.com/openbsd/src/blob/master/sys/ufs/ffs/fs.h)。

[夹具](../../src-tauri/tests/fixtures/ufs1/README.md)由独立的 NetBSD makefs 生成。
`just test ufs` 覆盖大小端、裸卷与 MBR 分区、硬链接、符号链接、创建时间缺省、文件完整读取和跨直接/间接块范围读。
