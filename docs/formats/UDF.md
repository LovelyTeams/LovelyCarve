# UDF 只读文件系统

LovelyCarve 使用纯 Rust `lamfold-udf` 读取 UDF。库改造自
[lamco-admin/lamfold 固定提交 `fabaad6af0c28b80535307b339b84110f3a3c12e`](https://github.com/lamco-admin/lamfold/tree/fabaad6af0c28b80535307b339b84110f3a3c12e/crates/lamfold-udf)，
MIT OR Apache-2.0；没有链接 C 库、调用系统挂载或外部提取器。布局依据
[ECMA-167](https://ecma-international.org/publications-and-standards/standards/ecma-167/)
和 [ECMA TR/112-2 收录的 OSTA UDF 2.60](https://ecma-international.org/wp-content/uploads/ECMA_TR-112-2_1st_edition_december_2023.pdf)。上游归属和本地改造见
[LOVELYCARVE.md](../../src-tauri/vendor/lamfold-udf/LOVELYCARVE.md)。

## 支持范围

| 能力 | 实现 |
| --- | --- |
| 卷 | UDF 1.02 / 1.50 / 2.00 / 2.01 / 2.50 / 2.60，单个底层分区 |
| 元数据分区 | UDF 2.50 / 2.60。按文件项描述的区段翻译目录与元数据地址，保留普通文件的底层分区引用。主文件项或卷根解析失败时，尝试完整镜像副本 |
| 虚拟分区 | UDF 1.50 VAT 尾部结构和 UDF 2.x VAT 头部。读取当前记录末端附近的 VAT，支持内联或普通分配描述符，并校验每项的底层地址 |
| Sparable 分区 | 按最新有效 sparing table 映射重定位的 packet。替换范围可在底层分区之外，必须完整落在检材内 |
| 来源 | cooked 镜像和磁盘分区；512 / 1024 / 2048 / 4096 字节块，保留源分区身份和原始几何 |
| 识别 | 第 256、最后一块和倒数第 257 块的 Anchor VD Pointer；完整 tag checksum 和 CRC16，主 / 备用 VDS 兜底 |
| 卷描述符 | 同一 VDS 的 Partition / Logical Volume descriptor，按 sequence number 选择；校验块大小、分区号、map 和 FSD / root ICB |
| inode | File Entry（261）和 Extended File Entry（266），单块 direct ICB strategy 4 |
| 文件与目录 | 内联、short_ad、long_ad 和有界 Allocation Extent Descriptor（258）续链；普通文件、子目录、硬链接和符号链接 |
| 空洞 | 未分配和已分配但未记录的 extents 返回零；后者仍验证实际分配范围 |
| 路径 | OSTA compressed Unicode 8 / 16，保留大小写；检查非法字符、重复名与目录祖先循环 |
| 时间 | access / modification / attribute-change；EFE 的 creation time；已记录偏移转换为 UTC，保留微秒精度 |
| 工作流 | 浏览、预览、范围读取、流式读取、哈希、导出、目录索引和既有只读挂载路径，桌面与 CLI 共用 |

ISO/UDF bridge 的 cooked 镜像优先显示 UDF 树；ISO-only 的镜像继续使用 CDFS。
符号链接的 UDF Path Component 解码为目标文字，预览、范围读取、哈希和导出使用相同的
目标文字字节，不跟随到宿主路径。原始编码始终保留在源镜像中。

每次读取只访问所需 extents，文件不要求整份载入内存。范围、流式、哈希和导出以同一份
逻辑长度及映射为准，分配表覆盖不足、分配范围越界或来源短读必须失败，不补零冒充正文。
节点 ID 为 `udf:part:<分区ID>:<UTF-8路径hex>`；同一 inode 的不同硬链接路径保持独立节点。

## 限制与拒绝策略

- 多底层分区、跨卷引用、多会话重建、历史 VAT 浏览以及元数据镜像逐块回退未接入。VAT ICB 查找限于已记录分区末端的四个块。顺序介质的分区描述符可以声明大于已记录镜像的容量，读取边界以实际检材为准。
- 不支持 extended_ad、间接 ICB strategy、多块 ICB、chained VDS、多 file sets、named streams、
  transformed / multiversion 文件；不做日志回放、写入、修复、已删除文件恢复或未分配空间枚举。
  删除的 FID 跳过后继续读取后面的现存项；损坏的现存记录报错，不悄悄截断目录列表。
- 没有将 UDF 接到 2352 / 2336 字节 BIN 或 CUE 原始轨道投影；该入口仍以
  [CDFS](CDFS.md) 的范围为准。UDF 镜像应提供 cooked 文件系统字节。
- 时区为未知（UDF `-2047` 哨兵）的本地时间保持未知，不标为 UTC、不投影进时间线。
  EA、所有者等原生元数据未提供独立浏览入口；源字节不变。
- VDS 最多 1024 块，VAT 正文最多 64 MiB，续链最多 128 个 AED、65536 个 extents。单目录最多 32 MiB、262144
  个现存项；inode 缓存最多 4096 项且约 16 MiB，超限清理缓存；符号链接编码最多 64 KiB。
  宿主路径深度及单次完整读取还受共享适配层限额约束。

## 验证与边界

`tests/fixtures/udf102.zlib` 和 `udf201.zlib` 是上游 mkudffs 建卷、Linux UDF 内核写入的
公开镜像的无损压缩副本。测试以生成时的固定内容公式核对目录、内联文件、FE / EFE 和正文；
不是运行时使用另一个解析器进行逐字段比对。
另用 Ubuntu 官方包中的 mkudffs 2.3 建了空的 UDF 1.50 / 2.01 VAT 和 2.01 sparable 卷，
用同包的 udfinfo 核对版本、几何和 VAT 位置，Rust 测试读取其根目录。

独立 Python 生成器 `scripts/udf-fixtures.py` 逐字段编码四种块大小的 CC0 合成镜像，覆盖
short / long AD、续链、两类空洞、Unicode、硬链接、符号链接和已知 / 未知时区。
负例重算 tag / CRC 后再修改语义字段，验证错误分区引用、越界 extent、短逻辑映射、
续链循环、CRC 损坏、截断和不支持的 partition maps。高级映射夹具覆盖两种 metadata map 顺序、碎片与镜像回退、两代 VAT、sparing table 冗余和跨 packet 读取。来源、许可及 SHA-256 位于
[夹具 manifest](../../src-tauri/vendor/lamfold-udf/tests/fixtures/manifest.json)。

宿主测试覆盖裸镜像、MBR 双分区身份隔离、ISO bridge 路由及完整 / 范围 / 流式 / 导出一致性；
CLI smoke 使用两个上游镜像核对浏览、跨块范围读、SHA-256 和导出字节。
宿主测试使用合成夹具验证高级映射。Windows GUI / Dokan 实际挂载，以及 macOS / Linux 桌面运行仍需单独验证。

```text
just test-udf
just clippy-udf
just test udf
just test-cli
just generate-udf-fixtures
```
