# F2FS

裸 F2FS 镜像和分区内 F2FS 可直接浏览、预览、哈希、导出、建立快照与索引。
读者为独立纯 Rust `f2fs-reader`，不挂载镜像、不依赖 Linux、驱动或外部进程，始终只读。

## 支持范围

- 4 KiB 文件系统块、512 字节逻辑扇区、每 segment 512 块的单设备布局；主 / 备超级块
  分别位于 1024 / 5120 字节。检查布局、源长度、元数据区域及可选超级块 CRC。
- 两个 checkpoint pack 检查头、尾、CRC、版本和包范围，按可回绕版本号选最新有效包；
  使用该包的 NAT 版本位图与 NAT journal，检查 journal 上限、重复项和 node footer 身份。
- 普通 / 内联目录、跨槽名称、内联数据、扩展 inode 属性、可变内联 xattr 区域；
  完整读取 inode 内直接地址、两个 direct node、两个 indirect node 及最后的多级间接树。
- inode 校验和、节点归属、分配块的 main-area 边界与完整源读取；格式定义的 hole / NEW_ADDR
  返回零，已分配块越界或来源截断报错。范围、流式与完整读取共用同一逻辑块映射。
- 硬链接按路径保留不同目录身份，inode 号相同；符号链接保留原始目标，不在宿主跟随。
  atime / mtime / ctime 与扩展 crtime 保留各自角色和纳秒精度，按 POSIX UTC 解释。
  没有 crtime 时不合成创建时间。文件不生成未经验证的物理 data runs。

读取的是最后有效的已提交 checkpoint；不执行 fsync roll-forward、日志恢复、删除项恢复、
SIT 一致性审计或垃圾回收，不把残留 node 当作当前文件。加密、压缩、casefold 和 device-alias
inode 保留可解析元数据，内容或目录读取明确拒绝。多设备、zoned、RO 特殊布局和未知特性
不支持；外置 xattr、配额与 fs-verity 没有专门展示或验证接口。
目录最多 16,384 个块、1,000,000 个条目，路径沿用共享适配器的 256 层上限。
非 UTF-8、反斜线名称明确拒绝，避免替换字符导致身份合并。
Android sparse 包装应先展开为裸镜像；这项适配不包含 sparse 容器解析。

## 来源与验证

NAT、checkpoint、superblock 和常量解码改造自
[KarpelesLab/fstool 0.5.0](https://github.com/KarpelesLab/fstool)，保留 MIT 许可；
未引入其 writer、格式化器、容器和 CLI。inode / 寻址 / 目录与宿主接入为本仓库代码，
对照 [Linux F2FS 布局](https://github.com/torvalds/linux/blob/v6.12/include/linux/f2fs_fs.h)
与 [内核文档](https://docs.kernel.org/filesystems/f2fs.html)。具体差异和许可见
[读取库来源](../../vendor/f2fs-reader/ORIGIN.md)。

独立 Python 夹具覆盖扩展属性改变地址起点及内联容量、跨 direct-node 边界、硬链接、
普通 / 内联目录、空洞与时间戳；另构造所有间接层级、NAT 位图 / 日志选择、损坏 checkpoint
回退、inode CRC、坏指针及截断。夹具只是最小字段构造，不宣称通过 fsck 的完整 mkfs 输出。

真实 mkfs 兼容样本来自
[syzkaller `syz_mount_image_f2fs_0`](https://github.com/google/syzkaller/blob/6d8cd38e2e11f13f9c5d662d48fb1bf82a7644cb/sys/linux/test/syz_mount_image_f2fs_0)，
由其 `syz-imagegen` 调用 `mkfs.f2fs -e cold -a 0 -s 1` 生成并填充。保留 Apache-2.0
许可证，以原 zlib 字节压缩保存 64 MiB 镜像；按 imagegen 的 `syzkaller` 重复内容和零文件
公式逐字节核对内联 / 多块文件、目录及硬链接。不是用户手机检材，也不是加密 / 压缩文件验证。
夹具来源与哈希在库的 `tests/fixtures/README.md` 中。

```text
just generate-f2fs-fixtures
just test-vendor f2fs-reader
just clippy-f2fs
just test flashfs_ --lib
just test-cli
```

外部已展开镜像还可设置 `F2FS_IMAGE`，运行
`just test-vendor f2fs-reader f2fs_external -- --ignored --nocapture`；未设置时 ignored
不计为外部样本验证。Windows 原生 GUI / Dokan 挂载需单独验证。
