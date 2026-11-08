# LovelyCarve lzma-rs adaptation

来源：crates.io `lzma-rs 0.3.0`，MIT，Guillaume Endignoux；发布包记录提交
[`da82bd118299`](https://github.com/gendx/lzma-rs/commit/da82bd1182993a07db035e285a8f8a87d32a30c2)。
保留原始 [LICENSE](LICENSE)、源码与独立 workspace。未引入上游 C `rust-lzma`
开发依赖；补齐发布包排除但 inline stream 测试需要的公开 `tests/files/small.txt`。

本地增加 `LzmaDecoder::decompress_checked`：正常化最终 range code，要求其为零
且底层输入实际 EOF。原 `decompress` API 保持原行为，调用方明确选择严格接口。
对已知长度、无结束标记的旧 SquashFS LZMA 块，该检查不能仅由“已产生足够输出”
替代；否则截短的 EOS 流也可能产生声明长度的错误成功。

backhand 在有界输出写入器内调用严格接口，并与 `lzma-rust2` 的输出和源消耗核对。
字典、属性和输出上限在调用前校验。该 vendor 没有 C/FFI 解码器，也没有修改
其他 LZMA 读取路径的宽容策略。stream 专用缓冲访问方法只在对应特性下编译。

本地单元测试使用相邻 backhand 的独立 NanaZip / Python 夹具，覆盖真正的无 EOS
流、所有截断位置、尾随字节、声明长度变化与截短 EOS 的反例；不需要运行外部
压缩程序。完整文件系统边界见 [SquashFS](../../docs/formats/SQUASHFS.md)。

```text
just test-vendor lzma-rs --features raw_decoder,stream
```
