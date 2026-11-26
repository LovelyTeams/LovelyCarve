# Xbox FATX / XTAF

LovelyCarve 使用纯 Rust 只读浏览初代 Xbox 的小端 FATX 和 Xbox 360 的大端 XTAF。两种格式均支持 16 位和 32 位分配表、簇链目录、有界文件范围读取、哈希与导出。浏览时跳过已删除的目录项。文件名原字节采用可逆的单字节映射，路径的 ASCII 字母可忽略大小写。

支持单独提取的卷、初代 Xbox 零售硬盘的固定分区和 XBPartitioner 的 LBA48 表，以及 Xbox 360 零售硬盘的系统扩展、兼容区和内容区。支持 Xbox 360 Devkit 表里的内容区和仪表板区，也能定位带系统缓存区的 Xbox 360 记忆卡数据卷。分区保留原镜像的绝对字节位置。

标准分配表计入保留项再向上按 4096 字节对齐。识别特定工具生成的紧凑 XTAF 头和分配表布局。簇链需要覆盖完整的逻辑文件大小，读取在逻辑 EOF 处停止。创建、修改和访问时间保留为未记录时区的本地时间，初代 Xbox 和 Xbox 360 分别使用 2000 年和 1980 年基准。

## 限制

- 支持的 Xbox 360 区域以 FATX/XTAF 卷签名为准。SFCX 系统缓存、Xbox 360 USB 容器 `Xbox360/Data000*`、STFS 和删除恢复属于额外格式或取证功能，目前未接入此读取器。
- 带文件的卷由独立工具生成。Devkit、LBA48 和记忆卡包装由测试构造，零售硬盘测试覆盖固定分区偏移和边界。

## 验证

```powershell
just test-vendor fatx-reader
just clippy-fatx
just test xbox_partition
just run-cli system
python scripts/test-cli.py src-tauri/target/debug/lovelycarve-cli.exe fatx
```

参见 [Xbox FATX 结构研究](https://aerosoul94.github.io/blog/2020/02/25/fatx-reading-and-recovery.html)及[读取器与样本来源](../../src-tauri/vendor/fatx-reader/ORIGIN.md)。
