# HPFS

LovelyCarve 用纯 Rust 只读解析 OS/2 HPFS，支持功能版本 2 和 3。识别同时检查分区内第 16 扇区的超级块和第 17 扇区的备用块，可直接打开裸卷或磁盘镜像内的 HPFS 分区。

目录通过 dnode B 树枚举，保留磁盘上的原始拼写。CLI 优先按原名查找，在忽略大小写后只有一个匹配项时也可解析路径。目录项的创建、修改和访问时间保留为无时区的本地时间。文件数据从 fnode 直接 extent 或 anode 多级分配树读取。对坏扇区重映射表中已有的热修复映射，读取时逐扇区应用，覆盖文件数据和元数据。

普通文件可以预览、范围读取、哈希和导出。分配图有缺口、指向卷外、分配树存在循环或目录结构冲突时返回错误。可浏览带 dirty 标志的卷中仍可验证的目录与文件。来源和时间角色见 [读取器来源](../../src-tauri/vendor/hpfs-reader/ORIGIN.md)。

## 限制

- 为常见 DOS OEM 和东亚代码页提供文件名解码。遇到缺少解码映射或无法解码的名称会报错。
- 扩展属性、HPFS386 ACL、配额及其私有扩展、删除项恢复和逐文件 `dataRuns` 尚未接入。Linux 使用 HPFS 扩展属性存储的符号链接也未处理。
- 不执行磁盘修复、写入或位图重建。热修复表里的已有映射只用于只读寻址。

## 验证

独立 `hpfsutils` 生成的镜像包含 513 项的多节点目录、空文件和跨扇区载荷。测试核对目录顺序、每个条目的内容、哈希和导出。另有基于该镜像构造的两级分配树和坏扇区映射，验证跨界读取及损坏父指针拒绝。

```powershell
just test-vendor hpfs-reader
just clippy-hpfs
just run-cli system
python scripts/test-cli.py src-tauri/target/debug/lovelycarve-cli.exe hpfs
```
