# HAMMER

LovelyCarve 以纯 Rust 只读打开 DragonFly BSD HAMMER v1–v7 的单物理卷。支持裸卷和分区内的卷，保留证据中的原始分区位置。

- 读取当前根目录、常规文件、硬链接和符号链接。目录查找区分大小写。路径中的特殊字符和非 UTF-8 字节按 `%HH` 显示。inode 的修改、访问与状态变更时间保留微秒精度。
- 按 B-tree 中记录的创建、删除事务号选择可见记录。v3 起已登记在主 PFS 的快照显示在 `@snapshots/<16 位十六进制事务号>`，可浏览、哈希和导出当时的文件版本。
- 范围读取校验分配地址与记录 CRC，按请求读取对应的数据块，稀疏空洞按格式定义补零。支持早期 IEEE CRC32 和 v7 CRC32C，包括升级卷中保留的旧 CRC。格式化工具可能留空卷头 CRC，此时仍校验树、inode 和数据记录。
- 需要 UNDO 或 REDO 的卷在打开时明确报告。v4 起另检查日志边界两侧的序号，以识别卷头索引尚未更新的日志。多物理卷、PFS 镜像从卷、v1/v2 旧式快照链接及未登记的历史事务尚未覆盖。

`just test-vendor hammer-reader` 与 `just clippy-hammer` 覆盖读取器。CLI 用例验证裸卷、MBR、链接、历史文件、范围读取、哈希与导出。原生格式样本覆盖小端 v1 和 v7，反向字节序及 v2–v6 尚缺对应原生样本。[样本与上游验证记录](../../src-tauri/vendor/hammer-reader/ORIGIN.md)
