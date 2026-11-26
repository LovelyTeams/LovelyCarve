# Xbox DVD File System

LovelyCarve 对 Xbox DVD 的 XDVDFS 游戏区提供纯 Rust 只读访问。支持裸 XISO 和 XGD1、XGD2、XGD3 整盘镜像，使用 2048 字节扇区。整盘的游戏区起点通过卷描述符的双签名和根目录范围确认。镜像前部可独立识别为 ISO 9660 或 UDF 时，仍可浏览该视频区。

目录按原生二叉树解析。名称使用 Windows-1252 解码，按 Xbox 工具采用的 ASCII 大小写规则查找，并保留原名。文件预览、范围读取、哈希和导出都读取游戏区内经过边界校验的连续文件内容。目录循环、重叠目录项和越界指针会明确报错。

## 限制

- 当前接受以 2048 字节用户数据扇区保存的游戏区。CSO 和 Games on Demand 容器、游戏授权与安全扇区解析不在本读取器的范围内。
- 文件目录项不含每文件时间戳。读取器保留文件名和大小，文件时间保持为空。
- 裸 XISO 由独立的 `extract-xiso` 生成。XGD1、XGD2 与 XGD3 的偏移包装及 ISO 9660 前缀由测试构造。

## 验证

```powershell
just test-vendor xdvdfs-reader
just clippy-xdvdfs
just run-cli system
python scripts/test-cli.py src-tauri/target/debug/lovelycarve-cli.exe xdvdfs
```

参见 [读取器与样本来源](../../src-tauri/vendor/xdvdfs-reader/ORIGIN.md)。
