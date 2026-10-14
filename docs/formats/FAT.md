# FAT / exFAT

FAT12/16/32 由 vendored `fatfs` 读取，exFAT 由 vendored `exfat` 读取，两者都带有本地取证补丁；实现独立编写，没有复制 libfsfat 的 LGPL C 代码。

本文以 [libfsfat `98782bf5eb33a804f118e27b77382aeff7f02b1d`](https://github.com/libyal/libfsfat/tree/98782bf5eb33a804f118e27b77382aeff7f02b1d)（2026-10-12 核对）为参照，审阅了公开头文件的 **53 个函数**以及 boot record、allocation table、directory、directory entry、file system、file entry、name 的实现、格式文档与工具入口。上游标记为 experimental，包含 FAT12/16/32 和 exFAT，打开时拒绝写访问。

参照源码：[公开 API](https://github.com/libyal/libfsfat/blob/98782bf5eb33a804f118e27b77382aeff7f02b1d/include/libfsfat.h.in)、[卷打开](https://github.com/libyal/libfsfat/blob/98782bf5eb33a804f118e27b77382aeff7f02b1d/libfsfat/libfsfat_volume.c)、[目录与簇链](https://github.com/libyal/libfsfat/blob/98782bf5eb33a804f118e27b77382aeff7f02b1d/libfsfat/libfsfat_file_system.c)、[名称与时间](https://github.com/libyal/libfsfat/blob/98782bf5eb33a804f118e27b77382aeff7f02b1d/libfsfat/libfsfat_directory_entry.c)、[格式文档](https://github.com/libyal/libfsfat/blob/98782bf5eb33a804f118e27b77382aeff7f02b1d/documentation/File%20Allocation%20Table%20%28FAT%29%20format.asciidoc)。

## 格式能力

| 能力 | libfsfat | LovelyCarve |
| --- | --- | --- |
| FAT12/16/32 | BPB、packed 12 / 16 / 32 位 FAT | 三种读者；按数据簇数判定类型，忽略可能错误的 `FATxx` 提示字符串 |
| 固定 / 簇链根目录 | FAT12/16 固定区域，FAT32 簇链 | 支持；FAT32 最后一个 EOC 簇照常读取 |
| 非标准逻辑扇区 | 接受 512–32768 字节的 2 的幂 | 识别、活文件与恢复统一接受该范围；独立测试覆盖 8192 字节 |
| exFAT | 独立的 boot / entry-set 分支 | 独立读者，支持范围读取、初始化尾部、时间精度与 extent |
| 来源与卷偏移 | 普通 / wide 文件名、libbfio | DiskImage / 分区切片，复用所有镜像格式、物理盘和解密字节来源 |
| 备份引导 | 不作为公开恢复 API | FAT32 常规 sector 6 或 BPB 指定位置、exFAT sector 12，供活文件与恢复共用；只覆盖读到的 BPB，不写回证据 |
| 活动 FAT | 公开 API 无 table selector | 活文件与恢复都遵循 FAT32 禁用镜像时的 table selector 及 exFAT ActiveFat |
| 短名、LFN、UTF-16 | 支持 | FAT 读取与 stat 共用长名、短名别名与大小写规则；exFAT 正确处理跨 FileName 槽位的代理对 |
| OEM 代码页 | 全局 codepage 用于窄字符串；名称转换仍有 `TODO add codepage support` | 默认 FAT 转换对非 ASCII 短名有损；LFN 与 exFAT 使用 UTF-16 |
| exFAT 名称比较 | name / directory-entry 比较 | 精确 Unicode 名及 ASCII 不区分大小写；尚未按磁盘自定义 Up-case Table 完成全部 Unicode 折叠 |
| 原始属性 | read-only / hidden / system / volume / directory / archive 等 | `file_metadata` 提供原始 FAT u8 / exFAT u16 flags，不混入 POSIX mode |
| 目录项标识 | 卷内短名目录项偏移，可反向打开 | inode / stat / metadata 保留**绝对镜像**目录项偏移；应用 ID 按路径寻址，不提供裸偏移反向打开 |
| FAT 时间 | 日期、时间与 10ms fraction | 保留无时区的本地时间；活项与删除项保留创建时间精度，非法 fraction 不发布 |
| exFAT 时间 | packed time、fraction、offset 字段 | 带符号 7 位 offset、OffsetValid 未设置与 10ms fraction；活项与删除项共用转换 |
| 完整 / 流式 / 范围读取 | read buffer、at-offset、seek | 两种适配器都实现 `Volume::read_range`；EOF 返回空结果、跨 EOF 裁剪；预览与 Dokan 读取复用 |
| 物理 extent | 数量与按索引取 extent | stat 与 data_runs 来自读者的实际分配图；合并相邻物理簇，按逻辑长度裁剪，不含簇尾 slack |
| NoFatChain | 连续 stream extent | 活文件支持；连续 exFAT 子目录同样扫描删除项 |
| ValidDataLength / DataLength | 分别解析 stream 字段 | 大小取 DataLength，VDL 之后读零；完整、流式、范围读取与采样一致 |
| 损坏与短读 | 错误、边界与重复簇检查 | 检测循环、非法 / 坏簇、FAT 容量、分区外 extent 与短读；不把前缀当作完整文件 |
| 删除项 | FAT 0xe5 标为 unallocated，普通枚举跳过；无公开 undelete API | 删除槽位 / entry-set 扫描；同名候选按父目录和槽位区分 ID，只有完整的单 run 可读 |
| 只读 | 拒绝写访问 | FAT bookkeeping 写入由 cursor 吸收，exFAT 只暴露读接口；测试比对访问前后镜像的所有字节 |

## 53 个公开 API 的映射

以下省略 `libfsfat_` 前缀；大括号表示多个函数，例如 `volume_get_{utf8,utf16}_label{,_size}` 是四个函数。“对应”指应用提供相应的工作流，不代表提供 C ABI。

| 函数组 | 数量 | 对应或差异 |
| --- | ---: | --- |
| get_version、get_access_flags_read | 2 | 应用 / crate 版本、只读访问契约 |
| get_codepage、set_codepage | 2 | 没有全局窄字符串 codepage 状态；路径使用 Rust Unicode；OEM 限制见上表 |
| check_volume_signature{,_wide,_file_io_handle} | 3 | 分区解析、BPB 验证、证据字节来源 |
| notify_set_verbose、notify_set_stream、notify_stream_open、notify_stream_close | 4 | tracing / RUST_LOG，没有 FILE* 日志 API |
| error_free、error_fprint、error_sprint、error_backtrace_fprint、error_backtrace_sprint | 5 | anyhow::Result、错误上下文与传播、RAII |
| volume_initialize、volume_free、volume_close | 3 | Volume / Arc / EvidenceSession 卸载生命周期 |
| volume_signal_abort | 1 | 长任务使用 TaskManager / CancellationToken 和流式 visitor；同步的 FAT 加载与目录打开没有独立的 abort 句柄 |
| volume_open{,_wide,_file_io_handle} | 3 | Unicode 原生路径、DiskImage 与分区字节接口 |
| volume_get_file_system_format | 1 | PartitionKind、FAT `volume_metadata.format`、exFAT 适配器身份 |
| volume_get_{utf8,utf16}_label{,_size} | 4 | `volume_metadata.label`；Rust String 可转 UTF-8 / UTF-16，无需查询 C 缓冲区长度 |
| volume_get_root_directory | 1 | 分区节点、root_children、空路径 children |
| volume_get_file_entry_by_identifier | 1 | 不支持按裸目录项偏移反向打开；路径 ID 可打开，偏移作为来源元数据保留 |
| volume_get_file_entry_by_{utf8,utf16}_path | 2 | Unicode 字符串路径；UTF-16 在系统边界转换 |
| file_entry_free、file_entry_get_identifier | 2 | RAII、inode / `metadata.entry_offset`；注意绝对镜像偏移与上游卷内偏移的区别 |
| file_entry_get_{access,creation,modification}_time | 3 | Mactb accessed / created / modified，保留精度与时区依据 |
| file_entry_get_file_attribute_flags | 1 | `file_metadata.attributes`，后端 Rust 接口 |
| file_entry_get_{utf8,utf16}_name{,_size} | 4 | FsNode.name、metadata.name / FAT short_name、标准 Unicode 转换 |
| file_entry_get_number_of_sub_file_entries、file_entry_get_sub_file_entry_by_index | 2 | children 的长度与索引，上层 filedb 分页 |
| file_entry_get_sub_file_entry_by_{utf8,utf16}_name | 2 | 名称 / 路径查找；exFAT Unicode 折叠限制见上表 |
| file_entry_read_buffer、file_entry_read_buffer_at_offset | 2 | read_file、stream_file、read_file_range |
| file_entry_seek_offset、file_entry_get_offset | 2 | 无状态的显式 offset 请求；vendor File 支持 Seek，跨 IPC 不共享游标 |
| file_entry_get_size | 1 | FsNode / FsStat.size；exFAT 使用 DataLength |
| file_entry_get_number_of_extents、file_entry_get_extent_by_index | 2 | FsStat.data_runs / data_runs / live_data_runs 的长度与索引 |
| **合计** | **53** | |

fsfatinfo 的卷信息与文件层级对应镜像信息页、文件树和 CLI；fsfatmount 对应 Dokan 只读挂载。Python 绑定属于另一种语言封装，不进入桌面取证运行时。

## 实现与边界

`volumes/fat/read.rs` 维护经过校验的 extent 计划，供完整、流式、范围读取和 stat 使用；`volumes/exfat/read.rs` 使用 vendor 的实际分配图。`vendor/fatfs` 补充了目录项偏移、短名匹配、有限 extent 迭代与目录重复簇检查；`vendor/exfat` 处理逻辑大小、初始化尾部、严格簇链、时间与 UTF-16，并提供原生 extent 和属性。`fat_deleted` / `exfat_recover` 统一了活动 FAT、时间、同名记录身份与恢复目录读取。

范围读取跳过不需要的**正文**，但仍需路径解析和簇链元数据遍历，任意偏移跳转不是 O(1)。exFAT 打开正文时会重新加载 FAT；目录快照遍历复用 Root。

FAT 本地时间和 OffsetValid 未设置的 exFAT 时间输出为 `YYYY-MM-DDTHH:mm:ss.SSS`，不附加虚构的 `Z`；只有已知 offset 的 exFAT 时间才归一化为 UTC RFC 3339。时间线只投影带时区的时间，不猜测检材所在时区。

这些 metadata 是后端 Rust 接口，`inode` 字段返回**绝对镜像**目录项偏移（分区基址 + 卷内目录项偏移），删除项的 `inode` 同样是其目录槽位的绝对镜像偏移；因此按「元数据地址」排序即按物理槽序排序，反映的是槽位在目录流中的物理位置，不是写入或时间顺序。原始属性与卷序列号没有对应的 Tauri 命令，也不在详情面板中展示。

尚未覆盖：按任意裸目录项偏移反向打开、完整的 OEM 多字节代码页、按自定义 exFAT Up-case Table 的 Unicode 比较，以及 Boot Region / entry-set / upcase 全部 checksum 的严格验证链。删除的连续 run 和 FAT32 高字截断重建仍是候选推断，不保证簇未被重用；碎片化的删除正文、缺失父链及删除目录的递归恢复没有完整支持。LFN 序号丢失后的删除项名称是尽力恢复。

## 验证

```powershell
just test fat
just test-vendor fatfs
just test-vendor exfat
```

合成样本逐字段构造，不由待测库格式化，不含检材数据：

| 测试 | 覆盖 |
| --- | --- |
| `volumes/fat/tests.rs` | 三种 FAT、碎片范围与 EOF、错误链、活动 FAT32、大逻辑扇区、属性与偏移、10ms 时间、同名删除项、证据字节不变 |
| `volumes/exfat/fixture_tests.rs` | 碎片 / 连续、VDL 0 / 700 / 1100、跨初始化边界、尾部零、跨槽位代理对、UTC-5、属性与偏移、坏链、备份 BPB、连续子目录删除项 |
| `fat_deleted` / `exfat_recover` 测试 | 删除短名 / LFN / entry-set、目录 slack、最后 EOC 簇、备份选择、高字截断候选 |
| vendor/exfat 时间测试 | 未知 / 正 / 负 offset、10ms 上限与非法值 |

验证依据是固定源码审计、独立构造的镜像和本地测试；尚未与当前 libfsfat 二进制做逐文件差分，也未在 Windows 上验证真实挂载，不宣称格式完全兼容。fatfs / exfat 原有许可证与来源保留在各自的 vendor 目录。
