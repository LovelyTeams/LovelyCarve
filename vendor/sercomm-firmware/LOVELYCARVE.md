# Sercomm firmware reader

本仓库原创 Rust 库，AGPL-3.0-only，与 LovelyCarve 主程序相同；不是外部仓库的改名
或移植。保留独立 Cargo workspace，便于对容器解析和解码单独验证。

先复用已有 Rust 能力：`zip` 读取 ZIP 索引 / 元数据，`flate2` 进行有界 raw DEFLATE
并验证完整结束，`crc32fast` 校验输出，已加固的 `ubidump` 校验逻辑 UBI。
布局依据：OpenWrt firmware-utils
[`b2a93c6f77ba/src/mksercommfw.c`](https://github.com/openwrt/firmware-utils/blob/b2a93c6f77bac3a6525e93b383c987b073b4633b/src/mksercommfw.c)
和 [OpenWrt v24.10.1 `netgear_sercomm_nand`](https://github.com/openwrt/openwrt/blob/v24.10.1/target/linux/ramips/image/mt7621.mk)。
没有复制、构建或调用这些 C / shell 工具。

支持一个 stored / DEFLATE ZIP32 `.bin` 成员、512 字节 seal、匹配的尾部、固定 NAND
位置与 UBI 布局。逐块检查取消、解码长度和 CRC，拒绝短流、尾随压缩字节、本地 /
中央记录不一致、加密、data descriptor、ZIP64、多成员和未验证布局。
编码源和解码 NAND 各限 512 MiB。

`NandImage` 的不可变结果分别提供压缩来源范围和解码逻辑空间，不能相互冒充。
返回的内核字段只检查布局；宿主必须在发布视图前通过已有 uImage 读取器核对其头和
数据 CRC。宿主为 UBI 建立 native filesystem view，保留原检材 ID 和源分段几何，
原始镜像哈希 / RAW 导出不经过逻辑视图。

夹具由独立 Python stdlib 编码；stored 输入以 zlib 压缩运输，避免提交固定 NAND
填充产生的大文件。来源、许可和哈希在主仓库 `test_data/firmware/` 中。

```text
just test-vendor sercomm-firmware
just clippy-sercomm
just test firmware_sercomm --lib
```

范围、宿主验证及可选外部样本测试见 [固件指南](../../docs/formats/FIRMWARE.md)。
