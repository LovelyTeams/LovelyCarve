# HFS+ / HFSX：libfshfs 完整对照

传统 HFS（Mac OS Standard）现由独立纯 Rust 读取器支持，范围及限制见 [CLASSIC_HFS.md](CLASSIC_HFS.md)。本文只描述 HFS+ / HFSX。

本次对照日期为 2026-10-13，固定 [libfshfs 78321de1eda54b30261adf59b938781128591b2e](https://github.com/libyal/libfshfs/tree/78321de1eda54b30261adf59b938781128591b2e)。逐项检查 README、`include/libfshfs.h.in`、catalog/B-tree、name、file entry、data stream、extended attribute 实现，公开头文件中的 **95 个不同函数全部列于下表**。对照不是引入 C 库：运行时继续使用 MIT 的 `hfsplus` 与共享 `cmpfs`；Apple 固定折叠表单独按 APSL-2.0 标记。

## 实际能力与本次修复

| 能力 | libfshfs 基线 | LovelyCarve 现在的实现 |
| --- | --- | --- |
| HFS+ / HFSX | Unicode 3.2 的 HFS+ / HFSX | 支持；HFSX 依据 Catalog B-tree 的 BC / CF 类型选择二进制或折叠比较 |
| 名称 | Unicode 路径 / 名称 | 固定 Apple 折叠表、忽略字符、Unicode 3.2 NFD / Hangul；保留 HFS 的不分解区间；CNID / thread 使用原始 UTF-16 查询 |
| 目录 / 标识 | 路径、CNID、父记录、子项 | `list_directory`、`walk`、`entry_by_id`；原始目录项 CNID 与硬链接 backing reference 分开 |
| 硬链接 / 符号链接 | backing inode / link target | 文件和目录间接链接按 private directory 解析，限深与查环；应用发布符号链接目标 |
| 数据 fork | 完整 / seek / offset / extent | 同一完整 allocation plan 覆盖 inline + overflow；完整、流式和范围读取一致 |
| resource fork | 独立 stream | `open_resource_fork` / `resource_extents`，使用 FF overflow key；不会误取 data fork 的 00 key |
| xattr | inline / fork；attr extents 尚未完整 | inline、fork 与 Attributes-tree 30 continuation；`xattr_metadata` / `open_xattr` / `get_xattr` |
| 压缩 | zlib 3/4、LZVN 7/8、RAW 9/10、LZFSE 11/12 | 支持这些类型及原始 type 1；移除应用层统一拒绝压缩的旧分支；压缩 open/seek 与完整/导出使用相同逻辑字节 |
| 时间 / 权限 | 创建、内容修改、entry 修改、访问、备份、added、BSD | 原始备份时间、带符号 POSIX added time、UID/GID、owner/admin flags、link count、device number、Finder bytes / catalog flags 保留 |
| 元数据树 | B-tree 与 fork 寻址 | 节点跨 extent 读取、metadata fork overflow、完整 offset table、节点号/逻辑长度、索引/叶链循环校验 |
| 来源完整性 | 格式读取与错误 | 分区乘加、来源长度、allocation blocks 与完整 fork 校验；缺失映射 / 短源不生成“成功的部分文件” |
| 备用头 | volume header | 主头无效时在精确卷片尾恢复 alternate header；`HeaderCopy` 保留选择来源 |

旧代码的 `open_file` 只用前八个 inline extent，`read_file_to` 会成功返回部分长度，resource fork overflow 还用了 data key。它们现在统一先验证完整映射；坏链在写出前报错，截断来源在实际读取时报错。B-tree node 也不能再假设首个物理地址后的所有字节都连续。

文件 / 目录的 HFS epoch 时间是 1904 起的 UTC 秒；**Volume Header 的 create_date 是本地时间例外**，保留原值，不擅自标成 UTC。Finder added time 是另一种 POSIX 秒证据。应用的通用 MACB 映射目前发布创建、内容修改、属性修改与访问时间；备份 / added 和完整 BSD/Finder 字段保留在读者 API，尚没有额外的界面列。

## 公开 API 逐项映射

“等价”指格式能力通过 Rust API 提供，不表示保留 C ABI。UTF-8 / UTF-16 字符串及 size 两套 C 函数通常由 Rust String / UTF-16 编码和长度代替；生命周期由所有权代替。资源 / 属性 API 已在读者提供，应用默认文件树尚未把它们展示为独立命名流节点。

| libfshfs 函数 | 状态 | Rust / 应用对应与边界 |
| --- | --- | --- |
| `libfshfs_get_version` | 等价 | Cargo 版本 / 只读 Read + Seek；无写入接口 |
| `libfshfs_get_access_flags_read` | 等价 | Cargo 版本 / 只读 Read + Seek；无写入接口 |
| `libfshfs_get_codepage` | 设计不同 | Unicode 固定规则；不提供可变进程级 codepage，传统 HFS 不在范围内 |
| `libfshfs_set_codepage` | 设计不同 | Unicode 固定规则；不提供可变进程级 codepage，传统 HFS 不在范围内 |
| `libfshfs_check_volume_signature` | 等价 | HFS signature probe；VolumeHeader / HfsVolume::open 进一步验证版本与几何 |
| `libfshfs_check_volume_signature_wide` | 等价 | HFS signature probe；VolumeHeader / HfsVolume::open 进一步验证版本与几何 |
| `libfshfs_check_volume_signature_file_io_handle` | 等价 | HFS signature probe；VolumeHeader / HfsVolume::open 进一步验证版本与几何 |
| `libfshfs_notify_set_verbose` | 设计不同 | 宿主 tracing / Result；不提供 C 的全局 FILE* 通知状态 |
| `libfshfs_notify_set_stream` | 设计不同 | 宿主 tracing / Result；不提供 C 的全局 FILE* 通知状态 |
| `libfshfs_notify_stream_open` | 设计不同 | 宿主 tracing / Result；不提供 C 的全局 FILE* 通知状态 |
| `libfshfs_notify_stream_close` | 设计不同 | 宿主 tracing / Result；不提供 C 的全局 FILE* 通知状态 |
| `libfshfs_error_free` | 等价 | HfsPlusError / std::io::Error、Display 与宿主上下文；RAII |
| `libfshfs_error_fprint` | 等价 | HfsPlusError / std::io::Error、Display 与宿主上下文；RAII |
| `libfshfs_error_sprint` | 等价 | HfsPlusError / std::io::Error、Display 与宿主上下文；RAII |
| `libfshfs_error_backtrace_fprint` | 等价 | HfsPlusError / std::io::Error、Display 与宿主上下文；RAII |
| `libfshfs_error_backtrace_sprint` | 等价 | HfsPlusError / std::io::Error、Display 与宿主上下文；RAII |
| `libfshfs_volume_initialize` | 等价 | 所有权与 Drop；没有需要手动释放的 C 对象 |
| `libfshfs_volume_free` | 等价 | 所有权与 Drop；没有需要手动释放的 C 对象 |
| `libfshfs_volume_signal_abort` | 部分 | 宿主任务/流式 visitor 取消；可包装 Read；读者没有单独的 C abort 标志 |
| `libfshfs_volume_open` | 等价 | HfsVolume::open<R: Read + Seek>；宿主 PartitionCursor / std 文件路径 |
| `libfshfs_volume_open_wide` | 等价 | HfsVolume::open<R: Read + Seek>；宿主 PartitionCursor / std 文件路径 |
| `libfshfs_volume_open_file_io_handle` | 等价 | HfsVolume::open<R: Read + Seek>；宿主 PartitionCursor / std 文件路径 |
| `libfshfs_volume_close` | 等价 | 所有权与 Drop；没有需要手动释放的 C 对象 |
| `libfshfs_volume_get_utf8_name_size` | 等价 | HfsVolume::volume_name；String / encode_utf16 / length |
| `libfshfs_volume_get_utf8_name` | 等价 | HfsVolume::volume_name；String / encode_utf16 / length |
| `libfshfs_volume_get_utf16_name_size` | 等价 | HfsVolume::volume_name；String / encode_utf16 / length |
| `libfshfs_volume_get_utf16_name` | 等价 | HfsVolume::volume_name；String / encode_utf16 / length |
| `libfshfs_volume_get_root_directory` | 等价 | stat / list_directory / resolve_path；根 CNID 2 |
| `libfshfs_volume_get_file_entry_by_identifier` | 等价 | HfsVolume::entry_by_id；原始 entry / parent / link identity |
| `libfshfs_volume_get_file_entry_by_utf8_path` | 等价 | stat / list_directory / resolve_path；根 CNID 2 |
| `libfshfs_volume_get_file_entry_by_utf16_path` | 等价 | stat / list_directory / resolve_path；根 CNID 2 |
| `libfshfs_file_entry_free` | 等价 | 所有权与 Drop；没有需要手动释放的 C 对象 |
| `libfshfs_file_entry_get_identifier` | 等价 | DirEntry.cnid / EntryMetadata.identifier；stat 解析 backing inode |
| `libfshfs_file_entry_get_parent_identifier` | 读者支持 | EntryMetadata.parent_identifier，再 entry_by_id 查询父记录 |
| `libfshfs_file_entry_get_parent_file_entry` | 读者支持 | EntryMetadata.parent_identifier，再 entry_by_id 查询父记录 |
| `libfshfs_file_entry_get_link_identifier` | 读者支持 | EntryMetadata.link_identifier；原始目录 entry.identifier 独立保留 |
| `libfshfs_file_entry_get_creation_time` | 读者支持 | FileStat.create_date；保留原始 epoch、角色与秒精度；通用 UI 的额外列边界见上文 |
| `libfshfs_file_entry_get_modification_time` | 读者支持 | FileStat.modify_date；保留原始 epoch、角色与秒精度；通用 UI 的额外列边界见上文 |
| `libfshfs_file_entry_get_entry_modification_time` | 读者支持 | FileStat.attribute_mod_date；保留原始 epoch、角色与秒精度；通用 UI 的额外列边界见上文 |
| `libfshfs_file_entry_get_access_time` | 读者支持 | FileStat.access_date；保留原始 epoch、角色与秒精度；通用 UI 的额外列边界见上文 |
| `libfshfs_file_entry_get_backup_time` | 读者支持 | FileStat.backup_date；保留原始 epoch、角色与秒精度；通用 UI 的额外列边界见上文 |
| `libfshfs_file_entry_get_added_time` | 读者支持 | FileStat.added_time；保留原始 epoch、角色与秒精度；通用 UI 的额外列边界见上文 |
| `libfshfs_file_entry_get_file_mode` | 读者支持 | FileStat.permissions.mode；BSD special union 原值另行保留 |
| `libfshfs_file_entry_get_number_of_links` | 读者支持 | FileStat.link_count；BSD special union 原值另行保留 |
| `libfshfs_file_entry_get_owner_identifier` | 读者支持 | FileStat.permissions.owner_id；BSD special union 原值另行保留 |
| `libfshfs_file_entry_get_group_identifier` | 读者支持 | FileStat.permissions.group_id；BSD special union 原值另行保留 |
| `libfshfs_file_entry_get_device_identifier` | 读者支持 | FileStat.device_number；BSD special union 原值另行保留 |
| `libfshfs_file_entry_get_device_number` | 读者支持 | FileStat.device_number；BSD special union 原值另行保留 |
| `libfshfs_file_entry_get_utf8_name_size` | 等价 | EntryMetadata.name / DirEntry.name；String / encode_utf16 / length |
| `libfshfs_file_entry_get_utf8_name` | 等价 | EntryMetadata.name / DirEntry.name；String / encode_utf16 / length |
| `libfshfs_file_entry_get_utf16_name_size` | 等价 | EntryMetadata.name / DirEntry.name；String / encode_utf16 / length |
| `libfshfs_file_entry_get_utf16_name` | 等价 | EntryMetadata.name / DirEntry.name；String / encode_utf16 / length |
| `libfshfs_file_entry_get_utf8_symbolic_link_target_size` | 等价 | FileStat.kind + 逻辑文件内容；应用 FsNode.link_target |
| `libfshfs_file_entry_get_utf8_symbolic_link_target` | 等价 | FileStat.kind + 逻辑文件内容；应用 FsNode.link_target |
| `libfshfs_file_entry_get_utf16_symbolic_link_target_size` | 等价 | FileStat.kind + 逻辑文件内容；应用 FsNode.link_target |
| `libfshfs_file_entry_get_utf16_symbolic_link_target` | 等价 | FileStat.kind + 逻辑文件内容；应用 FsNode.link_target |
| `libfshfs_file_entry_has_resource_fork` | 读者支持 | stat.resource_fork_size / open_resource_fork / resource_extents；界面没有独立 fork 节点 |
| `libfshfs_file_entry_get_resource_fork` | 读者支持 | stat.resource_fork_size / open_resource_fork / resource_extents；界面没有独立 fork 节点 |
| `libfshfs_file_entry_get_number_of_extended_attributes` | 读者支持 | list_xattrs + get_xattr / xattr_metadata；Vec 索引 / Option 查询 |
| `libfshfs_file_entry_get_extended_attribute_by_index` | 读者支持 | list_xattrs + get_xattr / xattr_metadata；Vec 索引 / Option 查询 |
| `libfshfs_file_entry_has_extended_attribute_by_utf8_name` | 读者支持 | list_xattrs + get_xattr / xattr_metadata；Vec 索引 / Option 查询 |
| `libfshfs_file_entry_has_extended_attribute_by_utf16_name` | 读者支持 | list_xattrs + get_xattr / xattr_metadata；Vec 索引 / Option 查询 |
| `libfshfs_file_entry_get_extended_attribute_by_utf8_name` | 读者支持 | list_xattrs + get_xattr / xattr_metadata；Vec 索引 / Option 查询 |
| `libfshfs_file_entry_get_extended_attribute_by_utf16_name` | 读者支持 | list_xattrs + get_xattr / xattr_metadata；Vec 索引 / Option 查询 |
| `libfshfs_file_entry_get_number_of_sub_file_entries` | 等价 | list_directory / Vec indexing / catalog path lookup |
| `libfshfs_file_entry_get_sub_file_entry_by_index` | 等价 | list_directory / Vec indexing / catalog path lookup |
| `libfshfs_file_entry_get_sub_file_entry_by_utf8_name` | 等价 | list_directory / Vec indexing / catalog path lookup |
| `libfshfs_file_entry_get_sub_file_entry_by_utf16_name` | 等价 | list_directory / Vec indexing / catalog path lookup |
| `libfshfs_file_entry_read_buffer` | 等价 | FileReader Read + Seek / stream_position；应用 Volume::read_range 已接通 |
| `libfshfs_file_entry_read_buffer_at_offset` | 等价 | FileReader Read + Seek / stream_position；应用 Volume::read_range 已接通 |
| `libfshfs_file_entry_seek_offset` | 等价 | FileReader Read + Seek / stream_position；应用 Volume::read_range 已接通 |
| `libfshfs_file_entry_get_offset` | 等价 | FileReader Read + Seek / stream_position；应用 Volume::read_range 已接通 |
| `libfshfs_file_entry_get_size` | 等价 | stat.size；压缩文件为解压后的逻辑长度 |
| `libfshfs_file_entry_get_number_of_extents` | 部分 | data_extents 返回原始 data fork；压缩文件不伪装成逻辑字节直接物理映射 |
| `libfshfs_file_entry_get_extent_by_index` | 部分 | data_extents 返回原始 data fork；压缩文件不伪装成逻辑字节直接物理映射 |
| `libfshfs_data_stream_free` | 等价 | 所有权与 Drop；没有需要手动释放的 C 对象 |
| `libfshfs_data_stream_read_buffer` | 等价 | ForkReader Read / Seek / stream_position；先 seek 再 read 可指定 offset |
| `libfshfs_data_stream_read_buffer_at_offset` | 等价 | ForkReader Read / Seek / stream_position；先 seek 再 read 可指定 offset |
| `libfshfs_data_stream_seek_offset` | 等价 | ForkReader Read / Seek / stream_position；先 seek 再 read 可指定 offset |
| `libfshfs_data_stream_get_offset` | 等价 | ForkReader Read / Seek / stream_position；先 seek 再 read 可指定 offset |
| `libfshfs_data_stream_get_size` | 等价 | ForkData.logical_size / FileStat.resource_fork_size |
| `libfshfs_data_stream_get_number_of_extents` | 读者支持 | resource_extents；data_extents；fork 物理 allocation map |
| `libfshfs_data_stream_get_extent_by_index` | 读者支持 | resource_extents；data_extents；fork 物理 allocation map |
| `libfshfs_extended_attribute_free` | 等价 | 所有权与 Drop；没有需要手动释放的 C 对象 |
| `libfshfs_extended_attribute_get_utf8_name_size` | 读者支持 | list_xattrs / XattrMetadata.name；String 与 UTF-16 |
| `libfshfs_extended_attribute_get_utf8_name` | 读者支持 | list_xattrs / XattrMetadata.name；String 与 UTF-16 |
| `libfshfs_extended_attribute_get_utf16_name_size` | 读者支持 | list_xattrs / XattrMetadata.name；String 与 UTF-16 |
| `libfshfs_extended_attribute_get_utf16_name` | 读者支持 | list_xattrs / XattrMetadata.name；String 与 UTF-16 |
| `libfshfs_extended_attribute_read_buffer` | 读者支持 | open_xattr 的 Cursor Read / Seek / stream_position；当前值读入内存后 seek |
| `libfshfs_extended_attribute_read_buffer_at_offset` | 读者支持 | open_xattr 的 Cursor Read / Seek / stream_position；当前值读入内存后 seek |
| `libfshfs_extended_attribute_seek_offset` | 读者支持 | open_xattr 的 Cursor Read / Seek / stream_position；当前值读入内存后 seek |
| `libfshfs_extended_attribute_get_offset` | 读者支持 | open_xattr 的 Cursor Read / Seek / stream_position；当前值读入内存后 seek |
| `libfshfs_extended_attribute_get_size` | 读者支持 | xattr_metadata.size；inline / fork 逻辑长度 |
| `libfshfs_extended_attribute_get_number_of_extents` | 读者支持 | xattr_metadata.extents；包括 Attributes-tree continuation |
| `libfshfs_extended_attribute_get_extent_by_index` | 读者支持 | xattr_metadata.extents；包括 Attributes-tree continuation |

## 限制与验证

- 传统 HFS 由[独立读取器](CLASSIC_HFS.md)支持。HFS-wrapper 的 embedded HFS+、Unicode 2.1 旧卷、journal replay / deleted record recovery 尚未实现；这些也不能从 libfshfs 的 README 名称“支持 HFS”推断为支持。
- LZBitmap 13/14 与 dataless 内容明确报错。压缩文件暂时整文件解码后 seek：解码输出及压缩 resource fork各限制 512 MiB，fork xattr 值限制 16 MiB；超过预算明确失败。zlib / LZFSE 解码输出按期望长度加一限界，不让畸形流无限膨胀。
- metadata forks 通过 overflow tree 重建；如果 Extents B-tree 自己的下一条记录位于尚未可访问的 fragment，明确失败，没有猜测或全盘扫描 bootstrap。
- FileVault 派生视图的 FsStat 不发布伪装成原盘密文地址的 HFS data runs；普通未加密 HFS 卷可以返回真实图像地址。FileVault 的逻辑卷 segment 映射另由 CoreStorage 读者保留。
- 本次验证了 41 个 HFS 单元/合成图测试、共享 cmpfs 28 测试，及公开 dfvfs FileVault 卷解密后的真实 HFS catalog / walk / full / stream / seek 一致性。没有运行原生 libfshfs DLL 的逐字段对拍，也没有完成 macOS / Linux 桌面或 Dokan 实挂验证，不能声称所有真实卷都已验证。

```powershell
just test-vendor hfsplus
just clippy-hfsplus
just test-vendor cmpfs
# 真实 CoreStorage/HFS 样本（不要提交检材）：
$env:FVDE_ORACLE_IMAGE = 'C:/path/to/fvdetest.cs.raw'
just test-filevault --test oracle_fvde
```

参考：[libfshfs 固定源码](https://github.com/libyal/libfshfs/tree/78321de1eda54b30261adf59b938781128591b2e)、[Apple TN1150](https://developer.apple.com/library/archive/technotes/tn/tn1150.html)、[Apple 固定 HFS 表](https://github.com/apple-oss-distributions/hfs/tree/d1bac2f062e6e9c0dfcce302d9aacb10173d0eea)、[FileVault 对照](FILEVAULT.md)。上游来源与各表的许可证见 `src-tauri/vendor/hfsplus/LOVELYCARVE.md`。
