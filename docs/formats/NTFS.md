# NTFS

NTFS 由 vendored `ntfs` crate 加本地取证适配层读取，不加载 libfsntfs DLL。本文以
[libfsntfs](https://github.com/libyal/libfsntfs/tree/faaa031d0f779578559d5c025c9583d103702bcc)
固定提交 `faaa031d0f779578559d5c025c9583d103702bcc`（2026-10-12 核对）的 README、
[公开头文件](https://github.com/libyal/libfsntfs/blob/faaa031d0f779578559d5c025c9583d103702bcc/include/libfsntfs.h.in)、
[NTFS 格式说明](https://github.com/libyal/libfsntfs/blob/faaa031d0f779578559d5c025c9583d103702bcc/documentation/New%20Technologies%20File%20System%20%28NTFS%29.asciidoc)
及 WOF、文件条目、安全描述符实现为参照。

下表区分格式读取能力、Rust 核心 API 和桌面入口。源码对照与固定样本回归不等于已用 libfsntfs
对所有检材做过完整差分；未覆盖的场景见[验证边界](#验证边界)。

## 格式与核心能力

| 能力 | libfsntfs | LovelyCarve 当前实现与入口 | 验证 |
| --- | --- | --- | --- |
| NTFS 3.0 / 3.1、BPB、扇区/簇/记录尺寸、卷号 | 支持只读 | `Ntfs` + `NtfsVolume::volume_metadata` | vendor NTFS 真实格式样本、主工程合成卷 |
| 卷名、版本、卷状态 | 支持 | `volume_metadata` 保留原始 flags | 卷 3 驻留属性回归 |
| MFT 按记录读取、序列号、基记录引用、LSN、分配状态 | 支持 | 缓存 MFT run map；`record_metadata` | 跨 MFT 扩展记录、普通卷回归 |
| `$MFT` 自身的 `$ATTRIBUTE_LIST` | 支持 | 从已可映射前缀读取扩展，避免递归重入 | 合成分裂 MFT；断链/不连续明确失败 |
| 文件属性链与 ADS | 支持 | 按类型、原始 UTF-16 名称和连续 VCN 连接；instance 仅定位记录内属性 | 不同 instance、填充名称偏移、相邻不同 ADS |
| 驻留 / 非驻留 / 碎片 / 稀疏 / 未初始化尾部 | 支持 | `DataStream`，零 LCN 差值仍为分配簇；短读失败 | 稀疏、碎片、尾部、短读、损坏头和 runlist |
| 原始属性、类型/flags/name/size、映射段 | 支持 | `read_attribute_value`、`record_metadata.attributes.extents`；未知类型保留数值 | 驻留值、稀疏与已分配映射；压缩映射表示存储字节 |
| LZNT1 | 支持 | 完整/流式/范围/ADS/采样统一解码；压缩单元解满后忽略簇对齐填充 | Windows `RtlCompressBuffer` 生成的固定向量、末尾单字节填充回归 |
| WOF XPRESS-Huffman 4K / 8K / 16K | 支持 | `WofCompressedData` + WOF 文件 provider；同一逻辑读取入口 | Windows 原生 codec 向量、压缩/存储混合块 |
| WOF LZX 32K | 支持 | WIM 变种 LZX，含最后一个不足 32K 的块 | wimlib C oracle 向量与跨块读取 |
| WOF offset table / 64 位 offsets | 支持 | N−1 个相对偏移、末端隐式边界；按需读取两项，缓存一个块 | 32 位表覆盖；64 位分支尚无 >4 GiB 固定样本 |
| 默认流和 ADS 随机读取 | 支持 | `read_range` / `read_data_stream_range`；`ntfs-ads:` 接入 `FsSession` 直接范围读取 | ADS 跨属性边界、LZNT1、WOF 跨块 |
| 目录小索引/大索引、按路径查找 | 支持 | `$I30`；`find_record_by_path` 精确名称匹配，不自动跨重解析点 | vendor 的大目录索引；循环索引回归 |
| 大小写敏感名称 / 同目录多个硬链接 | 支持 | 用 (record, raw name) / (parent, raw name) 保留全部名称边；DOS 别名仅从普通列表隐藏 | 同父目录 A.txt / a.txt、DOS 别名 |
| 所有 `$FILE_NAME` 名称、命名空间、父引用、FN 时间/尺寸/flags | 支持 | `record_metadata.filenames`，保留 DOS/POSIX/Win32/组合 namespace | FN 原始时间与 UTF-16 字节 |
| SI 时间、owner/security ID、USN、quota | 支持 | `record_metadata`，SI/FN 分开，100 ns FILETIME 整数不丢失 | SI/FN 不同时间、原始 FILETIME |
| Object ID、GUID、EA、bitmap 等属性 | 属性读取及部分结构化 getter | vendor 结构化 reader；主接口可枚举并读取原始属性值 | 没有专用的界面解析面板 |
| 符号链接 / junction / reparse | 支持名称/标签/压缩方法 | bounded parser、substitute/print name、flags/raw data；目录和扫描投影为 Symlink | UTF-16 目标、越界拒绝；不跟随宿主路径 |
| 旧安全描述符属性 | 支持 | `security_descriptor` | 合成 self-relative SID |
| 现代安全描述符 `$Secure:$SII` / `$SDS` | 支持 | B-tree 查找 security ID；核对位置/长度与两个头；返回原始 SID/ACL 描述符 | 小索引查询、调用预算、缺失/越界失败；大索引复用 vendor 路径 |
| 删除 / 未分配记录 | 支持分配状态及 MFT 枚举 | 删除快照、全 MFT 边扫描、原始属性/内容读取 | 原有主工程删除/扫描回归；残留内容不保证可恢复 |
| 独立导出的 `$MFT` 元数据文件 | 支持 metadata-only | `NtfsMftMetadataFile`：按槽读取、fixup、卷 3 标签/版本、SI/FN、原始驻留属性 | 从合成卷导出的 MFT；非驻留内容明确要求原卷 |
| USN journal | 支持读取 journal record | 稀疏前缀跳过，逻辑 reader + V2/V3 解码；V4 不伪造日期 | 原有分块、页填充、FILETIME 精度回归 |
| BitLocker / VSS 检测 | 支持签名检测 | 由证据/加密/VSS 域处理；已解锁卷复用相同 NTFS reader | 现有 BitLocker / VSS 测试 |
| NTFS 1.0 / 1.1 / 1.2 | README 明确不支持 | 不声称旧版本完整兼容 | 无承诺 |
| EFS 解密 | README 明确不支持 | 明确拒绝逻辑内容读取，仍可检查记录元数据 | 不实现用户私钥导入 |
| NTFS 写入、journal replay / 修复 | 只读库范围外 | 不修改检材 | 范围外 |

## 接口与探针

`NtfsVolume` 的 metadata / security API 供 Rust 调用和只读 JSON 探针使用。
压缩读取、硬链接、重解析投影和 ADS 范围读取已经接入桌面与 CLI 的文件操作；
细粒度元数据、安全描述符和独立 MFT 目前没有桌面专用面板。

```powershell
just run-example probe_ntfs_metadata '<镜像路径>' <分区ID> <MFT记录号>
just run-example probe_ntfs_metadata --mft '<导出的 $MFT>' <MFT记录号>
just run-example probe_ntfs_stream '<镜像路径>' <分区ID> <MFT记录号> '<独立对照文件>'
```

元数据探针也接受卷内路径代替 MFT 记录号。流读取探针对超过 16 MiB 的文件使用临时文件
比对完整流、范围读取和前 16 MiB 的有界读取，避免把整个大文件放入内存；可选对照文件按长度和 SHA-256 核对。

`read_attribute_value(record, instance, budget)` 读取物理属性内的值。
非驻留压缩值仍是编码后的存储，不能当作已解码正文；文件正文必须走
`read_file` / `stream_file` / `read_range`。扩展属性头可按其物理 record/instance 查询。
映射中的 offset 是卷内偏移；BitLocker 解密卷也采用该坐标，不能误当原镜像物理字节。

## 验证边界

- 严格 reader 对损坏属性/USA/BAAD 返回具体错误；未提供 libfsntfs 损坏条目的部分恢复对象。

- 固定二进制只含合成字节，来源、输入生成规则与 SHA-256 见
  [NTFS fixtures](../../src-tauri/src/core/filesystem/volumes/ntfs/testdata/README.md)。
- WOF 回归在合成 NTFS 卷中使用独立原生 codec 输出；尚未完成大批真实 Compact/WOF
  文件的全量 libfsntfs 差分，不声称在所有 Windows 版本及损坏检材上完全一致。
- WIMBoot provider 1 需要外部 WIM 资源，当前明确报错；未知 provider/算法、缺 ADS
  和不合法 offset table 也报错，不把稀疏默认流的零数据当作正文。
- `ms-compress` 的 LZX 跟随 wimlib 的编码位零填充行为；WOF 本身没有内容校验和，
  成功解码不能证明数据未损坏，取证完整性仍由原镜像及导出哈希确认。
- 独立 MFT 无 BPB/cluster bytes；只报告实际存在的记录/驻留值，不猜簇大小或解密内容。
  扩展记录独立寻址，不把缺失的 volume 数据拼成空文件。
- 核心返回完整的安全描述符原始字节，并检查 SID/ACL 边界；未实现宿主权限应用、
  SDDL 美化或专用权限 GUI。
- WOF 解码使用 `ms-compress`，只启用 std / xpress / lzx 特性，发行 profile 对其使用
  `opt-level=3`；来源与许可见第三方声明。
- vendor 的 `--no-default-features` 测试依赖 std 夹具，不作为 no_std 读取的验证结果。

## 测试

```powershell
just test ntfs
just test-vendor ntfs
```

vendor 测试不会随主工程的 `just test` 执行，修改 `src-tauri/vendor/ntfs` 时需要单独运行。
公共读取接口改动另跑 `just check-examples`。

## 公开 C API 逐项索引

下表覆盖固定头文件的全部函数，映射的是读取能力；Rust 不复制 C 指针管理、
UTF-8/UTF-16 缓冲区长度查询、通知流及 bfio handle 绑定。生命周期由 RAII，
错误由 Result/anyhow，输出由 tracing/JSON，取消由调用者 token 管理。

<details>
<summary>展开全部 197 个公开函数的对应关系</summary>

| libfsntfs 函数 | 数量 | 本地对应能力/入口 |
| --- | ---: | --- |
| `get_version`、`get_access_flags_read`、`get_codepage`、`set_codepage`、`check_volume_signature`、`check_volume_signature_wide`、`check_volume_signature_file_io_handle` | 7 | Rust Result/tracing/编译版本；卷探针；UTF-16 名称（不暴露 C ABI） |
| `check_mft_metadata_file_signature`、`check_mft_metadata_file_signature_wide`、`check_mft_metadata_file_signature_file_io_handle` | 3 | 公共辅助/生命周期：Rust 所有权、标准 IO、Result（不复制 C ABI） |
| `notify_set_verbose`、`notify_set_stream`、`notify_stream_open`、`notify_stream_close`、`error_free`、`error_fprint`、`error_sprint`、`error_backtrace_fprint`、`error_backtrace_sprint` | 9 | Rust Result/tracing/编译版本；卷探针；UTF-16 名称（不暴露 C ABI） |
| `volume_initialize`、`volume_free`、`volume_signal_abort`、`volume_open`、`volume_open_wide`、`volume_open_file_io_handle`、`volume_close` | 7 | `NtfsVolume::open*` / Rust RAII / 上层 CancellationToken |
| `volume_has_bitlocker_drive_encryption`、`volume_has_volume_shadow_snapshots`、`volume_get_bytes_per_sector`、`volume_get_cluster_block_size`、`volume_get_mft_entry_size`、`volume_get_index_entry_size`、`volume_get_utf8_name_size`、`volume_get_utf8_name`、`volume_get_utf16_name_size`、`volume_get_utf16_name`、`volume_get_version`、`volume_get_flags`、`volume_get_serial_number` | 13 | `volume_metadata` / 证据层 BitLocker、VSS 检测 |
| `volume_get_root_directory` | 1 | 按记录 / `find_record_by_path` / `children(5)` |
| `volume_get_number_of_file_entries` | 1 | `volume_metadata` / 证据层 BitLocker、VSS 检测 |
| `volume_get_file_entry_by_index`、`volume_get_file_entry_by_utf8_path`、`volume_get_file_entry_by_utf16_path` | 3 | 按记录 / `find_record_by_path` / `children(5)` |
| `volume_get_usn_change_journal` | 1 | `visit_usn_journal` |
| `file_entry_free` | 1 | `record_metadata` / `list_data_streams` / 原始属性值；RAII 管理条目 |
| `file_entry_is_empty` | 1 | `list_data_streams` 的逻辑长度 / 缺失流类型错误 |
| `file_entry_is_allocated` | 1 | `record_metadata.allocated` |
| `file_entry_is_corrupted` | 1 | 严格结构校验返回具体 `NtfsError`；不提供损坏条目的部分恢复对象 |
| `file_entry_get_file_reference`、`file_entry_get_base_record_file_reference`、`file_entry_get_parent_file_reference`、`file_entry_get_parent_file_reference_by_attribute_index`、`file_entry_get_journal_sequence_number`、`file_entry_get_creation_time`、`file_entry_get_modification_time`、`file_entry_get_access_time`、`file_entry_get_entry_modification_time`、`file_entry_get_file_attribute_flags`、`file_entry_get_i30_file_reference`、`file_entry_get_i30_creation_time`、`file_entry_get_i30_modification_time`、`file_entry_get_i30_access_time`、`file_entry_get_i30_entry_modification_time`、`file_entry_get_i30_size`、`file_entry_get_i30_file_attribute_flags`、`file_entry_get_utf8_name_size`、`file_entry_get_utf8_name`、`file_entry_get_utf16_name_size`、`file_entry_get_utf16_name`、`file_entry_get_name_attribute_index`、`file_entry_get_utf8_name_size_by_attribute_index`、`file_entry_get_utf8_name_by_attribute_index`、`file_entry_get_utf16_name_size_by_attribute_index`、`file_entry_get_utf16_name_by_attribute_index`、`file_entry_get_utf8_path_hint_size`、`file_entry_get_utf8_path_hint`、`file_entry_get_utf16_path_hint_size`、`file_entry_get_utf16_path_hint`、`file_entry_get_utf8_symbolic_link_target_size`、`file_entry_get_utf8_symbolic_link_target`、`file_entry_get_utf16_symbolic_link_target_size`、`file_entry_get_utf16_symbolic_link_target` | 34 | `record_metadata` / `list_data_streams` / 原始属性值；RAII 管理条目 |
| `file_entry_get_security_descriptor_size`、`file_entry_get_security_descriptor` | 2 | `security_descriptor` |
| `file_entry_get_number_of_attributes`、`file_entry_get_attribute_by_index`、`file_entry_has_directory_entries_index` | 3 | `record_metadata` / `list_data_streams` / 原始属性值；RAII 管理条目 |
| `file_entry_has_default_data_stream` | 1 | `list_data_streams` / `read_data_stream*` |
| `file_entry_has_i30_entry`、`file_entry_is_symbolic_link` | 2 | `record_metadata` / `list_data_streams` / 原始属性值；RAII 管理条目 |
| `file_entry_get_number_of_alternate_data_streams`、`file_entry_get_alternate_data_stream_by_index`、`file_entry_has_alternate_data_stream_by_utf8_name`、`file_entry_has_alternate_data_stream_by_utf16_name`、`file_entry_get_alternate_data_stream_by_utf8_name`、`file_entry_get_alternate_data_stream_by_utf16_name` | 6 | `list_data_streams` / `read_data_stream*` |
| `file_entry_get_number_of_sub_file_entries`、`file_entry_get_sub_file_entry_by_index`、`file_entry_get_sub_file_entry_by_utf8_name`、`file_entry_get_sub_file_entry_by_utf16_name` | 4 | `children` / 精确名称 `find_record_by_path` |
| `file_entry_read_buffer`、`file_entry_read_buffer_at_offset`、`file_entry_seek_offset`、`file_entry_get_offset` | 4 | 逻辑 full / stream / range reader |
| `file_entry_get_size` | 1 | `record_metadata` / `list_data_streams` / 原始属性值；RAII 管理条目 |
| `file_entry_get_number_of_extents`、`file_entry_get_extent_by_index` | 2 | `record_metadata.attributes.extents`（编码流为存储映射） |
| `attribute_free`、`attribute_get_type`、`attribute_get_data_flags`、`attribute_get_utf8_name_size`、`attribute_get_utf8_name`、`attribute_get_utf16_name_size`、`attribute_get_utf16_name`、`attribute_get_data_vcn_range`、`attribute_get_data_size`、`attribute_get_valid_data_size`、`attribute_list_attribute_get_number_of_entries`、`attribute_list_attribute_get_entry_by_index`、`attribute_list_entry_free`、`attribute_list_entry_get_attribute_type`、`attribute_list_entry_get_file_reference`、`attribute_list_entry_get_utf8_name_size`、`attribute_list_entry_get_utf8_name`、`attribute_list_entry_get_utf16_name_size`、`attribute_list_entry_get_utf16_name` | 19 | `record_metadata.attributes` / `read_attribute_value` |
| `file_name_attribute_get_parent_file_reference`、`file_name_attribute_get_creation_time`、`file_name_attribute_get_modification_time`、`file_name_attribute_get_access_time`、`file_name_attribute_get_entry_modification_time`、`file_name_attribute_get_file_attribute_flags`、`file_name_attribute_get_name_space`、`file_name_attribute_get_utf8_name_size`、`file_name_attribute_get_utf8_name`、`file_name_attribute_get_utf16_name_size`、`file_name_attribute_get_utf16_name` | 11 | `record_metadata.filenames` / 原始 `$FILE_NAME` |
| `object_identifier_attribute_get_droid_file_identifier`、`object_identifier_attribute_get_birth_droid_volume_identifier`、`object_identifier_attribute_get_birth_droid_file_identifier`、`object_identifier_attribute_get_birth_droid_domain_identifier` | 4 | vendor `NtfsObjectId` / 原始 `$OBJECT_ID` |
| `reparse_point_attribute_get_tag`、`reparse_point_attribute_get_compression_method`、`reparse_point_attribute_get_utf8_substitute_name_size`、`reparse_point_attribute_get_utf8_substitute_name`、`reparse_point_attribute_get_utf16_substitute_name_size`、`reparse_point_attribute_get_utf16_substitute_name`、`reparse_point_attribute_get_utf8_print_name_size`、`reparse_point_attribute_get_utf8_print_name`、`reparse_point_attribute_get_utf16_print_name_size`、`reparse_point_attribute_get_utf16_print_name` | 10 | `record_metadata` reparse 字段 / WOF reader / 原始值 |
| `security_descriptor_attribute_get_security_descriptor_size`、`security_descriptor_attribute_get_security_descriptor` | 2 | `security_descriptor` / 原始安全属性 |
| `standard_information_attribute_get_creation_time`、`standard_information_attribute_get_modification_time`、`standard_information_attribute_get_access_time`、`standard_information_attribute_get_entry_modification_time`、`standard_information_attribute_get_file_attribute_flags`、`standard_information_attribute_get_owner_identifier`、`standard_information_attribute_get_security_descriptor_identifier`、`standard_information_attribute_get_update_sequence_number` | 8 | `record_metadata` / 原始 `$STANDARD_INFORMATION` |
| `volume_information_attribute_get_version`、`volume_information_attribute_get_flags` | 2 | `volume_metadata.version/flags` |
| `volume_name_attribute_get_utf8_name_size`、`volume_name_attribute_get_utf8_name`、`volume_name_attribute_get_utf16_name_size`、`volume_name_attribute_get_utf16_name` | 4 | `volume_metadata.name` / 原始 UTF-16 属性值 |
| `data_stream_free`、`data_stream_get_utf8_name_size`、`data_stream_get_utf8_name`、`data_stream_get_utf16_name_size`、`data_stream_get_utf16_name`、`data_stream_read_buffer`、`data_stream_read_buffer_at_offset`、`data_stream_seek_offset`、`data_stream_get_offset`、`data_stream_get_size`、`data_stream_get_number_of_extents`、`data_stream_get_extent_by_index` | 12 | `list_data_streams` / named stream full、stream、range / extents |
| `mft_metadata_file_initialize`、`mft_metadata_file_free`、`mft_metadata_file_open`、`mft_metadata_file_open_wide`、`mft_metadata_file_open_file_io_handle`、`mft_metadata_file_close`、`mft_metadata_file_get_utf8_volume_name_size`、`mft_metadata_file_get_utf8_volume_name`、`mft_metadata_file_get_utf16_volume_name_size`、`mft_metadata_file_get_utf16_volume_name`、`mft_metadata_file_get_volume_version`、`mft_metadata_file_get_volume_flags`、`mft_metadata_file_get_number_of_file_entries`、`mft_metadata_file_get_file_entry_by_index` | 14 | `NtfsMftMetadataFile`（元数据；外部簇不可用） |
| `usn_change_journal_free`、`usn_change_journal_get_offset`、`usn_change_journal_read_usn_record` | 3 | `visit_usn_journal` / `read_usn_journal` |

</details>

共 197 个公开函数；头文件 SHA-256：`16767e38a7c50f054325abc319e37bb34b6e819323dda183d196ecdb79ea3139`。
