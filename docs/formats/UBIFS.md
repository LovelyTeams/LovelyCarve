# UBIFS 文件系统

LovelyCarve 可直接打开裸 `.ubifs`，也可浏览 UBI 命名逻辑卷、受支持固件封装中的
UBIFS。读取器沿用 [rust_ubidump](https://github.com/aspnmy/rust_ubidump) 的 Rust
节点、键、inode 与索引结构，在同一 vendor 中改为有界只读接口；压缩数据使用
现有 Rust 生态的 `flate2/zlib-rs`、`lzokay` 与 `ms-compress-ruzstd`。
来源和本地修改见 [vendor 说明](../../vendor/ubidump/LOVELYCARVE.md)。

## 能力与完整性

- 支持格式 v4 / v5 的 simple key，R5 和小端 test hash；R5 按有符号字节计算，
  保留中文名称和合法名称哈希碰撞。名称必须能表示为 UTF-8 路径组件。
- 超级块、主节点、索引和 inode / 目录项校验节点 JAMCRC、长度与类型。
  数据节点在实际读取时验证整个压缩节点的 CRC，随后按需解码，不在列目录时展开所有正文。
- 从超级块正确读取 min-I/O / LEB 几何；主节点在两个 master LEB 中按对齐步长查找，
  按提交与序号选择有效记录。一个副本 CRC 损坏时可用另一个；同一提交的有效记录
  根索引冲突则拒绝，不回退到历史目录来掩盖错误。
- 遍历所选提交的索引，不扫描散落节点拼成文件树。检查层级、分支键范围、叶节点键、
  节点重叠 / 循环、目录父链、同名项及目录引用的 inode 类型。
- 普通文件按 4 KiB 数据键寻址，缺失块与短数据节点后部按 UBIFS 语义补零，按 inode
  逻辑长度裁剪；完整、流式和范围读取共用同一路径。已分配节点的越界、源短读、CRC
  或解压错误明确失败，不转换成空洞。
- 读取 stored、LZO、raw DEFLATE（UBIFS 的 `zlib` 压缩类型）和 Zstandard。
  检查精确输出长度、终止与尾随输入；有 Zstandard 校验和时核对其值。
  单个逻辑数据块最多 4096 字节，Zstandard 字典窗口最多 1 MiB。
- 保留硬链接的各个目录路径、inode 号、模式和有符号秒 / 纳秒级 Unix 时间戳；
  atime、mtime、ctime 分别进入对应角色，不把 ctime 当作出生时间。
  符号链接展示目标，不跟随到宿主路径；库保留设备/FIFO/socket 类型，当前文件树以零长度节点展示，读取其正文明确失败。
- 原始检材字节、哈希与 RAW 导出仍对应原始文件。UBI 内嵌卷和固件明文只是文件系统
  读取视图；缓存键包含来源分区 / UBI 卷号，压缩文件不虚构连续物理 data runs。

## 限制

- 当前读取干净且没有待处理孤儿的已提交树。脏卷、恢复状态或待清理孤儿明确拒绝，
  尚不提供 journal replay、未提交文件、历史提交或删除恢复。
- 不支持加密、认证 / 签名 UBIFS、其他 key format、未来 RO compatibility 版本，
  不解码 NAND OOB / ECC；不提供 Zstandard 外部字典。名称与符号链接目标须为 UTF-8，包含反斜杠的名称不能投影为当前路径节点。
- 扩展属性节点和内联值保留在源中并检查其节点结构，不作为目录项或独立数据流投影；
  当前文件树接口不展示 UID/GID、xattr 与原始时间粒度等完整 inode 属性。
- LPT / 空闲空间与分配恢复尚未接入，因此不提供文件系统未分配空间节点。
- LEB 上限 16 MiB、索引深度 64、目录深度 256、索引节点数 1,000,000，
  索引/元数据的节点字节预算 512 MiB。单次文件读上限 256 MiB，较大文件使用流式或范围读取。

## 验证与依据

```text
just test-vendor ubidump
just clippy-ubi
just test ubifs_host --lib
just test-cli
```

独立 Python 夹具覆盖 v5、多级索引、哈希碰撞、中文名称、硬链接、符号链接、
空文件、跨块空洞、负秒数和纳秒时间、主节点副本、CRC、层级 / 键 / 循环 / 重叠、
未知标志、截断与源短读。压缩夹具分别使用 Python raw DEFLATE、独立 LZO literal
编码、Zstandard raw-block frame，以及 Node/libzstd 生成的带校验和压缩帧。
宿主测试另覆盖乱序 UBI PEB、Buffalo ENC / TRX 明文视图及原始字节保留。

Unblob 的公开 `banana.zlib.ubifs` / `banana.lzo.ubifs` / `banana.zstd.ubifs` 与其
独立记录的四个文件输出核对字节、哈希、流式和范围读取。前两个样本实际使用 stored
数据节点，文件名不能证明解压覆盖；真正压缩路径由独立夹具验证。公开来源、长度、
哈希和读取结果见[样本清单](firmware-samples.json)。这些不是所有厂商固件的兼容承诺。

```powershell
$env:LOVELYCARVE_UBIFS_IMAGES = '<公开镜像1>;<公开镜像2>'
just test ubifs_host_external --lib -- --ignored
$env:UBIFS_EXTERNAL_SAMPLES = '<公开镜像1>;<公开镜像2>'
just test-vendor ubidump ubifs_external -- --ignored
```

格式交叉依据为 Linux v6.12 的
[ubifs-media.h](https://github.com/torvalds/linux/blob/v6.12/fs/ubifs/ubifs-media.h)、
[key.h](https://github.com/torvalds/linux/blob/v6.12/fs/ubifs/key.h)、
[compress.c](https://github.com/torvalds/linux/blob/v6.12/fs/ubifs/compress.c) 与
[crypto/deflate.c](https://github.com/torvalds/linux/blob/v6.12/crypto/deflate.c)。
它们用于核对格式语义，没有编译或运行内核文件系统代码。
