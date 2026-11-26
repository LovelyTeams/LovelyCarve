# AIX JFS1

LovelyCarve 用独立的纯 Rust 读取器解析 AIX 原始 JFS。应用识别类型为 `aixjfs`，可打开已提取的 AIX JFS 逻辑卷镜像或包含该文件系统的分区。IBM 后续的 Linux JFS2 由另一读取器处理。读取始终只读。

支持大端 `fsv3` 和 `fsv3p` 的标准分配布局、512/1024/2048/4096 字节的片段、每个分配组中的 inode 表、直接与一级和二级间接寻址。主超级块位于 4096 字节，备用副本位于 126976 字节。主副本失效时可从几何有效的备用副本打开，两个有效副本的关键字段冲突时拒绝读取。保留硬链接 inode、内联和外部符号链接，以及访问、修改和元数据变更时间的纳秒精度。

文件预览、范围读取、哈希和导出使用同一读取路径。普通文件的空洞返回零。对非零地址，读取会检查整段实际分配范围和逻辑 EOF；读取短碎片时不会进入相邻物理区域。

## 限制

- 当前支持标准 4 KiB 逻辑块布局和不超过 2 GiB 的文件。大文件专用的 128 KiB 分配布局、压缩卷、日志重放、ACL 和删除恢复尚未实现。
- 目录名称和链接显示沿用宿主的 UTF-8 文本契约。不能无损解码的名称或链接文本会明确报错，链接原始字节仍可由读取器取得。
- 需要能找到文件系统起始位置的数据源。AIX LVM 的多盘逻辑卷重组未接入。
- 测试使用公开磁盘结构生成的合成镜像，尚未和上游未公开的 AIX 4.3.3 原生语料互验。上游实现以其原生语料做过读取核对。

## 验证

五组镜像包含一个原始 `fsv3` 卷和四种 `fsv3p` 片段尺寸。覆盖跨分配组 inode、目录和链接、短碎片、稀疏数据、坏指针和备用超级块。CLI 另测带 MBR 的坏主副本镜像、按节点和路径读取、二级间接偏移、哈希及导出。

```powershell
just test-vendor aix-jfs-reader
just clippy-aix-jfs
just run-cli system
python scripts/test-cli.py src-tauri/target/debug/lovelycarve-cli.exe aixjfs
```

参见 [IBM 超级块说明](https://www.ibm.com/support/pages/where-are-superblocks)、[IBM 碎片与间接寻址说明](https://www.ibm.com/docs/en/aix/7.1.0?topic=volumes-allocating-jfs-file-space) 和 [读取器来源](../../src-tauri/vendor/aix-jfs-reader/ORIGIN.md)。
