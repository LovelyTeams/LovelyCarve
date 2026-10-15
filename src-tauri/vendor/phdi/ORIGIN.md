# 来源与实现范围

- LovelyCarve 原创 Rust 实现，2026-10-13，AGPL-3.0-only，见 LICENSE。
- 格式 / 能力参照：Joachim Metz 与贡献者的
  [libyal/libphdi](https://github.com/libyal/libphdi/commit/fb132dceb2a1a43c2597b10bbf9e65757d08cf7c)，
  固定提交 `fb132dceb2a1a43c2597b10bbf9e65757d08cf7c`。
  上游库 LGPL-3.0-or-later，工具 GPL-3.0-or-later，格式文档 GFDL-1.3-or-later。
- 参考公开格式结构、API 与差分读取语义；未纳入上游 C 实现、文档正文、
  夹具或二进制，不提供 libphdi C ABI，也不声称由 libyal 发布。
- Rust 依赖：quick-xml 0.42.0（MIT），由 Cargo.lock 固定；禁止 unsafe。
- `tests/reader.rs` 和宿主 / CLI 测试是本项目生成的合成结构。
  `tests/oracle.py` 为原创、独立于 Rust mapper 的 Python 单稀疏 extent 解码器。
- 外部公开验证样本来自 dfVFS，保留在仓库外；不分发样本或原文。
  固定来源、输入 / 输出 SHA-256 和验证边界见 [PHDI](../../../docs/formats/PHDI.md)。

这里是适合 Rust 宿主的只读能力重写，未翻译 C 错误对象、日志流、codepage
全局状态、libbfio 句柄池和写访问标志；明确拒绝不支持的布局。
