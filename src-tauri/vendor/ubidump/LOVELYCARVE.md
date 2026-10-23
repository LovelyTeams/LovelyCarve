# rust_ubidump 的 UBI / UBIFS 库适配

来源：[aspnmy/rust_ubidump](https://github.com/aspnmy/rust_ubidump)，
固定提交 `19782c3eabc25a1baab7faefe12fe6f4bd362dac`（0.1.0），MIT。
保留上游 `LICENSE` 中 Willem Hengeveld 的版权声明；上游说明其源自 Python ubidump。

此目录将所需的 UBI 层裁剪、改造为独立只读库。保留并修正 EC、VID、VTBL
解码器与 JAMCRC；不引入上游 CLI、路径导出或 C LZO 依赖；UBIFS 节点和索引解析另行按下述规则适配。

- 将 `File` 和重开路径改为有界 `ReadAt`，可读取宿主镜像内任意分区，不生成整卷临时文件。
- 修正上游 VTBL 记录长度 200 → 172；CRC 覆盖前 168 字节，名称长度超过 127 时返回错误。
- 区分物理 PEB 与逻辑 LEB，按 VID 的卷号、逻辑块号与序号重组，重复序号明确拒绝。
- 校验 EC / VID / VTBL CRC、几何、镜像序列、卷类型、保留容量、填充与更新标记。
  静态卷与复制块验证数据 CRC；静态卷不得缺块，末块按 `data_size` 裁剪。
- 有效卷表副本须一致；一个副本 CRC 损坏时可使用另一个，两份都损坏或有效副本冲突则失败。
- 动态卷未映射 LEB 按 UBI 擦除语义返回 `0xff`，不把未分配块当成源短读。
- 提供逻辑范围读及 `Read + Seek`，不打印诊断或直接写出文件。

支持范围、限制及宿主测试入口见 [UBI 固件格式说明](../../../docs/formats/UBI.md)。
`src/tests.rs` 用独立的逐位 CRC 算法构造合成镜像，验证乱序、多副本、跨块、
擦除洞、静态末块、CRC、损坏与短读。运行 `just test-vendor ubidump`。

## UBIFS 节点与只读文件系统

`filesystem_nodes.rs`、`filesystem_index.rs`、`filesystem.rs` 依据同一提交的
`src/ubifs/{node,key,index,inode,dir_entry,superblock,master,data}.rs` 结构与字段解码改造，
保留 MIT 来源；与 Linux v6.12 的 ubifs-media.h / key.h / compress.c 交叉核对。

- 将文件路径重开和无界读改为共享的有界 ReadAt，复用已有 JAMCRC。
- 修正 node_type 偏移 14 → 20；按真实 LEB/min-I/O 扫描两个主节点区，而非固定 4 KiB 步长。
- 按提交与序号选择 CRC 有效主节点；同提交根冲突拒绝，脏/恢复/待清理孤儿状态拒绝。
- 检查索引层级、键范围、叶子键、节点位置、重叠和循环；仅遍历所选索引，不扫描历史节点导出。
- R5 使用有符号字节与回绕运算，保留合法哈希碰撞。修正上游超级块部分字段和 inline inode 解码。
- 按数据块号补空洞、裁 inode 长度，支持直接范围读取；源短读和分配节点 CRC/解压错误不能变成空洞。
- 符号链接与 xattr inode 内联数据按原始字节解释，不按默认 compr_type 错误解压。
- UBIFS zlib 类型使用 Linux crypto deflate 的 raw DEFLATE，而非上游的 zlib 包装解码。
  复用 flate2/zlib-rs、lzokay 2.0.1 和 ms-compress-ruzstd 0.9.1；不链接 C LZO 解码器。
- Zstandard 检查窗口、逻辑输出、帧终止、尾随输入及可选校验和。

限制、资源上限、实际执行的夹具类别与外部样本入口见 [UBIFS](../../../docs/formats/UBIFS.md)。
合成夹具和生成器均为原创 CC0-1.0，来源和哈希见 [test_data/README.md](test_data/README.md)。
