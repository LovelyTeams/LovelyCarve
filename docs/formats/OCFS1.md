# OCFS1

`ocfs1-reader` 只读打开 Oracle Cluster File System 第一代的 OracleCFS 1.2 磁盘格式，使用小端字段和 512 字节扇区。宿主使用独立的 `ocfs1` 类型识别裸卷和分区。OCFS2 的兼容卷头标记为 2.0，由 [OCFS2 读取器](OCFS2.md) 识别其真正的超级块。

| 能力 | 范围 |
| --- | --- |
| 卷 | 卷头、卷标、根目录位置、节点 publish 扇区和全局簇分配位图 |
| 目录 | 128 KiB 目录块、槽位索引和同级续链、嵌套目录、过滤已删除条目 |
| 内容 | 文件记录内置 extent、`EXTHDR2` / `EXTDAT1` 多层映射、跨 extent 范围读、文件哈希和导出 |
| 元数据 | UTF-8 名称、POSIX 模式、创建时间和修改时间，保留符号链接内容 |

要求完整的离线卷。仍记录独占挂载或有节点需要恢复时拒绝打开。所有已分配数据必须落在卷内并在全局分配位图中标为占用，缺少逻辑范围或源字节会报错。不执行在线锁协调、日志重放、写入、删除恢复或完整分配所有权审计。

`just test-vendor ocfs1-reader` 与 `just clippy-ocfs1` 检查解析器。`just test-cli` 检查裸卷与 MBR、目录续链、两种 extent 路径、范围读、哈希和导出。夹具按 Oracle 原始提交归档中的字段偏移独立生成。原生 `mkfs.ocfs` 镜像、早于 1.2 的格式及不同扇区几何尚待验证。[格式来源与夹具](../../src-tauri/vendor/ocfs1-reader/ORIGIN.md)
