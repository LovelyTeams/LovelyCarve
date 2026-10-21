# rust_ubidump 的 UBI 库适配

来源：[aspnmy/rust_ubidump](https://github.com/aspnmy/rust_ubidump)，
固定提交 `19782c3eabc25a1baab7faefe12fe6f4bd362dac`（0.1.0），MIT。
保留上游 `LICENSE` 中 Willem Hengeveld 的版权声明；上游说明其源自 Python ubidump。

此目录将所需的 UBI 层裁剪、改造为独立只读库。保留并修正 EC、VID、VTBL
解码器与 JAMCRC；不引入上游 CLI、路径导出、UBIFS 树解析器或 C LZO 依赖。

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
