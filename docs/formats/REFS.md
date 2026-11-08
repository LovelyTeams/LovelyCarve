# ReFS

ReFS 由本地的纯 Rust crate `refs-reader` 读取。桌面、CLI、只读挂载和快照构建都不加载或分发 `librefs-13.dll`；
旧 DLL 及其 FFI 代码只在显式的 `refs-dll-oracle` 特性下编译，用作对照工具，正式读取路径不会回退到 DLL。
节点 ID 沿用 `refs:` / `refs-snap:` 前缀。

## 实现范围

读取器覆盖分区相对的精确读取、卷头、superblock / checkpoint、碎片化元数据节点、虚拟块映射、object table、
递归目录、硬链接、驻留与非驻留数据、extent 子树、索引及外置属性列表、稀疏与未写入区间、命名流（ADS）、
文件流快照和 CoW 共享块。

- 普通文件与快照流都按 extent 映射直接随机读取，未写入区间读零；流式读取校验完整长度。
- 目录缓存限制为 128 个目录 / 16 MiB，随卷实例释放。
- 快照扫描在挂载、元数据读取和目录项之间检查调用方的 `CancellationToken`，不额外创建轮询线程。
- 重解析链接保留目标，读取时返回 UTF-8 目标文本；FILETIME 保留 100 ns 精度，长记录按磁盘上的 MACB 顺序解码，
  硬链接数据保留文件名记录中的时间。
- 读取通道直接使用 `DiskImage`，因此可以复用各种镜像容器和解密后的证据字节，无需先导出 RAW。

验证范围是下文三份 ReFS 3.14 公开镜像和结构单元测试，不宣称覆盖所有 ReFS 版本、损坏卷恢复、数据完整性校验、
写入或日志回放。元数据结构与引用都有边界检查，但读取成功不等于 CRC / SHA 校验已经认证。

## 来源

移植自 [b3nguang/refsprogs 的 DLL 分支](https://github.com/b3nguang/refsprogs/tree/feature/build-librefs-dll)
提交 `391c1bd56986142fc359288b7d2cb41cb3eb3680`，包含 Erik Larsson 的 refsprogs 解析逻辑与命名流扩展。
crate 保留 GPL-2.0-or-later 的 `LICENSE` 和 `ORIGIN.md`，许可说明见 [第三方声明](../../THIRD_PARTY_NOTICES.md)。

选型时评估过以下现成的 Rust 实现，在相同镜像上均不能满足需求，只作为探针的开发依赖保留：

| 候选 | 来源 | 结果 |
| --- | --- | --- |
| `refs-core` 0.1.0 | [SecurityRonin/refs-forensic](https://github.com/SecurityRonin/refs-forensic) | 4 KiB / 64 KiB 镜像均找不到根 object 0x600；发布版没有文件内容提取 |
| Rust `librefs` 0.1.0 | [docs.rs](https://docs.rs/librefs/0.1.0/librefs/) | 4 KiB 根目录报 `volume corrupt: key u64`；64 KiB 挂载报 `hash_type out of range (value=4)` |
| `AlanIWBFT/refs-linux` | [源码](https://github.com/AlanIWBFT/refs-linux) | Linux 内核模块，依赖内核 API 与 C VFS shim，不是跨平台的用户态读取器 |

`librefs` 发布包登记的仓库 ReFS-Linux/librefs 已无法访问，评估使用 crates.io 上的原始包（VCS revision
`a8a89b1a09bcf0c4b91b0b2970c5a88562c297ba`）。它的 B+tree 读取器固定使用 4096 字节页，快照管理器的内存列表也不能证明能读取磁盘上已有的快照。

## 公开镜像

样本来自 [xbqt/forefst](https://github.com/xbqt/forefst/tree/d4d5fd28942eb14ebe11d90f4eb9342f5e3d7b92/analysis/samples/disks)，
固定到提交 `d4d5fd28942eb14ebe11d90f4eb9342f5e3d7b92`；压缩包哈希与该提交的 Git LFS 指针一致。

| 镜像 | ReFS / 簇 / 元数据校验 | RAW 逻辑大小 | 压缩包 SHA-256 |
| --- | --- | --- | --- |
| win11refstestmftecmd | 3.14 / 4 KiB / CRC64 | 4,294,967,296 | dc4fb09a134a1b16d289e192d6a9dbc8a5e6779f278735ff78372b4cf259cfb1 |
| wininsiderrefs8gtest2 | 3.14 / 64 KiB / SHA-256 | 8,589,934,592 | 274d0387656a1f6e36e1128e60d9baa0f71bc37d511914cc68f238323ea61d64 |
| win11refs2tsnapshots | 3.14 / 4 KiB / CRC64 | 2,199,023,255,552 | 36d6a02992f333af5fba2c2663688c6d9e7add124412cd317b6dbf663fb15913 |

解压后的 4 GiB RAW SHA-256 为 `f7392dc51e9228f292efb10152181781fc424a0a64baf1c8a5c3d9d71eee400e`，
8 GiB RAW 为 `246d3c2f969ff6a67d6a9879723de50c20152b0e11d61034e38c4c7851c24ffc`。2 TiB 镜像需要解压为稀疏文件
（逻辑长度与 zstd frame 一致，非零数据约 608 MiB），没有另算完整 RAW 的哈希。

镜像、第三方验证脚本和 JSON 报告放在被忽略的 `fixtures/refs-experiment/` 中，不提交到仓库。下载时把 `<name>` 换成表中名称：

```powershell
curl.exe -fL "https://media.githubusercontent.com/media/xbqt/forefst/d4d5fd28942eb14ebe11d90f4eb9342f5e3d7b92/analysis/samples/disks/<name>/<name>.raw.zst" -o "fixtures/refs-experiment/<name>.raw.zst"
Get-FileHash "fixtures/refs-experiment/<name>.raw.zst" -Algorithm SHA256
zstd -d --sparse "fixtures/refs-experiment/<name>.raw.zst"
```

## 内容验证

| 镜像 | 目录项 / 遍历目录（含根） | 命名流 | 与独立解析器比对的普通文件 |
| --- | --- | --- | --- |
| 4 KiB | 1744 / 238 | 675 | 1063 个，长度与 SHA-256 全部一致 |
| 64 KiB | 894 / 119 | 245 | 529 个，长度与 SHA-256 全部一致 |
| 2 TiB 快照 | 304 / 35 | 13，其中 10 个快照 | 267 个，长度与 SHA-256 全部一致 |

全部 2,942 个目录项的文件名、目录身份、非链接大小、MACB 时间和链接目标都与 forefst 发布的 CSV 一致；链接大小采用
UTF-8 目标文本的长度，与原始 DATA 大小区分。快照镜像中 5 个文件共 10 个历史版本（5 字节到 3,202,036 字节）另用
forefst 独立恢复比对，另有 923 个 ADS 的完整长度与 SHA-256 一致。独立解析器固定在同一提交，其 Python 代码只在
被忽略的目录中运行，没有复制进本项目。extent 的 WRITTEN / HOLE 标志、MACB 时间字段和 CoW 共享块的解释参考了该项目的
磁盘结构分析，并以真实字节和独立哈希验证。

旧 DLL 无法挂载 64 KiB 镜像；在 4 KiB 镜像上只能列出 599 项 / 73 个目录，另有 10 项目录、链接或短读错误。一个
149 MiB 的预分配文件暴露出 DLL 没有处理未写入 extent：DLL 的哈希为
`8c63cbc67d8ff5481e01ea2f42ec770669239811932e294a8b286a879845a269`，Rust 读取器与独立解析器一致的哈希为
`c7d96f97bfab0380aa99d8c0c8ffdec69715d637febcc74e5e83cc8292ecb664`。读取器不会为了与旧 DLL 一致而输出未写入区间中的残留数据。

checkpoint 位置：4 KiB 镜像为物理块 123892 / sequence 617，64 KiB 为 15637 / 161，快照镜像为 5368580 / 265。
偏移表按磁盘字段定位，修正了旧 C 实现中 SHA-256 引用布局与物理 checkpoint 地址的问题。

## 复现

`probe-refs` 配方在仓库根目录运行（`lovelycarve-core` 的 example），镜像与输出路径请使用绝对路径；`--output` 以 `create_new` 创建文件，重复运行需换新的报告名。
`--backend all|dll|refs-core|librefs|ported` 默认为 `all`，候选失败时保留原始错误并返回非零；`ported` 的完整读取会核对读出长度与声明大小。

```powershell
just test-refs
just clippy-refs
just probe-refs <镜像绝对路径> --backend ported --walk --output <新报告.json>
just probe-refs <快照镜像绝对路径> --backend ported --walk --file /testsnapshots/test.txt --output <新报告.json>
$env:REFS_RUST_IMAGE = '<4 KiB 镜像路径>'
just test --jobs 1 --lib production_reader_traverses_streams_and_seeks_without_the_dll -- --ignored
$env:REFS_RUST_SNAPSHOT_IMAGE = '<2 TiB 快照镜像路径>'
just test --jobs 1 --lib production_snapshot_streams_keep_versions_ranges_and_partial_output_contracts -- --ignored
$env:REFS_ORACLE_IMAGE = '<4 KiB 镜像路径>'
just test-refs-oracle --jobs 1 -- --ignored
```

`just test-refs` 覆盖结构、extent、短读、稀疏、版本隔离、流描述符与时间；`just test-refs-oracle` 的忽略测试在 4 KiB 镜像上与 DLL 对照，
并断言未写入区间采用独立解析器确认的结果。设置上述环境变量后，应用层测试还会核对普通文件的完整哈希、随机范围与 EOF，
2 TiB 镜像中 10 个快照版本的哈希与 `refs-snap` 范围读取，以及 `.dirz` 快照与实时目录的一致性。
