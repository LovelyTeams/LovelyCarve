# MBR / EBR

MBR 与 EBR 由独立的 Rust 代码解析，不引入 libvsmbr 源码、动态库或新依赖。本文以 [libyal/libvsmbr](https://github.com/libyal/libvsmbr) 的提交
[`ec39c3bf9bbb5ab42cf9b0939d1208f6eb256310`](https://github.com/libyal/libvsmbr/tree/ec39c3bf9bbb5ab42cf9b0939d1208f6eb256310)（2026-10-13 核对）为参照，
逐项检查了公开头文件、boot-record / partition-entry / volume / partition 实现、格式说明和测试目录。
这是能力与源码对照，没有编译运行 libvsmbr，下文的验证不是两个实现之间的差分比对。

## 实现入口

- `crates/core/src/evidence/partitions/mbr.rs`：MBR 探测、受约束的缺签名恢复、EBR 遍历和几何推断。
- `partitions/mbr/metadata.rs`：四个原始槽、类型字节、启动标志、原始及解码 CHS、磁盘签名与槽位字节偏移。
- `partitions/mbr/reader.rs`：只读、受分区边界约束的随机与游标读取。
- `evidence/image/open.rs`：对缺少可信几何的镜像采用 EBR 验证的扇区大小，然后进行分区与文件系统检测。
- `just run-example probe_partitions <image> --mbr`：输出所有已读 MBR/EBR 的原始槽位信息及恢复告警。

`MbrTable::read(source, sector_size)` 是分析接口：返回标准 MBR 表，包括有签名但没有数据分区的空表。
`MbrParser` 是自动探测接口：空表继续交给后续文件系统探测，防止把卷启动扇区认成空磁盘。
`MbrTable::records` 保留扩展槽、空槽及已读损坏 EBR；`partitions` 只包含经过范围验证的数据分区。
主 MBR 的非法启动标志或前两槽的起点越过镜像仍拒绝整表，以维持已有的卷启动扇区误识别防护。
需要查看拒绝的主记录时，可直接使用 `MbrBootRecord::decode`。

原始元数据通过后端 API 和探针提供；桌面 `Partition` / IPC 结构保持原有文件系统投影。
原始类型与文件系统类型是不同信息：`0x07` 不保证 NTFS，`0x83` 不保证 ext4，`0xAF` 也不保证 APFS。
`refine_partition_kinds` 继续使用卷签名细化；未知类型字节仍完整保留，未知文件系统不因此变得可浏览。

## 格式、遍历与容错

| 项目 | libvsmbr 基准行为 | LovelyCarve |
| --- | --- | --- |
| 标准记录 | 固定读取 512 字节，签名位于 510–511 | 相同；大扇区也读取其最前 512 字节 |
| 四个主槽 | 跳过类型 0，收集数据分区 | 四槽原始信息保留；类型 0/长度 0 不发布数据分区 |
| 磁盘签名 | 读取 440–443，现代签名位置也用于经典 MBR | 保留原始 u32；已有 ImageInfo 以 `0xXXXXXXXX` 显示非零值；零值为 None |
| 类型字节 | 公开 get-type；名称表只在 debug 编译下存在 | 全部 256 值原样保留；不将所有旧类型伪装为支持的文件系统 |
| FAT、隐藏分区等投影 | 类型名称提示，不读取文件系统 | 投影 FAT12 / 隐藏 FAT12、隐藏 FAT16/32、隐藏 NTFS / WinRE、Darwin UFS、BSD/OS、Linux RAID；`0xAF` 默认 HFS+，再由签名细化 |
| CHS 与启动标志 | 内部 flags；CHS 在 debug 输出中解码 | 原始 flags/CHS 和解码接口常驻；保留零/饱和值；不用 CHS 猜 LBA |
| 扩展类型 | 主表 0x05/0x0F；EBR 仅 0x05 作为链接 | 主表与 EBR 均支持 0x05/0x0F/0x85 |
| 多个扩展入口 | 一表超过一个扩展槽时报错 | 所有入口均可遍历；主数据槽先输出，再按扩展入口和槽位顺序深度优先输出 |
| EBR 数据槽 | 遍历四槽 | 相同，保留原始 record_lba / slot |
| 相对寻址 | 数据相对当前 EBR，链接相对首 EBR | 相同；全程 u64 LBA，加法在已校验的设备扇区范围内 |
| 循环和分叉 | 递归上限及自身链接保护；不支持分叉 | 迭代遍历与全局 visited 集合；循环/重复链接告警，分叉可读 |
| 遍历预算 | 最大递归深度 256 | 全表最多读取 256 个 EBR；到限告警并保留已验证结果 |
| 分区 ID | int 索引 | 现有 u8 ID 允许 256 个数据分区；第 257 个明确报错，禁止回绕 |
| 扇区大小 | 支持指定 512/1024/2048/4096；首 EBR 读失败时逐级增大 | 四种均支持；首个结构与范围有效的 EBR 可推断几何；没有有效 EBR 时不猜 |
| 有主数据分区的大扇区 MBR | 通过扩展入口推断几何 | 即使 512 字节探测已找到主数据分区，也先验证 EBR 几何，避免沿用错误的字节偏移 |
| 容器/物理盘几何 | setter 与库自动设置标记 | EWF、VDI、VHD/VHDX、PMFX、物理设备的已知几何不由 MBR 推断覆盖；RAW/DMG/VMDK/QCOW2/组装源可推断 |
| 起点与完整长度 | 主要按表值建立读视图 | 数据和扩展入口必须完整落在镜像内；逻辑分区及链接范围必须落在所属扩展容器内；数据起点不能覆盖自己的记录 |
| 损坏 EBR | 无效签名或读失败报错 | 无效签名、非法启动标志、短记录、越界链接告警并停止该分支，保留其他已验证分区；实际来源 I/O 错误带 EBR LBA 上下文传播 |
| 缺 MBR 签名 | 拒绝 | 有条件恢复：所有槽结构合法、不重叠、至多一个活动槽，且至少一个主数据分区命中支持的卷签名 |
| 空表与卷启动扇区 | 可以打开签名匹配的空表 | 分析接口可读空表；自动探测不认领，卷启动扇区继续走文件系统识别 |
| GPT | 类型 0xEE 可被作为普通数据类型；没有 GPT 表解析 | GPT 解析优先于 MBR；保护 MBR 不取代有效 GPT |
| 分区字节读取 | sector-vector/cache 加 Read/Seek 风格游标 | SectorSource 上的边界视图；复用证据源缓存和解压读者；提前短读明确失败 |

实现刻意使用恢复式告警处理可隔离的表损坏，并传播来源 I/O 错误；不会自动修补原始字节，也不把错误长度截成看似有效的分区。
签名表的重叠数据范围不会被自动删除或重排，便于分析原始布局；缺签名恢复仍要求主范围不重叠。

标准 CHS 柱面解码采用 `byte[2] | ((byte[1] & 0xC0) << 2)`。
基准提交的内部 `libvsmbr_chs_address_copy_from_byte_stream` 将移位后的十位直接作为 cylinder，
对于非饱和值与此标准字节布局不同；本实现保留原始三字节并用独立期望值验证，不复制这一行为。

## 全部 38 个公开 API

范围为 [include/libvsmbr.h.in](https://github.com/libyal/libvsmbr/blob/ec39c3bf9bbb5ab42cf9b0939d1208f6eb256310/include/libvsmbr.h.in) 中所有 `LIBVSMBR_EXTERN` 声明，包含条件编译的 wide/BFIO 项。
“对应”表示可完成同一取证操作，非 C ABI 或返回码兼容；“宿主替代”表示使用已有 Rust/应用生命周期机制；“不适用”说明原因。

<details>
<summary>展开全部 38 个公开函数的对应关系</summary>

| libvsmbr API | LovelyCarve 对应及边界 |
| --- | --- |
| `get_version` | 宿主替代：Cargo 包版本和应用版本；没有加载 libvsmbr 动态库 |
| `get_access_flags_read` | 对应：SectorSource / MbrPartitionReader 均为只读接口 |
| `get_access_flags_read_write` | 不适用：仅为上游访问标志便利函数；上游 open 本身拒绝 WRITE；本工具不写回证据 |
| `get_access_flags_write` | 同上；没有磁盘分区表写入能力 |
| `get_codepage` | 不适用：MBR/EBR 不含需要字符编码解释的文本；路径由 Rust/操作系统处理 |
| `set_codepage` | 同上；不引入全局 codepage 状态 |
| `check_volume_signature` | 对应：读取前 512 字节后 `has_boot_signature`；不代表完整表有效 |
| `check_volume_signature_wide` | 对应：Rust Path/PathBuf 打开 Unicode 路径，使用相同签名检查 |
| `check_volume_signature_file_io_handle` | 对应：SectorSource::read + has_boot_signature；适用于容器逻辑流与分区切片 |
| `notify_set_verbose` | 宿主替代：tracing 过滤级别，默认 warn，RUST_LOG 开启详情 |
| `notify_set_stream` | 宿主替代：应用 tracing subscriber 配置 |
| `notify_stream_open` | 宿主替代：应用日志 sink；没有独立 MBR 日志文件句柄 |
| `notify_stream_close` | 宿主替代：日志 sink 生命周期 |
| `error_free` | 宿主替代：Rust Result/error 的 RAII |
| `error_fprint` | 宿主替代：顶层错误输出与 tracing |
| `error_sprint` | 宿主替代：Display / to_string |
| `error_backtrace_fprint` | 宿主替代：anyhow 错误上下文链及顶层诊断 |
| `error_backtrace_sprint` | 宿主替代：`format!("{error:#}")` 上下文链；Rust backtrace 由运行配置决定 |
| `volume_initialize` | 对应：MbrTable::read；对象由 Rust 管理 |
| `volume_free` | 对应：drop MbrTable / DiskImage / reader |
| `volume_signal_abort` | 对应：MbrTable::read_cancellable 接收调用方 CancellationToken，读取前及遍历中直接检查；游标字节读取不另建任务/取消状态 |
| `volume_open` | 对应：DiskImage::open + MbrTable::read；镜像封装仍由统一证据读者打开 |
| `volume_open_wide` | 对应：Path/PathBuf Unicode 路径，无重复 wide API |
| `volume_open_file_io_handle` | 对应：MbrTable::read(&dyn SectorSource, sector_size) |
| `volume_close` | 对应：Rust drop；桌面证据关闭仍由既有 remove_image_runtime 清理 |
| `volume_get_bytes_per_sector` | 对应：MbrTable::sector_size / DiskImage::sector_size / ImageInfo |
| `volume_set_bytes_per_sector` | 对应：MbrTable::read 的显式参数；更改几何须重新解析，不修改已建立的读视图 |
| `volume_get_disk_identity` | 对应：records[0].disk_signature，原始 u32 含零；ImageInfo::disk_id 为非零格式化值 |
| `volume_get_number_of_partitions` | 对应：MbrTable::partitions.len()；不包含扩展容器 |
| `volume_get_partition_by_index` | 对应：MbrTable::partitions.get(index)；越界为 None |
| `partition_free` | 对应：Rust drop；reader 不拥有源，不能提前释放证据源 |
| `partition_get_type` | 对应：MbrPartition::entry.type_byte；全 256 值保留，不与 PartitionKind 混淆 |
| `partition_get_volume_offset` | 对应：MbrPartitionReader::volume_offset()，逻辑分区已加当前 EBR 基址 |
| `partition_read_buffer` | 对应：std::io::Read；EOF 为 0，越过分区尾截到尾，尾前短读为错误 |
| `partition_read_buffer_at_offset` | 对应：MbrPartitionReader::read_at；与上游 seek-then-read 不同，保留本地游标位置；需要更新游标时用 Seek + Read |
| `partition_seek_offset` | 对应：std::io::Seek，Start/Current/End；负数或溢出拒绝且游标不变；可以 seek 到 EOF 后 |
| `partition_get_offset` | 对应：Seek::stream_position() |
| `partition_get_size` | 对应：MbrPartitionReader::size()；LBA 长度经 checked_mul 转为字节 |

</details>

## 上游文档范围与限制

上游 [格式文档](https://github.com/libyal/libvsmbr/blob/ec39c3bf9bbb5ab42cf9b0939d1208f6eb256310/documentation/Master%20Boot%20Record%20%28MBR%29%20partition%20table%20format.asciidoc)
提及 classical、modern、AAP、NEWLDR、AST/NEC/SpeedStor 和 Disk Manager 变体。
基准 `libvsmbr_boot_record_read_data` 实际固定从 446 读取四个标准槽，并没有 AAP 附加项、八槽或十六槽变体的独立解析器。
LovelyCarve 同样解析标准四槽部分；不把“文档描述过”计为“库已实现”。CHS-only 分区缺少磁盘几何时也不能可靠恢复 LBA。
这两项属于双方当前边界，未宣称支持。EFI 系统、Solaris、VMFS 等类型可枚举和读取原始字节，类型识别不等于其文件系统支持。

GPT / APM / BSD / LVM / mdadm / 文件系统解析是宿主额外提供的能力，不属于 libvsmbr 的对照范围。

## 验证

```powershell
just test core::evidence::partitions::mbr
just test opens_raw_mbr_with_primary_and_large_sector_logical_partitions
just run-example probe_partitions <image> --mbr
```

`mbr/tests.rs` 覆盖主分区、EBR 四槽、超界起点、NTFS 启动消息误识别与缺签名恢复。
`mbr/regressions.rs` 使用独立逐字段构造的内存样本，验证原始元数据、非饱和 CHS、四种扇区大小、两级相对链接、
三种扩展类型、多入口与分叉、重复/循环链接、完整边界、非法 EBR、全 256 类型、超过旧 64 记录的长链、
256 分区 ID 边界与遍历预算、游标语义、seek 溢出、提前短读、I/O 错误及取消。
`image/tests.rs` 用临时 RAW 文件经过完整 DiskImage::open 路径验证大扇区 MBR，并包含已有主数据分区的场景；另验证 EBR 几何推断不能覆盖已识别的 GPT。
测试不依赖用户检材，也不修改进程环境。
