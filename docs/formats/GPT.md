# GPT

GPT 由独立的纯 Rust 实现读取，不引入 libvsgpt 源码、DLL、Python 包或新依赖。vendored `gpt` crate
只提供类型 GUID 注册表；统一的 GPT 读取路径由宿主实现，避免严格解析与宽松恢复两条路径对长度、单扇区条目和 ID 的处理不一致。

本文以 [libyal/libvsgpt](https://github.com/libyal/libvsgpt) 的提交
[`e4e439f93c3a0b776590dbf80f102f3b58032ca3`](https://github.com/libyal/libvsgpt/tree/e4e439f93c3a0b776590dbf80f102f3b58032ca3)（2026-10-13 核对）为参照，
范围包括全部 42 个公开 C API、实际的 header / entry / MBR 读取实现、pyvsgpt、vsgptinfo 和格式文档。
上游标记为 experimental；本文按实际源码判断能力，不把文档里的类型名称或 TODO 算作已实现。

## 后端入口

- `core::evidence::partitions::gpt::GptTable::read(source, sector_size)`：保留识别到的 header、选用副本、非空原始条目及告警。
- `read_cancellable`：接收调用方 `CancellationToken`，读取间与条目间直接检查，取消返回错误。
- `GptHeader`：版本、头长度、reserved、当前/备份 LBA、可用范围、磁盘 GUID、条目位置/数量/步长、原始 CRC 与两项校验结果。
- `GptEntry`：原始 u32 槽位、字节偏移、类型/唯一 GUID、包含尾扇区的起止 LBA、全部 64 位属性、36 个原始 UTF-16 单元、范围有效性。
- `partition_count` / `partition_by_index`：只计范围有效的条目；稠密索引与磁盘原始槽位不同。
- `partition_by_entry_index`：按原始槽位查询，对应 libvsgpt 的 identifier 查询；这个 identifier 是 u32 槽位，**不是 GUID**。
- `partition_by_guid`：宿主额外的唯一 GUID 查询；nil 不作为标识，重复 GUID 返回歧义错误。
- `project`：产生既有 `Partition`；保留名称和唯一 GUID，类型仍由后续卷签名细化。
- `GptPartitionReader`：已验证条目的有界只读 `Read` / `Seek` / `read_at`，复用 `SectorSource`，独立游标。
- `infer_sector_size`：在没有权威来源几何时，按首选大小及 512/1024/2048/4096 探测；只有头结构和两项 CRC 验证通过才改变大小。

`detect_partitions_with_meta` 从同一次选表结果产生分区和磁盘标识，避免从损坏主头拿 GUID、从备份拿分区。
检测告警写入 tracing；直接分析接口通过 `warnings` 返回。
原始元数据通过 Rust API 和 `just run-example probe_partitions <image> --gpt` 查看。
桌面与 CLI 的 `ImageInfo` / `Partition` IPC 结构不含原始属性与主备诊断，界面也不展示这两项。

## 格式与恢复策略

| 项目 | libvsgpt 基准实现 | LovelyCarve |
| --- | --- | --- |
| 签名检查 | 在字节 512/1024/2048/4096 检查 `EFI PART` | `has_gpt_signature` 检查传入头；完整读取可显式指定四种扇区大小 |
| 扇区探测 | 从 512 逐次倍增至 4096，找到主头后使用该大小 | 四种均支持；自动改变几何要求头结构与两项 CRC；空的有效表也能确定几何 |
| 来源几何 | GPT open 内部从 512 开始推断 | EWF、VDI、VHD/VHDX、PMFX、物理盘的已知几何不被推断或旧 4Kn 兜底覆盖 |
| 标准/移位头 | 要求主头 current LBA 为 1 | 标准 LBA 1、原有恢复式 LBA 0/7/8、尾部备份及 leading 头指定的备份位置 |
| 仅备份存活 | open 必须先读取主头，主签名缺失时失败 | 主头清空后可直接恢复设备最后完整扇区的备份 |
| 备份位置 | 优先主头 backup LBA，找不到签名才用尾部 | 同时检查显式指针及尾部；镜像扩容后仍可选择原尾部有效副本；不遍历任意指针链 |
| 头长度 | 固定读 512 字节；要求长度 92..512 | 支持 92..逻辑扇区大小，CRC 覆盖声明的扩展头字节；损坏的零长度仅走显式告警恢复 |
| 头 CRC | 非零且不匹配标记 corrupt；零 CRC 不校验 | IEEE CRC32，校验字段置零；零值也与计算结果比较，不视为免校验 |
| 版本 | 不是 1.0 标记 corrupt | 非 1.0 或 reserved 非零标记未验证，不能用于改变几何 |
| 主备一致性 | 比较 reciprocal LBA、可用区间、GUID、数量、步长、数组 CRC 字段 | 同项比较，另比较 revision；分歧保留告警，确定选用一份，不混合条目 |
| 数组 CRC | 读取 stored CRC 并比较两份头的值；实际 entry reader 没有计算数组 CRC | 对完整声明数组计算 CRC，主数组坏而备份完整时优先备份 |
| 选择顺序 | 主要根据头 corrupt 状态选择，随后读取一份数组 | 头结构和两项 CRC 都有效的候选优先；都无有效副本才允许范围校验后的恢复 |
| 条目步长 | 接受 >=128，按声明步长遍历 | 接受 >=128 且为 128 的整数倍；解析固定前 128 字节，扩展字节不解释但纳入 CRC |
| 条目数量 | 声明条目数 >256 拒绝，即使大多为空 | 声明数组最多 1 MiB（128 字节条目最多 8192 槽）；整个数组超限明确拒绝该副本，不截取前缀 |
| 可浏览 ID | int 索引，raw entry_index 为 u32 | raw 槽位保留 u32；既有 u8 ID 可表示 256 个有效分区，第 257 个投影明确报错，不回绕 |
| 空表 | entry_count 0 返回成功；全空槽可打开 | 有效空表认领 GPT；未验证且无有效条目的候选不认领 |
| 空槽 | type GUID 全零跳过 | 相同；非空但范围无效的条目保留原始字段并设置 valid=false |
| GUID 字节序 | API 返回磁盘混合端序的 16 字节 | `Uuid::from_bytes_le` 解码为 canonical UUID；`to_bytes_le` 可取回磁盘布局；IPC unique_guid 仍用 canonical 字节 |
| 属性与名称 | entry reader 解码属性和 72 字节名称；公开 partition API 不提供两者 | 全部属性与原始 UTF-16 常驻；有效名称投影，非法代理对保留原单元并告警，不丢掉整个分区 |
| 单扇区分区 | 允许 end == start；长度为 end-start+1 | 相同，包含单扇区分区 |
| 完整范围 | 检查起点和尾点，但 end == sector_count 未被拒绝，且未检查 usable end | 要求尾点 < sector_count、完整范围落在声明可用区内；不能覆盖选用头、备份头或选用数组 |
| 数组/读取长度 | 声明长度完整读，不足时报错 | checked 地址计算、源范围校验、完整长度检查；越界元数据副本拒绝，真实 I/O 错误和提前短读带上下文传播 |
| 重叠/重复 GUID | 无对应完整验证与查询歧义检测 | 保留重叠条目并告警；重复唯一 GUID 告警，GUID 查询拒绝歧义 |
| 类型投影 | 原始 GUID；旧 `get_type` 返回 type 字节，GPT values 没有赋值 | GUID 常驻；Linux RAID / LUKS 投影为 Mdadm / Luks，另识别 LVM / swap / BIOS / APFS / HFS / UFS / ZFS / BitLocker |
| MBR/混合盘 | GPT open 后要求读取 MBR，可递归 EBR；非扩展 MBR 项发布被 `#ifdef TODO` 禁用 | 有效 GPT 优先；保护 MBR 不成为额外卷；原始混合槽可用 `--mbr` 检查，不合并重复卷 |
| 读游标 | partition read/seek，sector vector + cache；at-offset 会更新游标 | 源缓存由证据层负责；`read_at` 保留游标，Seek+Read 更新游标；分区 EOF 返回 0，越尾请求裁到尾 |
| 生命周期/路径 | C 手工管理、narrow/wide/BFIO、全局日志配置 | RAII、Path/PathBuf Unicode 路径、SectorSource 容器逻辑流与切片、现有 tracing 与应用关闭流程 |

不修补原始字节，不把截断长度改成看似完整的分区。
结构/CRC 验证只说明元数据校验通过，不保证条目范围、文件系统可浏览或证据内容真实性；
范围有效性单独记录，坏扇区来源审计仍由证据读者承担。
仅以 CRC 可验证候选自动改变几何；仅有损坏大扇区 GPT 时应使用显式几何分析 API，
另保留原有未知来源的 4096 字节整表兜底，不能保证所有损坏布局均自动恢复。
移位头和两份表都损坏的恢复能力不代表表符合 GPT 规范。

## 全部 42 个公开 C API

范围为 [include/libvsgpt.h.in](https://github.com/libyal/libvsgpt/blob/e4e439f93c3a0b776590dbf80f102f3b58032ca3/include/libvsgpt.h.in)
中的全部 `LIBVSGPT_EXTERN` 声明，含条件编译的 wide/BFIO 项。
“对应”表示同一只读取证操作，不承诺 C ABI、函数返回码或游标副作用兼容。

<details>
<summary>展开全部 42 个公开函数的对应关系</summary>

| libvsgpt API | LovelyCarve 对应及边界 |
| --- | --- |
| `get_version` | 宿主替代：Cargo/应用版本，没有加载 libvsgpt |
| `get_access_flags_read` | 对应：只读 SectorSource / GptPartitionReader |
| `get_access_flags_read_write` | 不适用：上游 open 拒绝 WRITE；本工具不写分区表 |
| `get_access_flags_write` | 同上，只是上游访问标志便利函数 |
| `get_codepage` | 不适用：GPT 名称固定 UTF-16LE；路径由操作系统处理 |
| `set_codepage` | 同上，不引入全局 codepage |
| `check_volume_signature` | 对应：读取候选头 + has_gpt_signature；完整打开用 DiskImage/GptTable |
| `check_volume_signature_wide` | 对应：Unicode Path/PathBuf，同一签名检查 |
| `check_volume_signature_file_io_handle` | 对应：SectorSource::read + has_gpt_signature |
| `notify_set_verbose` | 宿主替代：tracing/RUST_LOG |
| `notify_set_stream` | 宿主替代：应用 tracing subscriber |
| `notify_stream_open` | 宿主替代：应用日志 sink |
| `notify_stream_close` | 宿主替代：日志 sink 生命周期 |
| `error_free` | 宿主替代：Result/error RAII |
| `error_fprint` | 宿主替代：顶层错误和 tracing |
| `error_sprint` | 宿主替代：Display/to_string |
| `error_backtrace_fprint` | 宿主替代：anyhow 上下文链和顶层错误输出 |
| `error_backtrace_sprint` | 宿主替代：`format!("{error:#}")`；Rust backtrace 取决于运行配置 |
| `volume_initialize` | 对应：GptTable::read 创建结果 |
| `volume_free` | 对应：drop GptTable/DiskImage |
| `volume_signal_abort` | 对应：read_cancellable，直接检查调用方 token，无镜像取消线程 |
| `volume_open` | 对应：DiskImage::open + GptTable::read；统一证据读者提供源 bytes |
| `volume_open_wide` | 对应：Unicode Path/PathBuf，无重复 wide API |
| `volume_open_file_io_handle` | 对应：GptTable::read(&dyn SectorSource, sector_size) |
| `volume_close` | 对应：drop；运行时仍由既有 remove_image_runtime 清理 |
| `volume_get_bytes_per_sector` | 对应：GptTable::sector_size / DiskImage / ImageInfo |
| `volume_get_disk_identifier` | 对应：header().disk_guid；to_bytes_le 返回磁盘字节；disk_id 返回非 nil 大写字符串 |
| `volume_get_number_of_partitions` | 对应：partition_count，仅计 valid 条目 |
| `volume_get_partition_by_index` | 对应：partition_by_index，稠密有效列表，越界 None |
| `volume_has_partition_with_identifier` | 对应：partition_by_entry_index(...).is_some()，identifier 为原始 u32 槽位 |
| `volume_get_partition_by_identifier` | 对应：partition_by_entry_index，不存在/无效条目 None |
| `partition_free` | 对应：Rust drop；reader 借用源，不拥有/提前关闭证据 |
| `partition_get_entry_index` | 对应：GptEntry::entry_index，保留稀疏和 >=256 槽位 |
| `partition_get_identifier` | 对应：unique_guid / to_bytes_le，含 nil 原值 |
| `partition_get_type_identifier` | 对应：type_guid / to_bytes_le，未知类型仍完整保留 |
| `partition_get_type` | 语义边界：上游是遗留 MBR type 字节，GPT values 保持零；宿主使用原始 type_guid 与经签名细化的 PartitionKind，不假装两者等价 |
| `partition_get_volume_offset` | 对应：GptPartitionReader::volume_offset，checked LBA→bytes |
| `partition_read_buffer` | 对应：std::io::Read，EOF 0，提前短读错误 |
| `partition_read_buffer_at_offset` | 对应：read_at，有界；宿主游标不变，上游 seek-then-read 改游标 |
| `partition_seek_offset` | 对应：std::io::Seek，Start/Current/End；负数/溢出拒绝且位置不变，可 seek 超过 EOF |
| `partition_get_offset` | 对应：Seek::stream_position |
| `partition_get_size` | 对应：GptPartitionReader::size，含尾 LBA，checked 长度换算 |

</details>

## 上游工具、文档与未覆盖边界

[格式文档](https://github.com/libyal/libvsgpt/blob/e4e439f93c3a0b776590dbf80f102f3b58032ca3/documentation/GUID%20Partition%20Table%20%28GPT%29%20format.asciidoc)
列出的 EFI、Windows、Linux、BSD、Apple、Solaris、Ceph、VMware 等 GUID 是类型登记，
不意味着上游读取其中的文件系统或加密容器。宿主原始 GUID 读取涵盖所有 128 位取值；
文件系统 / 容器能力由各自读者决定：QNX4 / QNX6 与 CoreStorage / FileVault 2 已有独立读者，
GPFS、VMFS 和 Storage Spaces 仍未接入；GUID 类型识别本身不代表内容支持。
条目扩展区域仅参与 CRC，不解释厂商字段；混合 MBR 通过已有 MBR API 分析，不自动合并卷。

pyvsgpt 是同一 C 库的 Python 路径/文件对象/游标包装，没有额外格式解析能力；宿主以 Rust API、
统一镜像源和应用 CLI 替代，不提供 pyvsgpt 或 C ABI 兼容层。
vsgptinfo 的版本/帮助/verbose/镜像偏移和分区信息由应用版本、tracing、PartitionSlice 与 `--gpt` 探针对应；
不要求照搬它的输出排版或单独日志句柄。所有恢复和读取均不向检材写入。

主要实现依据：
[volume](https://github.com/libyal/libvsgpt/blob/e4e439f93c3a0b776590dbf80f102f3b58032ca3/libvsgpt/libvsgpt_volume.c)、
[header](https://github.com/libyal/libvsgpt/blob/e4e439f93c3a0b776590dbf80f102f3b58032ca3/libvsgpt/libvsgpt_partition_table_header.c)、
[entry](https://github.com/libyal/libvsgpt/blob/e4e439f93c3a0b776590dbf80f102f3b58032ca3/libvsgpt/libvsgpt_partition_entry.c)、
[partition values](https://github.com/libyal/libvsgpt/blob/e4e439f93c3a0b776590dbf80f102f3b58032ca3/libvsgpt/libvsgpt_partition_values.c)。

## 验证

```powershell
just test core::evidence::partitions::gpt
just test opens_raw_gpt_with_crc_validated_geometry_and_backup_recovery
just run-example probe_partitions <image> --gpt
```

回归样本逐字段独立编码，fixture CRC 使用独立的逐位 IEEE 实现并以 `123456789` 已知答案校验，
覆盖主 / 备头与数组 CRC、扩展头与条目、四种扇区大小、有效空表、稀疏高槽位、256 / 257 分区边界、
单扇区、完整范围、重复 GUID、重叠、UTF-16、过量 / 越界数组、I/O 错误与短读、取消和游标语义。
临时 RAW 文件经过完整的 `DiskImage::open` 路径，验证四种几何、保护 MBR、空表和仅备份存活的情况。
测试不依赖真实检材，也不修改进程环境；尚未与 libvsgpt 二进制在真实镜像上做差分比对。

已提交的 EWF 样本 `vendor/ewf/tests/data/gpt_130_partitions.E01` 实际包含 131 个非空条目，
`probe_partitions --gpt` 全部发布：512 字节几何、主备头与两份数组 CRC 均通过校验，选用 LBA 1，
没有恢复告警。文件名中的 130 不是条目数量的期望值。
