# APFS：读取能力与 libfsapfs 对照

对照日期：2026-10-13。基准为 [libyal/libfsapfs `cc122619a0fa5cecf7c077742ac62251fb85fb6c`](https://github.com/libyal/libfsapfs/tree/cc122619a0fa5cecf7c077742ac62251fb85fb6c)，逐项检查 README、`include/libfsapfs.h.in` 的 **110 个公开函数**、对应实现、工具与测试；结构依据 [Apple File System Reference（2020-06-22）](https://developer.apple.com/support/downloads/Apple-File-System-Reference.pdf)。

LovelyCarve 继续使用 MIT 的 vendored `apfs` 0.4.1 与共享 `cmpfs`，不加载 libfsapfs DLL，也没有复制 libfsapfs 的 C 实现。格式能力、Rust API 和桌面入口分别列出；API 对照不等于完整真实镜像 oracle 验证，更不代表 FileVault 已经可以解锁。

## 格式与工具能力

| 项目 | libfsapfs 基准 | LovelyCarve 当前行为 |
| --- | --- | --- |
| APFS v2，只读 | 支持 | 支持；原始证据只读 |
| APFS v1、Fusion、T2 | README 不支持 | v1 / Fusion 明确拒绝；T2 / 硬件密钥不支持 |
| 容器身份、尺寸、features | 公开 API | `container_info`；UUID、XID、block size/count、三类 feature masks、原始非零卷槽位 |
| 多卷 | 按卷枚举/选择 | `list_volumes` / `open_index`；保留空槽、卷 UUID、group UUID、role、XID、flags、allocated size |
| 卷尺寸 | `volume_get_size` 实现仍为 TODO，返回失败 | 单独暴露 allocated size，不能误称每卷独立逻辑容量；APFS 各卷共享容器 |
| 加密 | 密码、恢复密码、keybag 与 AES-XTS 路径 | **未实现解锁**；可枚举未加密 APSB 中的加密标志，打开时明确报错；加密 OMAP 不伪装为普通数据 |
| 检查点 | 最新事务及对象映射 | NXSB 检查 Fletcher、块尺寸、完整源几何；候选必须同 UUID/几何/版本特性；B-tree descriptor area 不支持 |
| OMAP 多版本与删除 | 按事务查找 | 取 `(OID, XID)` 的前驱，XID 不超过选中容器/卷事务；tombstone 不回退旧块；批量缓存和随机查找一致 |
| B-tree | 物理/虚拟树 | 检查表、key/value 边界、节点类型/层级；层级递减限制递归与循环；OMAP 前驱限制 64 层 |
| 普通目录、路径和 OID | 支持 | 两种目录键布局；路径 `stat/open_file`，OID `stat_by_id/open_file_by_id` |
| 大小写与 Unicode | case folding；不应仅按 README 推断 Unicode 版本 | 按卷 case / normalization bits 做 NFD 和 Unicode full case folding；保留原名，折叠后多重匹配报错；casefold 数据为 Unicode 9，NFD 数据来自 unicode-normalization 0.1.25，后续 Unicode 版本差异仍需真实镜像核验 |
| inode 元数据 | MACB、owner/group、mode、links、device | Rust API 保留带符号纳秒时间、parent/private ID、UID/GID、mode、links、BSD flags、RDEV；目录加入时间保留在 `DirEntry.date_added`；桌面共用模型只投影现有 MACB / mode / inode |
| 硬链接与克隆 | 目录名、inode 和流身份分离 | 保留各路径；数据按 inode `private_id` 读共享 extent；不推测缺失父链，不支持删除版本恢复 |
| 稀疏文件 | 支持 | interior gap 与显式 `phys_block_num=0` extent 读零；没有显式尾部 extent 的缺失尾巴报错，避免把损坏当真实零字节 |
| 完整/流式/范围读取 | file / attribute read、seek | 文件三种路径共用校验后的 extent plan；数据流 xattr 可独立 seek；源短读、重叠、逻辑/物理算术溢出、越过容器均报错 |
| extent 枚举 | 文件和属性公开 extent API | `file_extents` / `xattr_info.extents`；桌面普通文件 `FsStat.data_runs` 映射回原检材偏移；压缩/符号链接不给虚构的逻辑原始 runs |
| xattr 与 resource fork | 枚举、名称查找、读、seek | `list_xattrs/get_xattr/open_xattr/xattr_info`；embedded 和 data-stream 都支持；独立属性浏览/导出 UI 尚未接入 |
| 符号链接 | target API | xattr（含 data-stream）及数据 fork 回退；列表和目录索引保存目标；不跟随链接 |
| decmpfs 3/4、7/8、9/10、11/12 | zlib、LZVN、raw、LZFSE | 共享 `cmpfs` 已实现；列表、stat、预览、导出、范围读和目录索引使用解压逻辑长度 |
| decmpfs 1 | README 不支持 | 支持原样 xattr payload |
| decmpfs 13/14、dataless | README 不支持 LZBitmap | 明确拒绝；dataless 不作为空文件导出 |
| 原生快照 | README 标为不支持；源码可枚举元数据、打开 snapshot APSB，公开 snapshot API 只有名称/释放，未提供历史文件浏览 | `list_snapshots` 保留名称、XID、superblock block、create/change、flags 与 extent-reference 信息；**历史 catalog 浏览未实现**，不能用当前卷冒充历史版本 |
| sealed / encryption rolling / incomplete restore | 未承诺完整支持 | 明确拒绝相应 incompatible bits；不以普通 catalog 解释 fext/sealed 树 |
| 容器识别 | signature / fsapfsinfo | 独立 APFS 分区镜像、GPT/APM/MBR refinement 进入原生读者；NXSB 整块校验和与源几何验证 |
| 取证工具/挂载 | fsapfsinfo / fsapfsmount、Python/C API | 复用 LovelyCarve 浏览、目录索引、哈希、导出与 CLI；Dokan 缓存未命中可使用直接范围读；此次未做真实 Dokan 挂载验证 |
| 取消、资源与生命周期 | abort、C error/free | Rust `Result` / Drop；宿主任务取消和快照 PartialOutput；库目录 visitor 可停止；压缩文件仍整文件解码到内存，取消不保证能打断单次解码 |

## 本次修正与新增

- 原实现 OMAP 的直接查找按 OID 返回第一项，未遵守事务上限；现在随机查找与批量 cache 使用相同提交视图，删除记录不复活旧对象。
- 原实现先校验固定 4096 字节，导致更大 APFS block 无法打开；改为先读取受限几何，再校验完整块。分区游标与容器游标分别限制来源，后面的分区/容器外数据不能用于补足 extent。
- 原完整读取可返回短长度，流式读者却把缺失尾巴变成零；统一后明确拒绝缺失尾巴，并正确处理显式空洞。损坏 extent 不再被静默删除，重叠 extent 不再悄悄选一份。
- 原目录解析固定按 hashed key 解释，补 plain key、严格 UTF-8/终止符与卷内名称查找规则。embedded xattr 的声明长度不再被截短掩盖。
- 共享 `cmpfs` 补 LZFSE stored marker（11/12），验证 RAW resource block（10）的 0xcc 标记与 offset table / payload 边界；HFS+ 同样使用该修正。
- 原 APFS 直接范围读取未接入 dispatcher，文件来源 runs 为空，符号链接目标为空；现有预览、哈希、导出及随机读流程使用补齐后的接口。
- 原 `.dirz` 快路径只保存 inode 长度，压缩文件和 symlink 可能与 live listing 不同；现在补逻辑 metadata，并保持取消时删除部分输出。

## 公开 API 的逐项对应

下表包括头文件中条件编译的 wide / BFIO 入口。`等价` 指 Rust 的数据/操作能力，不表示提供同名 C ABI；`部分` 指元数据或底层能力存在但语义/宿主入口不同；`未实现` 是实际差距；`不同接口` 是内存、编码或通知机制的架构选择。

| libfsapfs 公开函数 | 状态 | 本地对应 / 边界 |
| --- | --- | --- |
| `libfsapfs_get_version` | 不同接口 | Cargo package / `UPSTREAM.md`，不伪造 libfsapfs 版本 |
| `libfsapfs_get_access_flags_read` | 等价 | 只读 `Read + Seek` 证据来源 |
| `libfsapfs_get_codepage` | 不同接口 | APFS 名称严格 UTF-8，无全局代码页 |
| `libfsapfs_set_codepage` | 不同接口 | 同上；无代码页转换开关 |
| `libfsapfs_check_container_signature` | 等价 | `NxSuperblock::parse` / `container_info` / 宿主 NXSB probe；Rust 来源代替 filename/BFIO 变体 |
| `libfsapfs_check_volume_signature` | 等价 | `ApfsSuperblock::parse` / `object::read_object`；Rust 来源代替 filename/BFIO 变体 |
| `libfsapfs_check_container_signature_wide` | 等价 | `NxSuperblock::parse` / `container_info` / 宿主 NXSB probe；Rust 来源代替 filename/BFIO 变体 |
| `libfsapfs_check_volume_signature_wide` | 等价 | `ApfsSuperblock::parse` / `object::read_object`；Rust 来源代替 filename/BFIO 变体 |
| `libfsapfs_check_container_signature_file_io_handle` | 等价 | `NxSuperblock::parse` / `container_info` / 宿主 NXSB probe；Rust 来源代替 filename/BFIO 变体 |
| `libfsapfs_check_volume_signature_file_io_handle` | 等价 | `ApfsSuperblock::parse` / `object::read_object`；Rust 来源代替 filename/BFIO 变体 |
| `libfsapfs_notify_set_verbose` | 不同接口 | 宿主 tracing / 任务事件；不提供 C 全局通知流 |
| `libfsapfs_notify_set_stream` | 不同接口 | 宿主 tracing / 任务事件；不提供 C 全局通知流 |
| `libfsapfs_notify_stream_open` | 不同接口 | 宿主 tracing / 任务事件；不提供 C 全局通知流 |
| `libfsapfs_notify_stream_close` | 不同接口 | 宿主 tracing / 任务事件；不提供 C 全局通知流 |
| `libfsapfs_error_free` | 不同接口 | `ApfsError` / `Result` / error source / Drop；无 C error buffer 或同格式 backtrace |
| `libfsapfs_error_fprint` | 不同接口 | `ApfsError` / `Result` / error source / Drop；无 C error buffer 或同格式 backtrace |
| `libfsapfs_error_sprint` | 不同接口 | `ApfsError` / `Result` / error source / Drop；无 C error buffer 或同格式 backtrace |
| `libfsapfs_error_backtrace_fprint` | 不同接口 | `ApfsError` / `Result` / error source / Drop；无 C error buffer 或同格式 backtrace |
| `libfsapfs_error_backtrace_sprint` | 不同接口 | `ApfsError` / `Result` / error source / Drop；无 C error buffer 或同格式 backtrace |
| `libfsapfs_container_initialize` | 不同接口 | Rust 所有权 / Drop / reader 借用生命周期，无显式 free |
| `libfsapfs_container_free` | 不同接口 | Rust 所有权 / Drop / reader 借用生命周期，无显式 free |
| `libfsapfs_container_signal_abort` | 部分 | catalog visitor 与宿主任务取消；单次解压/OMAP preload 不能即时打断 |
| `libfsapfs_container_open` | 等价 | `ApfsVolume::open`，generic `Read + Seek`（路径由宿主打开） |
| `libfsapfs_container_open_wide` | 等价 | `ApfsVolume::open`，generic `Read + Seek`（路径由宿主打开） |
| `libfsapfs_container_open_file_io_handle` | 等价 | `ApfsVolume::open`，generic `Read + Seek`（路径由宿主打开） |
| `libfsapfs_container_close` | 不同接口 | Rust 所有权 / Drop / reader 借用生命周期，无显式 free |
| `libfsapfs_container_get_size` | 等价 | `ContainerInfo.size`，checked geometry |
| `libfsapfs_container_get_identifier` | 等价 | `ContainerInfo.uuid` 原始 16 bytes |
| `libfsapfs_container_is_locked` | 部分 | 逐卷 `VolumeInfo.encrypted`；container keybag locked 状态未解析 |
| `libfsapfs_container_get_number_of_volumes` | 等价 | `list_volumes().len()` / 原始非零槽 |
| `libfsapfs_container_get_volume_by_index` | 等价 | `open_index` 使用原始槽位 index |
| `libfsapfs_volume_free` | 不同接口 | Rust 所有权 / Drop / reader 借用生命周期，无显式 free |
| `libfsapfs_volume_unlock` | 未实现 | 没有 FileVault/keybag 解锁链 |
| `libfsapfs_volume_get_features_flags` | 等价 | `VolumeInfo` 三类 feature masks |
| `libfsapfs_volume_get_size` | 不同接口 | 上游 TODO 返回失败；本地 `allocated_size` 不能称逻辑容量 |
| `libfsapfs_volume_get_identifier` | 等价 | `VolumeInfo.uuid` |
| `libfsapfs_volume_get_utf8_name_size` | 等价 | `VolumeInfo.name`，UTF-8 String；UTF-16 用标准 encode_utf16，无额外 buffer-size ABI |
| `libfsapfs_volume_get_utf8_name` | 等价 | `VolumeInfo.name`，UTF-8 String；UTF-16 用标准 encode_utf16，无额外 buffer-size ABI |
| `libfsapfs_volume_get_utf16_name_size` | 等价 | `VolumeInfo.name`，UTF-8 String；UTF-16 用标准 encode_utf16，无额外 buffer-size ABI |
| `libfsapfs_volume_get_utf16_name` | 等价 | `VolumeInfo.name`，UTF-8 String；UTF-16 用标准 encode_utf16，无额外 buffer-size ABI |
| `libfsapfs_volume_is_locked` | 部分 | `encrypted` 标志；加密卷打开明确拒绝 |
| `libfsapfs_volume_set_utf8_password` | 未实现 | 密码/恢复密码编码入口依赖尚未实现的 FileVault 解锁 |
| `libfsapfs_volume_set_utf16_password` | 未实现 | 密码/恢复密码编码入口依赖尚未实现的 FileVault 解锁 |
| `libfsapfs_volume_set_utf8_recovery_password` | 未实现 | 密码/恢复密码编码入口依赖尚未实现的 FileVault 解锁 |
| `libfsapfs_volume_set_utf16_recovery_password` | 未实现 | 密码/恢复密码编码入口依赖尚未实现的 FileVault 解锁 |
| `libfsapfs_volume_get_root_directory` | 等价 | `list_directory("/")` / `stat_by_id(2)` |
| `libfsapfs_volume_get_next_file_entry_identifier` | 等价 | `VolumeInfo.next_object_id`，不是文件数量 |
| `libfsapfs_volume_get_file_entry_by_identifier` | 等价 | `stat_by_id` / `open_file_by_id` |
| `libfsapfs_volume_get_file_entry_by_utf8_path` | 等价 | `stat(path)` / `open_file(path)`；UTF-16 输入显式转 UTF-8 |
| `libfsapfs_volume_get_file_entry_by_utf16_path` | 等价 | `stat(path)` / `open_file(path)`；UTF-16 输入显式转 UTF-8 |
| `libfsapfs_volume_get_number_of_snapshots` | 等价 | `VolumeInfo.num_snapshots` / `list_snapshots` |
| `libfsapfs_volume_get_snapshot_by_index` | 部分 | `list_snapshots` 元数据；未打开历史 catalog/文件 |
| `libfsapfs_snapshot_free` | 不同接口 | Rust 所有权 / Drop / reader 借用生命周期，无显式 free |
| `libfsapfs_snapshot_get_utf8_name_size` | 等价 | `SnapshotInfo.name`；UTF-16 用标准编码转换 |
| `libfsapfs_snapshot_get_utf8_name` | 等价 | `SnapshotInfo.name`；UTF-16 用标准编码转换 |
| `libfsapfs_snapshot_get_utf16_name_size` | 等价 | `SnapshotInfo.name`；UTF-16 用标准编码转换 |
| `libfsapfs_snapshot_get_utf16_name` | 等价 | `SnapshotInfo.name`；UTF-16 用标准编码转换 |
| `libfsapfs_file_entry_free` | 不同接口 | Rust 所有权 / Drop / reader 借用生命周期，无显式 free |
| `libfsapfs_file_entry_get_identifier` | 等价 | `FileStat.oid` / `DirEntry.oid` |
| `libfsapfs_file_entry_get_parent_identifier` | 等价 | `FileStat.parent_oid`；inode 父记录不替代多路径身份 |
| `libfsapfs_file_entry_get_parent_file_entry` | 等价 | parent OID → `stat_by_id`；保留硬链接路径限制 |
| `libfsapfs_file_entry_get_creation_time` | 等价 | `create_time: i64` 纳秒；宿主 MACB |
| `libfsapfs_file_entry_get_modification_time` | 等价 | `modify_time: i64` 纳秒 |
| `libfsapfs_file_entry_get_access_time` | 等价 | `access_time: i64` 纳秒 |
| `libfsapfs_file_entry_get_inode_change_time` | 等价 | `change_time: i64` 纳秒 |
| `libfsapfs_file_entry_get_added_time` | 部分 | `DirEntry.date_added`；按目录名关系，不投影为 birth |
| `libfsapfs_file_entry_get_file_mode` | 等价 | `FileStat.mode` 原始 POSIX 类型/权限 |
| `libfsapfs_file_entry_get_number_of_links` | 等价 | `FileStat.nlink`；目录 nchildren 与 links 有不同语义 |
| `libfsapfs_file_entry_get_owner_identifier` | 等价 | `FileStat.uid` |
| `libfsapfs_file_entry_get_group_identifier` | 等价 | `FileStat.gid` |
| `libfsapfs_file_entry_get_device_identifier` | 等价 | RDEV 原始值 `FileStat.device_number` |
| `libfsapfs_file_entry_get_device_number` | 等价 | `FileStat.device_numbers`，仅字符/块设备 |
| `libfsapfs_file_entry_get_utf8_name_size` | 等价 | `DirEntry.name`；inode/OID 本身没有唯一名称 |
| `libfsapfs_file_entry_get_utf8_name` | 等价 | `DirEntry.name`；inode/OID 本身没有唯一名称 |
| `libfsapfs_file_entry_get_utf16_name_size` | 等价 | `DirEntry.name`；inode/OID 本身没有唯一名称 |
| `libfsapfs_file_entry_get_utf16_name` | 等价 | `DirEntry.name`；inode/OID 本身没有唯一名称 |
| `libfsapfs_file_entry_get_utf8_symbolic_link_target_size` | 等价 | `DirEntry.link_target` / `open_file` 链接字节；宿主列表与索引已接入 |
| `libfsapfs_file_entry_get_utf8_symbolic_link_target` | 等价 | `DirEntry.link_target` / `open_file` 链接字节；宿主列表与索引已接入 |
| `libfsapfs_file_entry_get_utf16_symbolic_link_target_size` | 等价 | `DirEntry.link_target` / `open_file` 链接字节；宿主列表与索引已接入 |
| `libfsapfs_file_entry_get_utf16_symbolic_link_target` | 等价 | `DirEntry.link_target` / `open_file` 链接字节；宿主列表与索引已接入 |
| `libfsapfs_file_entry_get_number_of_extended_attributes` | 等价 | `list_xattrs().len()`；库 API，未独立 UI |
| `libfsapfs_file_entry_get_extended_attribute_by_index` | 等价 | `list_xattrs` + `open_xattr` / `xattr_info` |
| `libfsapfs_file_entry_has_extended_attribute_by_utf8_name` | 等价 | `get_xattr(...).is_some()` 或 `list_xattrs` 原名查找 |
| `libfsapfs_file_entry_has_extended_attribute_by_utf16_name` | 等价 | `get_xattr(...).is_some()` 或 `list_xattrs` 原名查找 |
| `libfsapfs_file_entry_get_extended_attribute_by_utf8_name` | 等价 | `get_xattr/open_xattr/xattr_info`；UTF-16 名称显式转换 |
| `libfsapfs_file_entry_get_extended_attribute_by_utf16_name` | 等价 | `get_xattr/open_xattr/xattr_info`；UTF-16 名称显式转换 |
| `libfsapfs_file_entry_get_number_of_sub_file_entries` | 等价 | `list_directory().len()` |
| `libfsapfs_file_entry_get_sub_file_entry_by_index` | 等价 | `list_directory` 的原名/OID |
| `libfsapfs_file_entry_get_sub_file_entry_by_utf8_name` | 等价 | `list_directory` / 路径查找；卷内名称规则，不依赖 host 大小写 |
| `libfsapfs_file_entry_get_sub_file_entry_by_utf16_name` | 等价 | `list_directory` / 路径查找；卷内名称规则，不依赖 host 大小写 |
| `libfsapfs_file_entry_read_buffer` | 等价 | `open_file` / `Read`，统一 extent plan |
| `libfsapfs_file_entry_read_buffer_at_offset` | 等价 | `read_file_range`，已接 dispatcher |
| `libfsapfs_file_entry_seek_offset` | 等价 | `open_file` 的 `Seek` |
| `libfsapfs_file_entry_get_offset` | 等价 | `Seek::stream_position` |
| `libfsapfs_file_entry_get_size` | 等价 | `stat.size` 为解压/链接逻辑长度 |
| `libfsapfs_file_entry_get_number_of_extents` | 部分 | `file_extents`，压缩/链接不映射为普通逻辑 runs |
| `libfsapfs_file_entry_get_extent_by_index` | 部分 | `file_extents` / 宿主 `FsStat.data_runs`；同上 |
| `libfsapfs_extended_attribute_free` | 不同接口 | Rust 所有权 / Drop / reader 借用生命周期，无显式 free |
| `libfsapfs_extended_attribute_get_identifier` | 部分 | `XattrInfo.stream_id`；resident 属性以文件 OID + 原名标识 |
| `libfsapfs_extended_attribute_get_utf8_name_size` | 等价 | `XattrEntry.name` / `XattrInfo.name`，标准 UTF-8/UTF-16 转换 |
| `libfsapfs_extended_attribute_get_utf8_name` | 等价 | `XattrEntry.name` / `XattrInfo.name`，标准 UTF-8/UTF-16 转换 |
| `libfsapfs_extended_attribute_get_utf16_name_size` | 等价 | `XattrEntry.name` / `XattrInfo.name`，标准 UTF-8/UTF-16 转换 |
| `libfsapfs_extended_attribute_get_utf16_name` | 等价 | `XattrEntry.name` / `XattrInfo.name`，标准 UTF-8/UTF-16 转换 |
| `libfsapfs_extended_attribute_read_buffer` | 等价 | `open_xattr` 的 `Read`；resident / stream |
| `libfsapfs_extended_attribute_read_buffer_at_offset` | 等价 | `open_xattr` 的 `Seek` + `Read` |
| `libfsapfs_extended_attribute_seek_offset` | 等价 | `open_xattr` 的 `Seek` |
| `libfsapfs_extended_attribute_get_offset` | 等价 | `Seek::stream_position` |
| `libfsapfs_extended_attribute_get_size` | 等价 | `XattrInfo.size`；不按文件的压缩头改变属性本身 |
| `libfsapfs_extended_attribute_get_number_of_extents` | 等价 | `XattrInfo.extents.len()`；resident 为零 |
| `libfsapfs_extended_attribute_get_extent_by_index` | 等价 | `XattrInfo.extents` 原始逻辑/物理/crypto 字段 |

## 验证与样本

```text
just test-apfs
just clippy-apfs
just test-vendor cmpfs
just test-vendor hfsplus
just test --lib apfs
just fmt
just check
just clippy
just test --lib
just check-examples
```

本次已运行：`fmt`、`check`、宿主及 APFS `clippy`、`check-examples`；APFS 37 通过 / 1 ignored，cmpfs 27 通过，HFS+ 29 通过，后端全量库测试 1023 通过 / 12 ignored。默认并行的宿主测试编译曾出现 rustc `STATUS_ACCESS_VIOLATION`；通过设置 `CARGO_BUILD_JOBS=1` 并运行 `just test --lib` 成功完成。没有运行桌面/CLI 二进制 smoke 或真实挂载。

固定合成结构在 [vendor/apfs/tests/data](../../vendor/apfs/tests/data/README.md)，由独立 Python 字段构造器生成，包含两个目录键布局、非连续卷槽、加密卷标志、多版本 OMAP、克隆、空洞、压缩、symlink、embedded/data-stream xattr 与快照元数据。它们没有完整 allocator/extent-reference 树，不是可由 macOS 挂载的完整磁盘，因此只能证明所覆盖结构和读取路径。

库回归核对完整/流式/范围读、past-EOF 和大偏移、事务/删除与 cache 一致、8 KiB superblock、跨来源边界、损坏表/extent/xfield。宿主回归核对 MBR 分区的 APFS 类型 refinement、原检材 runs、dispatcher range、live / `.dirz` 压缩/symlink 元数据一致及取消清理。共享 `cmpfs` 的编码测试独立执行。

真实样本可用已有 `LOVELYCARVE_APFS_SNAPSHOT_TEST` / `LOVELYCARVE_APFS_DECMPFS_TEST` 环境入口；未设置时跳过，不能算作真实镜像验证。上游旧 `test_omap_lookup` 因缺少 `../tests/appfs.raw` 默认 ignored。没有构建 libfsapfs 做真实镜像字节 oracle，本次不声称完整实现等价、密码解锁、历史快照浏览或跨平台挂载已验证。

## 剩余差距

FileVault 的 container/volume keybag、PBKDF、KEK/VEK 解包、AES-XTS、crypto-state 与解锁会话/IPC/UI 尚未实现；这是相对 libfsapfs 的主要功能差距。快照元数据已补，历史浏览需要 snapshot APSB、同 XID 的 OMAP/catalog、历史卷身份和宿主缓存隔离。独立 xattr UI、完整硬链接 sibling 信息、sealed/fext、checkpoint descriptor B-tree、损坏卷恢复以及大压缩文件的分块解码/取消也未完成。

这些条目不能通过加一个 capability 标志或接入未验证的 DLL 视为完成；后续需各自独立实现及外部样本核验。
