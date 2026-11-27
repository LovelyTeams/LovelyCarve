# HAMMER2

LovelyCarve 使用纯 Rust 只读打开 DragonFly BSD HAMMER2 版本 1 和 2 的单物理卷。根目录列出卷内发布的 PFS，例子中的 `DATA`、`LOCAL` 与 `ARCHIVE` 分别是数据 PFS、本地 PFS 和一个已发布的快照。

## 支持范围

- 校验卷头的扇区 CRC32C 和整块 CRC32C。在同一卷的有效副本中选择事务号最新的卷头。裸镜像和可定位的分区保留原始字节范围。
- 支持 PFS 根、目录间接块、长名称、嵌入 inode 的小文件、硬链接和符号链接。不同 PFS 独立解析 inode，即使快照和活动 PFS 引用相同的物理块。在 CLI 中跟随绝对符号链接时从链接所在的 PFS 开始。
- 支持普通、稀疏、LZ4 与 zlib 数据块。范围读取只访问与请求重叠的索引和数据块。稀疏空洞按格式定义补零。记录了校验码的数据块在解压前核对 CRC32C、xxHash64 或 DragonFly SHA192。
- 读取 inode 的微秒级时间与 POSIX 模式。目录查找区分大小写，文件名中的非 UTF-8 字节和会影响路径解析的字符按 `%HH` 显示。

## 边界与验证

- 当前只组装单物理卷。多成员文件系统、集群副本、已删除对象和未作为 PFS 发布的历史事务不在此读取器覆盖范围。原生镜像验证覆盖小端格式，反向字节序尚无原生样本核对。
- `just test-vendor hammer2-reader` 和 `just clippy-hammer2` 验证读取器。CLI 端到端用例覆盖裸卷、MBR、备用卷头、快照命名空间、链接、稀疏范围、哈希与导出。原生样本由 HAMMER2 `makefs` 生成。[读取器来源记录](../../src-tauri/vendor/hammer2-reader/ORIGIN.md)
