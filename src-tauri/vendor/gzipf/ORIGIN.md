# 来源与实现范围

- 实现：LovelyCarve 原创 Rust 代码，2026-10-13；AGPL-3.0-only，完整许可见 LICENSE。
- 能力参照：libyal/libgzipf，固定提交
  [d1b4e6ba5911e1b86a3f9d4ca3fd31f1d69a6b68](https://github.com/libyal/libgzipf/commit/d1b4e6ba5911e1b86a3f9d4ca3fd31f1d69a6b68)。
  作者 Joachim Metz；上游代码 LGPL-3.0-or-later，格式文档 GFDL-1.3-or-later。
- 格式依据：[RFC 1952](https://www.rfc-editor.org/rfc/rfc1952)。
- 仅参考格式语义和公开接口，没有复制上游 C 实现、文档正文、夹具或许可证
  下的其他内容，也没有提供二进制兼容层。本 crate 不声称由 libyal 发布。
- 运行依赖：`flate2` 1.1.10（MIT OR Apache-2.0）显式 `rust_backend`，
  `crc32fast` 1.5.2（MIT OR Apache-2.0）；完整解析版本记录于 Cargo.lock。
- 独立夹具：本项目编造，Python 3.10 / C zlib 1.2.12 生成；生产解码使用
  Rust miniz_oxide 0.9.1。没有提交真实检材。哈希、生成器和许可位于 tests/data/。

本 crate 保留独立 vendor 工作区，同时作为宿主 path 依赖；宿主 Cargo.lock
记录 gzipf 0.1.0，tar.gz / tgz、Docker / OCI 压缩层与小米备份 GZIP 分支
使用本 crate。接口对照、主动差异和验证范围
见 [docs/formats/GZIP.md](../../../docs/formats/GZIP.md)。
