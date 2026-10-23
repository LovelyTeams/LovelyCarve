# Native YAFFS1 / YAFFS2 reader

本仓库原创只读 Rust 库，AGPL-3.0-only。`wire.rs` 的前缀结构与字段校验复用并改造
binwalk-ng `845806c163747eb524d66b5fe3de2ed86ea3aaad` 的 MIT Rust 实现；许可与版权
保留于 `LICENSE-BINWALK-MIT`。没有引入 Binwalk 的外部提取入口或完整依赖图。

按用户要求先评估 Rust 库，再补充原生能力。Binwalk 只有部分头解析；另一候选 Disrobe
的清单/专有许可不适合直接复用，未查看或引入其 YAFFS 实现。此库补充有界几何、标签、
奇偶校验、版本索引、目录和范围读取。没有构建 C/C++ 或执行外部解包工具。

格式参考 Aleph One `474b3acb927d27b2305618aaf24456b9d33fe91b` 的对象头、packed tags、
扫描顺序与长度/时间定义；仅参考格式事实。ECC 独立使用 Rust 奇偶矩阵实现。
独立 Python 编码器生成 CC0 合成输入，公开输入只做环境变量控制的外部测试。

YAFFS1 扩展位于 `legacy_tags.rs` / `legacy_scan.rs`，共用头、命名空间与范围接口。
仅根据格式事实独立实现分散 tags、两位 serial 与页正文 ECC；不做证据纠错。
默认长度来自对象头；已知其他挂载配置可显式选择 `FileSizeSource::DataPages`。
详细范围和公开输出核对方法见 [YAFFS1](../../../docs/formats/YAFFS1.md)。

宿主通过 `Box<dyn ReadOnlyFilesystem>` 持有只读接口；具体索引、分配和析构留在此库，
不会把目录 / 数据页容器的具体类型传播到应用。调用方始终提供同一不可变字节来源。

支持范围、取消 / 内存预算、未校验数据与不可区分的整页截断、NAND ECC / 时间配置限制
见 [YAFFS2 指南](../../../docs/formats/YAFFS2.md)。库保留独立 workspace，使用根目录
`just test-vendor yaffs2-reader` 和 `just clippy-yaffs2`。
