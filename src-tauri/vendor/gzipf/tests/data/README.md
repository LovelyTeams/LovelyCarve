# 独立 GZIP 夹具

这些文件由本项目编造的文本和 0–255 字节序列组成，没有真实检材或
libgzipf 测试数据。许可证为 AGPL-3.0-only。

`python src-tauri/vendor/gzipf/tests/data/generate.py` 使用 Python 标准库的
C zlib 构造 stored、fixed Huffman、dynamic Huffman、全部可选头字段以及
五成员拼接流（含三个空成员）。生产读取器使用 Rust miniz_oxide 解码，
因此夹具编码与生产解码互相独立。`SHA256SUMS` 记录提交文件的哈希；
不同 zlib 版本可能生成不同但等价的压缩字节。生成环境版本见 ORIGIN.md。

Rust 测试另外用手工 stored DEFLATE 和按位 IEEE CRC32 编码最小样本，
验证截断、损坏、边界和资源限制。生成器无需在普通测试时运行。
