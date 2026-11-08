# LovelyCarve JFFS2 adaptation

来源：[xmoezzz/jffs2-rs](https://github.com/xmoezzz/jffs2-rs)，0.2.1，固定提交
`52e1234ce66b0e099b35afabff3fc3e3078a2c0e`；MIT，Copyright (c) 2023 xmoe。
保留原始 [LICENSE](LICENSE)。版本表示选用的上游基准，不代表上游包含本地改造。

沿用上游 `Jffs2Inode` / `Jffs2Dirent` 的字段基础与 RTIME 算法，拆分为节点、
扫描、版本索引与编码模块。修正固定小端、忽略 CRC、按 inode 覆盖目录项、
累加片段作为文件大小、顺序拼接正文的问题。RTIME 增加输入、输出和回引用检查。
用 `ReadAt` 读取来源，不 mmap / 解包到宿主，也不引入路径清理或覆写导出逻辑。

未复制或构建上游 `build.rs`、`src/c`、C LZO / Rubin、FFI、CLI、文件导出与
宿主路径处理。zlib 使用现有 `flate2/zlib-rs`，LZO 使用 `lzokay 2.0.1`（MIT）；
其许可证全文已记录在根 [第三方声明](../../THIRD_PARTY_NOTICES.md) 和
[codec 许可证](../ubidump/CODEC_LICENSES.md)。

支持新格式大小端、完整元数据 CRC、延迟数据 CRC、最高版本目录 / inode、
删除标记、硬链接、非重叠片段索引、范围 / 流式读取、NONE / COPY / ZERO /
RTIME / zlib / LZO。没有日志损坏容错重同步、NAND OOB、旧格式、Rubin、
厂商 LZMA、历史 / 删除恢复或 xattr 投影。完整边界见
[JFFS2](../../docs/formats/JFFS2.md)。

独立测试夹具由 Python stdlib 逐字段构造，不调用本库或 mkfs；CC0-1.0。
公开输入与独立已记录输出通过环境变量选择，仓库不重新分发这些输入。

```text
just test-vendor jffs2
just clippy-jffs2
```
