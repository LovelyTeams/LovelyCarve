# Veritas VxFS

LovelyCarve 的纯 Rust 只读解析器覆盖 VxFS 磁盘布局 2–4。UnixWare 使用的小端超级块位于卷起始后 1024 字节，HP-UX 使用的大端超级块位于 8192 字节。应用将它识别为 `vxfs`。HP-UX 资料中也称它为 JFS，与 AIX 和 Linux 的 IBM JFS 分属不同格式。

读取从对象定位表开始，沿结构文件集和主文件集的 inode 列表打开根目录。支持 inode 内联目录和数据、带哈希头的常规目录、硬链接身份、符号链接、直接 extent、第一张传统间接表和多级 typed extent。文件预览、范围读取、哈希和导出共用同一逻辑读取路径。时间保留访问、修改和元数据变更的微秒精度。

范围读取跨稀疏空洞时返回零。元数据缺失、extent 超出卷、分配记录重叠或对象定位表有环时返回错误。卷数据通过 RAW、其他镜像容器或已得到正确边界的分区输入。

## 限制

- 当前读取主文件集。额外文件集、带外部设备号的 extent、第二张传统间接表、VxFS 5 及以上版本、原生快照和扩展属性尚未提供。
- 读取镜像中已有的元数据。不重放意图日志，也不从原始 VxVM 设备集合重建逻辑卷。应用的逐文件 `dataRuns` 目前留空。
- LovelyCarve 验证样本是按公开磁盘结构生成的合成镜像。上游在 HP-UX 11.11 VxFS 4 上的实机验证属于上游实现，当前适配器尚未用其未公开的原生镜像语料复核。

## 验证

三组镜像分别覆盖 layout 2 小端 1 KiB、layout 3 小端 2 KiB、layout 4 大端 8 KiB。读取器测试覆盖链式对象定位表、内联和哈希目录、范围读、5 GiB 之后的稀疏文件标记、坏映射和目录边界。CLI 对同一组镜像核对路径和节点 ID、硬链接 inode、哈希、导出和时间，并用 MBR 包装大端样本复核分区内识别。

```powershell
just test-vendor vxfs-reader
just clippy-vxfs
just run-cli system
python scripts/test-cli.py src-tauri/target/debug/lovelycarve-cli.exe vxfs
```

格式参照与可重复样本的来源见 [读取器来源](../../src-tauri/vendor/vxfs-reader/ORIGIN.md)。
