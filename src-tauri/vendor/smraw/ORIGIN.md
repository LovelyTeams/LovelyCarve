# 来源与实现范围

- 实现：LovelyCarve 原创 Rust 代码，2026-10-13；AGPL-3.0-only，见 LICENSE。
- 能力参照：Joachim Metz 与贡献者的 libyal/libsmraw，固定提交
  [adfa6b8bc8617226805926180244c774366aad30](https://github.com/libyal/libsmraw/commit/adfa6b8bc8617226805926180244c774366aad30)。
  上游库 LGPL-3.0-or-later，工具 GPL-3.0-or-later。
- 参考其公开 API、RAW 拼接与输出命名语义；没有复制上游 C 实现、文档正文、
  测试数据或二进制，不声称由 libyal 发布，也不提供其 C ABI。
- 运行依赖：仅 Rust 标准库。禁止 unsafe，不依赖 libsmraw/libbfio 或 DLL。
- 合成测试：本项目编造的字节、编号文件、Unicode 信息与 GPT 分段，测试运行时
  在临时目录生成；CLI 的独立拼接参考由 Python 构造，无真实检材。
- 独立 vendor 工作区同时作为宿主 path 依赖；两份 Cargo.lock 固定 smraw 0.1.0。

当前范围是 RAW 字节读写、分段发现、句柄资源上限、取消与调用者元数据。
显式强行模式支持所选分片起点的连续前缀、短读诊断、完全越界清零与 Windows 单次临时错误重试；
宿主负责合成字节审计。其行为与默认严格读取独立，不宣称 CD 裸扇区或 EWF 兼容性。
`.info` sidecar、C 错误/日志对象、libbfio 池和已有检材原位写入不在范围内。
全部 72 个公开 API 的对应与主动差异见
[docs/formats/SMRAW.md](../../../docs/formats/SMRAW.md)。
