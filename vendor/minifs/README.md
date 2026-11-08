# minifs

独立、只读、纯 Rust 的 TP-Link / VxWorks MINIFS v3 读取库，AGPL-3.0-only。
不依赖 LovelyCarve、Tokio 或外部提取器。LZMA 复用 Apache-2.0 的
`lzma-rust2` 0.20.1；解析器为本仓库原创代码。

```rust
use minifs::{Limits, Reader};

fn main() -> anyhow::Result<()> {
    let bytes = std::fs::read("rootfs.minifs")?;
    let mut reader = Reader::open(bytes.as_slice(), Limits::default())?;
    for entry in reader.index().children("")? {
        println!("{} {:?}", entry.path, entry.file.map(|file| file.size));
    }
    let bytes = reader.read_range("/web/index.html", 0, 4096)?;
    println!("{} bytes", bytes.len());
    Ok(())
}
```

`ReadAt` 可接文件、mmap 或磁盘分区；输入必须从 MINIFS 魔数开始，且整个读取期间
保持不变。默认最多 100,000 个文件、16 MiB 元数据与展开路径预算、32 MiB 压缩 / 解压块、
16 MiB 字典、4096 字节路径、256 层目录。可通过 `Limits` 调整，不能用于不受信任
输入时任意取消限额。只缓存最后一个解压块；切块时先释放旧缓存。

`Index` 解析大端 32 字节头、名称表、20 字节文件记录与 12 字节块记录；块数取所有
文件块号的最大值加一，不依赖记录排序。验证名称起点与 NUL、UTF-8、安全路径、重名、
文件 / 目录冲突、整型运算、元数据及源范围、连续非重叠压缩块和文件逻辑范围。
目录按路径合成，保留文件名大小写；不跟随宿主路径，不写出证据文件。

文件数据按需解码，要求 LZMA-alone 的 EOS、精确输入 / 输出长度与合法最终 range-code。
pull 解码器通过严格输入检查真实源边界，push 解码器补验终态，两者结果必须一致。
验证期间还分配一个不超过块上限加一的缓冲区；不会把错误解压出的部分数据发布到缓存。
`read_range_with` 的回调在开始、pull 解码每 64 KiB 输入 / 输出及终态验证前后检查取消；
终态验证的一次调用最多处理一个受限块。缓存命中也先检查回调。

当前不支持旧式 20 字节头 + 88 字节固定文件表、无 EOS 的 LZMA、非连续块排列、
非 UTF-8 名称或加密变体。未知版本明确失败，不降级为成功的空目录。
未定义字段原样留在源中，不解释为时间、权限、校验和或删除状态。
MINIFS 不提供已知的文件内容校验和，结构和 LZMA 校验不等于固件签名认证。

格式依据：

- [Reverse Engineered MiniFS File System (2024)](https://arxiv.org/html/2407.05064v1)，
  32 字节头、名称 / 文件 / 块表和 LZMA 块；以官方 C6 实际字节交叉核对。
- [H4lo/MINIFS-Decompression](https://github.com/H4lo/MINIFS-Decompression)，旧布局参考；
  该项目无明确代码许可，没有复制、移植或调用其 Python 代码。
- [Archer C6 v4 研究记录](https://github.com/hacefresko/TP-Link-Archer-C6-EU-re)，
  用于定位官方固件下载地址；没有使用该仓库的解包结果作 oracle。

```text
just test-vendor minifs
just clippy-minifs
```

最小夹具与独立 Python 编码器采用 CC0-1.0，生成命令是 `just generate-minifs-fixtures`。
外部固件、SHA-256、逐文件 oracle 和宿主验证见
[MINIFS 格式指南](../../docs/formats/MINIFS.md)。公开厂商固件仍由厂商保留版权，
不随库分发。
