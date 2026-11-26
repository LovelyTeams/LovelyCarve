# UnixWare Boot File System

`unixwarebfs` 是 UnixWare `/stand` 启动文件系统。LovelyCarve 使用纯 Rust 只读访问小端卷，支持 `0x1BADFACE` 超级块、512 字节分配块、平面根目录、普通文件和硬链接。根目录的已分配空间可以大于逻辑长度。名称字段最长 14 字节，恰好占满时无需以 NUL 结尾。

裸卷和单独提取的分区可直接打开。包含 UnixWare 的整盘镜像可以通过 `0x63` MBR 分区中的经典 VTOC 找到 `/stand`。VTOC 的起点按整盘绝对 LBA 解释，浏览与字节读取仍使用原证据源。原始 MBR 槽位可通过 [MBR 分区探针](MBR.md) 查看。

文件支持预览、范围读取、哈希与导出。读取前校验 inode 的逻辑长度、已分配的连续物理区间和卷边界。保留同一 inode 的多个目录名称、Unix 权限以及秒级访问、修改和元数据变更时间。

## 限制

- VTOC 入口支持 512 字节扇区和经典内嵌的前 16 个槽，扩展切片表和坏扇区替代表尚未接入。经典槽号 0 描述整个 UnixWare 分区。
- 中断的文件搬移需要执行文件系统恢复，读取器会拒绝打开此类卷。已释放的目录槽不进入浏览结果。名称须符合宿主路径所需的 UTF-8 编码。
- 空卷由 util-linux 实际格式化。含文件的样本和 VTOC 整盘包装由测试构造。

## 验证

```powershell
just test-vendor unixware-bfs-reader
just clippy-unixware-bfs
just run-cli system
python scripts/test-cli.py src-tauri/target/debug/lovelycarve-cli.exe unixwarebfs
```

参见 [UnixWare 启动文件说明](https://uw714doc.sco.com/en/SM_startup/_Where_the_Boot_Programs_Reside.html)、[UnixWare `prtvtoc` 手册](https://uw714doc.sco.com/en/man/html.1M/prtvtoc.1M.html)和[样本来源](../../src-tauri/vendor/unixware-bfs-reader/ORIGIN.md)。
