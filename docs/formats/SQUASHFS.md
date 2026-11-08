# SquashFS 文件系统

LovelyCarve 复用并加固 Rust [backhand](https://github.com/wcampbell0x2a/backhand)
读取文件树，支持裸镜像、受支持的固件分段及 UBI 内嵌卷。原始证据仍只读打开，
压缩文件的逻辑读取不会替换源哈希或 RAW 导出内容。

## 版本与编码

| 版本 | 当前范围 |
| --- | --- |
| 1.0 | 标准大小端，最大 32 KiB 数据块；旧式 16 位块表、零长度编码表示 32 KiB；UID 高位编码与 GID 继承 |
| 2.0 / 2.1 | 标准大小端，最大 64 KiB 数据块；32 位块表、共享尾片段、v2.1 长目录与目录索引；v2.0 允许未排序目录 |
| 3.0 | 标准大小端、DD-WRT `hsqt` / `tqsh`，以及经验证的 adaptive / 标准 LZMA 路径 |
| 4.0 | 标准大小端；宿主启用 gzip、xz 与 Zstandard；标准 LZO / LZ4 和 v4 adaptive LZMA 特性尚未启用 |

v1/v2 在同一个 backhand vendor 中新增有界读取路径，复用版本无关的文件树、
文件描述符与范围 / 流式读取接口；没有引入另一个只面向 v4 的读取器。
对照候选为 [squashfs_reader](https://github.com/Macdu/squashfs_reader/tree/900b4779cf0b57a11711abf66197613588f85520)
和 [squashfs-async](https://github.com/cpg314/squashfs-async/tree/8b55fb7e0197885b3b9435dcae45568a3c8a6791)，
其超级块均限定为 v4。v3/固件封装的补充说明见 [FIRMWARE](FIRMWARE.md)。

## v1/v2 完整性与元数据

- 按镜像字节序读取数值和位域，校验超级块声明长度、块大小 / log、UID/GID、
  inode / 目录 / 片段表的顺序与完整范围。源截断或短读明确失败。
- 元数据逐块解析，允许 inode / 目录跨 8 KiB 块边界，引用必须落在实际块的
  解码范围内；检查记录重叠、名称、重复项、目录类型、目录环及深度。
  `check_data` 字节是格式标记而非 CRC，不将其描述为数据校验和。
- 保留硬链接的每个路径；v1/v2 没有现代 inode 编号，不生成伪造编号。
  支持普通文件、目录、符号链接、块 / 字符设备、FIFO 与 socket。
- 文件和目录保留其记录的有符号秒级 Unix mtime，包括 1970 年之前的值。旧特殊 inode 没有 mtime 时保持未知，
  不把文件系统创建时间代入。ctime / atime / 出生时间未记录，不补造。
- stored 与 zlib 数据均检查逻辑长度；zlib 核对 Adler32、完整结束和全部输入。
  v2 尾片段在读取时检查实际解码长度，分配数据越界不能伪装成空洞。
- LZMA 属性头 / EOS 解码复用 `lzma-rust2`。旧 v2 省略 EOS 的块使用 inode
  已知长度；必须同时满足精确输入 / 输出范围、两个解码接口一致，以及独立的
  `lzma-rs::decompress_checked` 最终 range code 为零、真实源 EOF 和相同输出。
  后者是本地补充的严格接口，避免截短的 EOS 流被误当作无 EOS 流。
  有效 zlib 头之后的校验或 EOF 错误不会尝试其他编码。
- 裸卷、固件明文与 UBI 卷使用同一读取路径。卷读取器缓存已建立的元数据与文件块计划，
  范围读取只展开与请求相交的数据块；原始源读取及操作级坏区审计沿用证据层。

## 原生 Rust 压缩读取

宿主的 gzip / DEFLATE 使用 `flate2` 的 `zlib-rs` 后端，LZMA / XZ 使用
`lzma-rust2`，Zstandard 使用 `ms-compress-ruzstd`。固件读取不调用 FMK、
`unsquashfs`、liblzma 或 libzstd。backhand 的独立 `zstd-write` 特性只保留
上游可选写入接口，宿主不启用它；应用其他功能的压缩依赖不属于这条读取路径。

依照 [XZ 格式规范](https://tukaani.org/xz/xz-file-format.txt)，XZ 检查块校验、索引记录、流尾与完整输入，只接受单个流；支持 LZMA2 与
最多三个 BCJ / Delta 预过滤器，包括 SquashFS 常用的 x86、PowerPC、IA64、
ARM、ARM Thumb 与 SPARC，以及组合过滤器链。先检查索引、块 / LZMA2 chunk 长度和
字典，再交给 Rust 库解压；字典上限 8 MiB，单块输出上限 4 MiB。Zstandard 检查完整单帧和可选内容校验和，窗口最多 8 MiB，
输出最多 4 MiB。两条路径均先完成校验再发布输出，失败不留下部分解压内容。

## 限制

- 不提供写入、历史 / 删除恢复、文件系统未分配空间或 NAND OOB / ECC 解码。
  无法识别的版本、位域、数据编码或不完整流明确报错。
- 路径须能表示为 UTF-8；名称中的反斜杠无法投影为当前路径组件。
  符号链接目标不跟随到宿主路径；特殊节点只展示类型，不产生可导出的设备正文。
- v1/v2 元数据预算 512 MiB，索引和路径预算分别为 256 MiB；最多 1,000,000 个
  inode / 可见路径，目录深度 256。单次文件内容读取最多 256 MiB，大文件使用
  范围或流式读取；LZMA 声明字典最多 8 MiB，实际历史窗口按块输出上限裁剪。
- 无 EOS 的共享片段需要其现存引用提供完整已知长度；存在未知尾部时拒绝读取。
  没有 CRC 的 stored / 旧 LZMA 数据不提供内容认证，格式边界检查不等于加密校验。
- v1/v2 仅支持本文列出的布局；其他厂商的 magic、压缩前缀或混合字节序需独立验证。

## 验证与来源

```text
just test-vendor backhand --features v1_v2,v3_ddwrt,v3_lzma
just test-vendor lzma-rs --features raw_decoder,stream
just clippy-squashfs
just test squashfs_legacy_host --lib
just test-cli
```

独立 Python 夹具逐字段生成 v1.0、v2.0/v2.1 大小端、压缩 / stored 元数据、
check marker、跨元数据块 inode、片段表、长目录、UID/GID、硬链接和各特殊类型。
NanaZip / 7-Zip 另生成无 EOS 的 LZMA 块，Python/liblzma 独立核对其字节；
截断测试逐个检查每个压缩长度，包含截短 EOS 的专门反例。来源与哈希见
[夹具说明](../../vendor/backhand/test_data/legacy/README.md)。

Unblob 的标准 v1/v2 大小端样本与其独立记录的文本输出核对；Binwalk 的 v2 BE
样本另覆盖无 EOS LZMA、嵌套树与全文件 / 流式 / 范围一致性。公开输入的来源、
长度、哈希与当前读取结果见[样本清单](firmware-samples.json)，不代表所有同类固件。

```powershell
$env:SQUASHFS_LEGACY_EXTERNAL = '<公开镜像1>;<公开镜像2>'
just test-vendor backhand --features v1_v2 legacy_external -- --ignored
$env:LOVELYCARVE_SQUASHFS_LEGACY = '<公开镜像1>;<公开镜像2>'
just test squashfs_legacy_host_external --lib -- --ignored
```

旧布局的格式交叉依据是 squashfs-tools 的固定提交
[`db038ef265ed`](https://github.com/plougher/squashfs-tools/tree/db038ef265ed00bb17aaf695a72f9b0c3489eb20/squashfs-tools)
中的 `squashfs_compat.h`、`unsquash-1.c` 和 `unsquash-2.c`；FMK 的旧 v2 LZMA
工具用于核对属性头和已知输出长度。没有编译或复制其 C/C++ 解包实现。

XZ 的各 BCJ / 校验模式以及 Zstandard 校验帧由 Python/liblzma、Python
`zstandard` 独立生成；测试逐个截断长度，并覆盖尾随输入、伪造索引、坏校验、
过大字典 / 窗口与超限输出。原始生成器、CC0 声明和 SHA-256 见
[压缩夹具](../../vendor/backhand/test_data/native-codecs.json)。
