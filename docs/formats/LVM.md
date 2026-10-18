# LVM2

LVM2 元数据与线性逻辑卷由纯 Rust 代码解析，文件系统读取与 RAID 重组共用同一套模型。本文以 libvslvm 提交 `dd725bdad6801048595a3ac7193b6a9d408c9966`（2026-10-13 核对）为参照，检查范围包括 [README](https://github.com/libyal/libvslvm/blob/dd725bdad6801048595a3ac7193b6a9d408c9966/README)、[全部公开 API](https://github.com/libyal/libvslvm/blob/dd725bdad6801048595a3ac7193b6a9d408c9966/include/libvslvm.h.in)、[格式说明](https://github.com/libyal/libvslvm/blob/dd725bdad6801048595a3ac7193b6a9d408c9966/documentation/Logical%20Volume%20Manager%20%28LVM%29%20format.asciidoc)、[LV 读取实现](https://github.com/libyal/libvslvm/blob/dd725bdad6801048595a3ac7193b6a9d408c9966/libvslvm/libvslvm_logical_volume.c) 以及 metadata / handle / physical-volume 实现。这里只对照读取能力，不提供 libvslvm 的 C ABI 兼容层，也不引入它的 DLL 或源码。

上游 README 仍将多 PV、多 segment、非线性 stripes、snapshots 列为不支持，但当前源码已经遍历 segments、按 PV 名称查找并使用 physical-volume IO pool，因此不能只按 README 推断其行为；多 stripe 分支仍明确拒绝。本文的结论来自源码与格式对照加本项目的回归测试，没有运行 libvslvm 二进制做真实镜像比对，不宣称所有多 PV 布局都已与上游逐字节一致。

## 能力矩阵

| 项目 | LovelyCarve 实现与边界 |
| --- | --- |
| 标签位置 | 检测与读取前四个 sector；验证标签 CRC、sector number、PV UUID、描述符范围 |
| 元数据完整性 | 校验 LVM weak CRC-32（seed `0xf597a6cf`，无 final XOR），MDA magic、version、地址与长度；零 CRC 也不豁免 |
| 活动副本 | 读取全部 label 指定的 MDA，包括远端备份；每个 MDA 仅采用已提交的 `raw_locns[0]`；有效副本取最高 seqno，同 seqno 冲突报错 |
| metadata ring 回绕 | 在 MDA 尾部拆分，并从 MDA+512 继续读取，拼接后校验 text CRC；保留两段原始来源 |
| ignored / precommit | ignored 不参与；未知 flags 拒绝；后续 raw locations 不冒充已提交副本 |
| PV 匹配 | 标签 compact UUID 匹配格式化 UUID；同大小 PV 不混淆，每个成员用自身 geometry |
| 文本语法 | 共享有界 grammar：注释、引号、转义、混合数组与块；重复字段/卷 UUID、无效 UUID、负数/溢出、缺失字段拒绝；引号中的关键词/括号不影响结构 |
| 空 VG | 无 logical_volumes 的有效 VG 可读取元数据与空 LV 列表 |
| VG/PV/LV/segment/stripe 元数据 | VG 名称/UUID/seqno/extent size，PV 名称/UUID/device/dev_size/pe_start/pe_count，LV 名称/UUID、全部 segments 与 stripes |
| LV 顺序与长度 | 保持文本 LV 声明顺序和 `lvm:N:M` 身份；segments 按 start_extent 排序，严格验证完整覆盖和 segment_count；长度累计全部段 |
| 单 PV 多段、碎片 LV | 完整线性 map；ext、XFS、Btrfs、NTFS 的元数据与正文均通过映射；完整、流式、范围读取共用字节来源 |
| 多 PV 线性 LV | RAID 检测按最新 VG 的 UUID 定义解析全部成员，PV 可以在不同检材、同一检材的不同分区或已组装的 md 阵列上；缺失、重复、冲突、越界时不构造残缺 LV。完整的跨 PV LV 组装为「逻辑卷 <卷组>/<卷>」合成检材（作为 md 成员的 LV 除外），成员检材补零的坏区按逻辑卷偏移记入该检材的坏区报告，见 [Linux md RAID](MDRAID.md) |
| 扇区单位 | 分区 LBA 按镜像 sector_size；所有 LVM metadata sector 值始终乘 512，覆盖 4Kn |
| LV 原始字节 | `lvm:N:M` 支持原始全量、范围、流式读取和 stat；未知文件系统的可读 LV 显示为文件，已识别文件系统继续作为目录 |
| 来源范围 | raw LV、ext 和 Btrfs stat 分拆回原镜像 offsets；NTFS 删除数据 runs 也跨 LV 段分拆；`absolute_offset` / fs_all offset 仅表示首物理段，完整来源以 map/runs 为准 |
| 未使用 PE | 扣除每个物理 segment；以 pe_count 为 PE 终点，按真实 extent size 过滤碎片；有未解析分配时抑制空闲空间投影 |
| 截断与损坏 | allocated short read 报错；checked 乘加与源/PV/PE/LV 边界检查；不把缺失成员补零 |
| 非线性 striped / mirror / RAID / snapshot / thin / cache | 保留 LV 名称、大小与 segment 元数据，明确拒绝正文读取；没有实现这些布局的语义 |
| LVM1、写入、修复磁盘元数据 | 未实现；证据只读 |

## 全部 69 个公开 API

下表逐项列出上游头文件的函数名。Rust 字符串和 Vec 自带长度，RAII 管理生命周期；这些属于行为对应，不是同名 C 函数。`支持` 表示本项目有相应读取数据或操作；`宿主对应` 表示由应用通用设施承担；`不适用` 表示无对应 C 运行时需求。

<details>
<summary>展开全部 69 个公开函数的对应关系</summary>

| libvslvm API | 状态 | LovelyCarve 对应 |
| --- | --- | --- |
| `get_version` | 不适用 | 本项目版本由 Cargo/应用提供，未加载 libvslvm |
| `get_access_flags_read_write` | 不支持写入 | 所有证据读者只读，无 read-write 模式 |
| `get_access_flags_write` | 不支持写入 | 不提供写入证据模式 |
| `get_codepage` | 不适用 | metadata 按 ASCII 解析；宿主路径使用 Rust Path |
| `set_codepage` | 不适用 | 不暴露 C narrow-string codepage 配置 |
| `check_volume_signature` | 支持 | Lvm2Parser + header::parse_label；支持 Path / SectorSource |
| `check_volume_signature_wide` | 支持 | Lvm2Parser + header::parse_label；支持 Path / SectorSource |
| `check_volume_signature_file_io_handle` | 支持 | Lvm2Parser + header::parse_label；支持 Path / SectorSource |
| `notify_set_verbose` | 宿主对应 | tracing 日志与 Metadata.diagnostics；不提供 C FILE/stream 配置 |
| `notify_set_stream` | 宿主对应 | tracing 日志与 Metadata.diagnostics；不提供 C FILE/stream 配置 |
| `notify_stream_open` | 宿主对应 | tracing 日志与 Metadata.diagnostics；不提供 C FILE/stream 配置 |
| `notify_stream_close` | 宿主对应 | tracing 日志与 Metadata.diagnostics；不提供 C FILE/stream 配置 |
| `error_free` | 宿主对应 | Rust 所有权 / RAII，无手动释放 C handle |
| `error_fprint` | 宿主对应 | anyhow 错误链 / 格式化上下文 / 宿主日志，无 C error 对象 |
| `error_sprint` | 宿主对应 | anyhow 错误链 / 格式化上下文 / 宿主日志，无 C error 对象 |
| `error_backtrace_fprint` | 宿主对应 | anyhow 错误链 / 格式化上下文 / 宿主日志，无 C error 对象 |
| `error_backtrace_sprint` | 宿主对应 | anyhow 错误链 / 格式化上下文 / 宿主日志，无 C error 对象 |
| `handle_initialize` | 宿主对应 | DiskImage / verified Metadata 构造 |
| `handle_free` | 宿主对应 | Rust RAII / Arc |
| `handle_signal_abort` | 宿主对应 | 宿主 TaskManager + CancellationToken；流式 visitor 传播取消；低层 metadata reader 没有独立 abort handle |
| `handle_open` | 支持 | DiskImage 打开后，header::read_metadata 读取 PV |
| `handle_open_wide` | 支持 | Rust Path 支持宿主 Unicode 文件名 |
| `handle_open_file_io_handle` | 支持 | SectorSource 支持 RAW、容器、物理盘与组装源 |
| `handle_open_physical_volume_files` | 支持，限线性 | raid::lvm_meta + members，按 UUID 绑定检材或已组装的 md 阵列，缺员不组装 |
| `handle_open_physical_volume_files_wide` | 支持，限线性 | 同上，使用 Rust Path，不依赖窄字节路径 |
| `handle_open_physical_volume_files_file_io_pool` | 支持，限线性 | 同上，Arc / SectorSource 代替 C IO pool |
| `handle_close` | 宿主对应 | EvidenceSession 卸载流程与 Arc 生命周期 |
| `handle_get_volume_group` | 支持 | Metadata.volume_group / LvmVolume.metadata |
| `volume_group_free` | 宿主对应 | Rust 所有权 / RAII，无手动释放 C handle |
| `volume_group_get_name_size` | 支持 | 对应 Rust String/Option<String>.len()，无需 caller buffer 或末尾 NUL |
| `volume_group_get_name` | 支持 | VolumeGroup.name |
| `volume_group_get_identifier_size` | 支持 | 对应 Rust String/Option<String>.len()，无需 caller buffer 或末尾 NUL |
| `volume_group_get_identifier` | 支持 | VolumeGroup.identifier |
| `volume_group_get_sequence_number` | 支持 | VolumeGroup.sequence_number |
| `volume_group_get_extent_size` | 支持 | VolumeGroup.extent_size_sectors × 512（checked） |
| `volume_group_get_number_of_physical_volumes` | 支持 | VolumeGroup.physical_volumes.len() |
| `volume_group_get_physical_volume` | 支持 | VolumeGroup.physical_volumes.get(index) |
| `volume_group_get_physical_volume_by_name` | 支持 | physical_volumes.iter().find(name)，读取成员身份另用 UUID |
| `volume_group_get_number_of_logical_volumes` | 支持 | VolumeGroup.logical_volumes.len() |
| `volume_group_get_logical_volume` | 支持 | VolumeGroup.logical_volumes.get(index) |
| `physical_volume_free` | 宿主对应 | Rust 所有权 / RAII，无手动释放 C handle |
| `physical_volume_get_name_size` | 支持 | 对应 Rust String/Option<String>.len()，无需 caller buffer 或末尾 NUL |
| `physical_volume_get_name` | 支持 | PhysicalVolume.name |
| `physical_volume_get_identifier_size` | 支持 | 对应 Rust String/Option<String>.len()，无需 caller buffer 或末尾 NUL |
| `physical_volume_get_identifier` | 支持 | PhysicalVolume.identifier；header.identifier 保留 compact UUID |
| `physical_volume_get_device_path_size` | 支持 | 对应 Rust String/Option<String>.len()，无需 caller buffer 或末尾 NUL |
| `physical_volume_get_device_path` | 支持 | PhysicalVolume.device（检材字符串，不打开该宿主路径） |
| `physical_volume_get_size` | 支持 | PhysicalVolume.size_sectors × 512；二进制 header.size_bytes |
| `logical_volume_free` | 宿主对应 | Rust 所有权 / RAII，无手动释放 C handle |
| `logical_volume_read_buffer` | 支持 | LogicalVolumeReader::read / 原始 LV 流式读取 |
| `logical_volume_read_buffer_at_offset` | 支持 | LinearLayout::read / LogicalVolumeReader::read_at / FsSession 范围读取 |
| `logical_volume_seek_offset` | 支持 | LogicalVolumeReader::seek（Start / Current / End） |
| `logical_volume_get_offset` | 支持 | LogicalVolumeReader::position |
| `logical_volume_get_size` | 支持 | LinearLayout.size / LvmLogicalVolume.size_bytes（全部段累计） |
| `logical_volume_get_name_size` | 支持 | 对应 Rust String/Option<String>.len()，无需 caller buffer 或末尾 NUL |
| `logical_volume_get_name` | 支持 | LogicalVolume.name / LvmLogicalVolume.name |
| `logical_volume_get_identifier_size` | 支持 | 对应 Rust String/Option<String>.len()，无需 caller buffer 或末尾 NUL |
| `logical_volume_get_identifier` | 支持 | LogicalVolume.identifier / LvmLogicalVolume.identifier |
| `logical_volume_get_number_of_segments` | 支持 | LogicalVolume.segments.len() |
| `logical_volume_get_segment` | 支持 | LogicalVolume.segments.get(index)，按 start_extent 排序 |
| `segment_free` | 宿主对应 | Rust 所有权 / RAII，无手动释放 C handle |
| `segment_get_offset` | 支持 | Segment.start_extent × extent_size_sectors × 512（checked） |
| `segment_get_size` | 支持 | Segment.extent_count × extent_size_sectors × 512（checked） |
| `segment_get_number_of_stripes` | 支持 | Segment.stripe_count / stripes.len()（striped 时校验一致） |
| `segment_get_stripe` | 支持 | Segment.stripes.get(index)，多 stripe 仅元数据，不支持正文 |
| `stripe_free` | 宿主对应 | Rust 所有权 / RAII，无手动释放 C handle |
| `stripe_get_physical_volume_name_size` | 支持 | 对应 Rust String/Option<String>.len()，无需 caller buffer 或末尾 NUL |
| `stripe_get_physical_volume_name` | 支持 | Stripe.physical_volume |
| `stripe_get_data_area_offset` | 支持 | Stripe.physical_extent × extent_size_sectors × 512；再加所引用 PV 的 pe_start 得原始位置 |

</details>

## 实现入口与验证

- `core/evidence/partitions/lvm2/header.rs`：二进制标签、弱 CRC、完整 MDA、活动文本和来源范围。`text.rs`：共享 grammar 与全部元数据模型。`layout.rs`：完整线性 byte map、范围拆分、独立 read/seek cursor。
- `core/filesystem/volumes/lvm.rs`：导航、原始 LV 字节、读取错误和空闲 PE 投影。ext/XFS/Btrfs/NTFS 原生读者复用这个 map；不会替换证据 ID、分区 ID 或原始分区几何。
- `core/evidence/raid/lvm_meta.rs`、`members.rs` 和 `lvm_stack.rs`：跨镜像与 md 阵列读取相同模型，选择最新定义、验证成员完整性，跨 PV 的 LV 组装为合成检材；`LinearLvSource` 不返回静默截断。
- `just run-example probe_lvm_active <image>`：输出经过校验的当前 VG/PV/LV/segment、文本来源与副本诊断。`probe_lvm_meta` 保留历史 ring 的调查用途，其扫描结果不用于激活卷。

合成测试覆盖四个标签 sector、CRC 已知向量（独立用 Python zlib seed/final-XOR 转换核对）、label/MDA/text 损坏、ignored/未知 flags、历史和 precommitted 高 seqno、wrapped active text、8 MiB 外备份及同 seqno 冲突、同大小 PV UUID、空 VG、注释/引号/转义、重复 key、段计数/覆盖/数值范围、缺失/重复成员、UUID 重命名与最新副本、4Kn 固定 sector 单位、跨段 full/range/stream/seek，以及 short read。

应用回归包含 raw LV 的读取 / stat / 流式读取、碎片 ext 卷的目录 / 内联 / 间接块 / 属性 / 符号链接及来源 runs、碎片 NTFS 正文与删除 runs、Btrfs tree-node 和 XFS cursor 跨段。Btrfs 与 XFS 这里验证的是映射读取路由，没有完整的真实文件系统镜像；`LOVELYCARVE_XFS_LVM_TEST` 是可选的外部 XFS 检材回归，未设置时跳过实际读取。测试夹具由本项目代码生成，ext 应用回归复用仓库中的 `audit_ext.bin`，没有下载或提交 libvslvm 样本。

资源上限：单个活动 metadata text 最多 16 MiB，grammar 深度最多 32、值节点最多 500,000；标签描述符只能位于 512 字节的 label 内，超限明确失败。CRC 损坏不会触发无校验的文本扫描，需要时可以用历史探针单独调查。

LVM 模型是核心 Rust API，没有单独的 IPC 命令；桌面与 CLI 的文件读取通过映射自动访问逻辑卷。修改后运行 `just test lvm` 与 `just check-examples`。
