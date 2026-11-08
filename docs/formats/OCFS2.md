# OCFS2

`ocfs2-reader` 是纯 Rust、只读的 OCFS2 0.90 卷读取器。适配前检索了
crates.io / GitHub：`unearth` 提供超级块识别，`reflicate` 使用已挂载卷的
reflink ioctl，未找到可直接接入的完整 Rust 镜像读取器。本实现复用
`unearth` 的 MIT 前缀布局，按原生格式定义新增读取和校验。
固定来源及许可见 [ORIGIN.md](../../vendor/ocfs2-reader/ORIGIN.md)。
产品不链接 C 文件系统库，不调用内核挂载或外部提取程序。

## 能力

| 项目 | 范围 |
| --- | --- |
| 格式与几何 | OCFS2 0.90、小端、512 / 1024 / 2048 / 4096 字节块；4 KiB–1 MiB 的二次幂簇；块 2 超级块、源长度、root / system 身份和特性位校验；OCFS1 保持独立 |
| 分配 | 全局簇位图、全局 inode 和每 slot 的 inode / extent 分配链；组边界、计数、代际、子分配位及占用状态校验；支持连续及 discontiguous 子分配组 |
| 目录 | 内联 / 外部目录、目录尾及索引 root / leaf；TEA 名称散列、桶位置、计数和目录项集合交叉检查；保留硬链接身份与不同路径，检查子目录父链 |
| 文件 | 内联正文、全部 extent-tree 层级；稀疏空洞和 unwritten 区域读零，已分配内容缺失 / 越界报错；保留 EOF 外预分配 |
| reflink | 共享 extent、内联和多叶 refcount 树；验证物理引用覆盖与同一文件内的重叠引用；返回 COW 后的逻辑字节 |
| 名称与链接 | 大小写敏感、UTF-8、255 字节名称和证据中的反斜杠；短 / extent 符号链接返回原始目标；设备、FIFO、socket 不当作普通文件读取 |
| 时间 | UTC atime / mtime / POSIX ctime 及独立纳秒；保留零值和负时间，不把 ctime 虚构为创建时间 |
| 扩展属性 | inode 内联、外部平坦块及索引 bucket；内联值和外联 value extent-tree；保留 user / system ACL / trusted / security 名称空间及二进制值 |
| 应用 | 裸卷 / 分区识别、浏览、预览、哈希、导出、直接范围读、目录快照和 CLI；属性复用命名流查看与导出接口 |

启用 metaecc 的元数据检查原始字节的 CRC32，包括目录尾、索引、分配组、
refcount 和 xattr bucket。CRC 不一致直接失败；不做 ECC 位修复。
范围读取复用已验证的映射，按逻辑偏移直接定位，不从文件头流式跳过。
所有宿主导出沿用新建文件、不覆盖规则，来源始终只读。

## 日志与限制

- 输入必须为完整、稳定、离线的卷视图。检查 slot map、每 slot 的 journal
  inode dirty 标志与 JBD2 superblock，要求日志无需重放、没有记录错误；
  local allocation 和 truncate log 必须为空。活动 slot 或恢复状态明确拒绝。
- 不执行在线集群协调、日志重放、写入、修复、删除恢复、孤儿扫描或历史快照浏览。
  不把未组装的共享存储成员当作完整卷；不声明完整 fsck 或全卷所有权审计。
- 未知 incompatible / readonly-compatible 特性、heartbeat-only、resize / tunefs
  进行中状态及非 UTF-8 名称明确失败；没有备用超级块恢复入口。
- 单树最多 100 万 extent、131072 个元数据块、深度最多 5；目录最多 100 万项。
  目录数据块 / 索引叶及属性 bucket 分别最多 131072 个。
  分配组最多 131072 个，位图合计最多 64 MiB；inode 缓存最多 128 项，内容映射仅缓存最近文件。
  单 inode 最多 65536 个属性、属性值合计 64 MiB、单值 64 KiB。
  工作量超限报错，不截断为成功结果。
- 原生验证覆盖四种块大小及 4 / 16 / 64 KiB 簇，单 slot 本地卷；
  更大簇、多 slot 集群卷和 JBD2 checksum-v2/v3 尚未完成独立原生核对。
  原生卷启用了 discontig 特性，实际分散子分配布局另以独立自制夹具覆盖。
  Windows Dokan 实际挂载仍需独立 GUI / 驱动验证。

## 验证

- `just test-ocfs2` / `just clippy-ocfs2` 检查独立 Python 自制的普通、metaecc、
  discontiguous 镜像及损坏、dirty、源故障、位图和边界场景；
  `just generate-ocfs2-fixtures` 可重复生成并记录 SHA-256。
  虚拟 8 GiB 源另行验证超过 4 GiB 的逻辑 / 物理偏移、直接范围定位与空洞零读。
- `scripts/ocfs2-native-fixtures.py --output <新目录>` 只格式化一次性 Linux
  测试环境中新建的普通镜像文件，调用 `mkfs.ocfs2`、Linux OCFS2 挂载和
  `fsck.ocfs2 -fn`。它复用相邻 GFS2 脚本的独立发明内容与原生清单生成代码，
  没有 Rust 编码器参与；所有测试内容为 CC0-1.0。
- 实际使用 Debian Linux `6.1.0-54-amd64`、`ocfs2-tools 1.8.7-1+b1` 生成
  四个 512 MiB 镜像：512 / 4096、1024 / 4096、2048 / 16384、4096 / 65536
  字节块 / 簇。第一组关闭 metaecc / indexed-dirs / inline-data / refcount，
  其余启用这些特性。每卷有 1500 项目录、12 MiB 文件、32 MiB 稀疏文件、
  1000 段碎片文件、未写入与 EOF 外预分配、长符号链接和内外联属性；支持
  reflink 的三组还包含交错 COW 的碎片克隆，触发多叶 refcount 索引。
- `OCFS2_ORACLE_DIR=<解码目录>` 后运行 `just test-ocfs2-native`：先验证镜像
  SHA-256，再逐一核对 1517 / 1519 个名称、模式、大小、纳秒时间、符号链接、
  全部属性及完整 / 范围内容。输入哈希见 ORIGIN.md；原生镜像保存在外部缓存。
  应用与 CLI 测试另行核对裸卷 / MBR、快照、来源身份、哈希和属性导出。

格式参考：[Linux OCFS2 on-disk 定义](https://github.com/torvalds/linux/blob/7a78f7ba9f24962c8033e0152bf34a7903ff61ba/fs/ocfs2/ocfs2_fs.h)。
