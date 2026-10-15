# Parallels Hard Disk Image（PHDI）

`src-tauri/vendor/phdi` 是原创只读 Rust crate；以
[libphdi 固定提交 fb132dceb2a1](https://github.com/libyal/libphdi/commit/fb132dceb2a1a43c2597b10bbf9e65757d08cf7c)
的格式与全部 56 个公开 API 为参照。它不提供 C ABI，不加载 C/DLL。
来源和许可证见 vendor 的 ORIGIN.md / LICENSE 及 THIRD_PARTY_NOTICES.md。

## 能力与读取规则

| 场景 | 实现 |
| --- | --- |
| 描述符 / bundle | UTF-8 `DiskDescriptor.xml`、`.hdd` 目录；显式指定 `.Backup` 可读，无自动切换 |
| Plain | 按 extent 长度读取 RAW；来源长度不足直接报错 |
| Sparse HDS | `WithoutFreeSpace` v2；BAT 零项读取父层或根空洞，非零项必须在合法数据区 |
| Split | 验证完整、不重叠、无空隙的 extent 覆盖；随机读可跨 extent |
| Differential | GUID 父链；循环 / 缺失父节点 / 根数据文件 / 重复 GUID 均报错；缺少未变化的子 extent 可以继承父 extent |
| 选定快照 | 默认 `TopGUID`，未提供时遵循 libphdi 的第一个 `Shot`；库可通过 `OpenOptions.snapshot` 显式选定 GUID，宿主没有新增快照选择 UI / IPC |
| 元数据 | 磁盘名 / UID、逻辑及物理扇区大小、extent 字节范围、image 类型 / 文件名、snapshot / parent GUID；GUID 是 16 字节网络顺序 |
| I/O | `Read` / `Seek` / `position` / 并发 `read_at`；EOF 裁剪，分配区短读报错 |
| 资源 | 8 MiB XML、32768 XML 节点、64 层 XML、默认 16M BAT 项、16 文件句柄、128 层父链；句柄按使用顺序淘汰 |
| 取消 | `open_with_options` / `read_at_with_cancel` 直接轮询闭包，返回 `Interrupted`，宿主任务仍沿用已有取消流程 |
| 完整性 | checked 扇区乘加、BAT 数量、元数据区、分配范围与重叠、descriptor / HDS 几何；每次读取检查来源长度 / 修改 / 创建时间，真实分配错误不合成零 |

XML 中 Disk_size / Start / End / Blocksize 和 BAT 地址始终以 512 字节计算。
`LogicSectorSize` 仅决定暴露给分区检测的扇区几何。
`Compressed` 指稀疏格式，不进行 DEFLATE 解压。
历史 HDS 的 data-start 为零时使用 BAT 末尾向上对齐的扇区边界。

宿主 `ImageSource::Phdi` 将逻辑盘接入分区、文件系统、哈希、RAW 导出和 CLI
现有路径；`source_format` 为 `phdi`。来源快照包含描述符及所选父链所有数据文件，
复用现有证据稳定性校验和指纹，不新增长期 per-image 状态。
桌面选择器允许 `.hdd` / `.hds` / XML，仅 DiskDescriptor.xml 按描述符入口识别。
打开有描述符的独立稀疏 HDS 会提示选择描述符，防止丢失父链或分段。

## 限制

- 拒绝 v1、扩展 `WithouFreSpacExt`、Parallels 加密、未知 image 类型和非 UTF-8 XML。
  上游固定提交同样没有实现扩展签名。
- 不修改检材，不创建 / 合并 / 删除快照，不自动恢复描述符备份。
- extent 文件名须在指定目录内；拒绝绝对路径、`..`、盘符和逃逸目录的 symlink。
  如数据文件被移动，可使用库的 extent-directory 覆盖；不猜测路径。
- 独立 HDS 没有父链元数据，零 BAT 项按根空洞处理；差分数据须提供完整描述符。
- 稳定性检查基于文件长度和时间，不是内容认证或防篡改机制。
- 真实样本覆盖一个单 extent 基础盘；真实分段 / 差分 / 4Kn 样本、与原生
  libphdi 的逐字节 oracle、macOS/Linux 和实际只读挂载尚未验证。

## 验证

```powershell
just test-phdi
just clippy-phdi
just test phdi --lib --jobs 1
just check
just clippy
just ipc-check
just typecheck
just test-fe
just test-cli
```

合成回归覆盖 Plain / Sparse / Split / 父链、独立 HDS、EOF、随机 / 顺序 /
并发读取、Unicode XML、坏 XML / BAT、范围溢出、截断、缺段、循环、路径和资源限额、
取消、源变化；宿主测试验证 MBR 几何、逻辑范围读取和 fingerprint 来源列表。
CLI smoke 通过真实命令打开、RAW 导出、关闭并核对源文件未变。

外部样本取自上游 synctestdata.sh 同一公开来源：
[dfVFS 917cefc9426d 的 hfsplus.hdd](https://github.com/log2timeline/dfvfs/tree/917cefc9426d6ded2687d2b55164ded25b44fcb7/test_data/hfsplus.hdd)。
样本不进入本仓库。

| 文件 / 逻辑输出 | SHA-256 |
| --- | --- |
| DiskDescriptor.xml | `055612a450d55d0265fd117b5cf57355376ec4c62f9a80db78a6cdfe402c8a2a` |
| hfsplus.hdd.0.{5fbaabe3-6958-40ff-92a7-860e329aab41}.hds | `1b5192b42705b7b959da2590d590363cda0088a4834a29c2e2984d938eb1070c` |
| 33554432 字节逻辑盘 | `b5d1c3bb34c26e20976ba041e742c7529cce93b9cb00b435ba5c4af3e652a1e1` |

2026-10-13 用 Python `tests/oracle.py` 独立生成 RAW，Rust external oracle 测试
对全盘顺序读、跨块随机读和 EOF 逐字节通过；这不是 libphdi 二进制差分测试。
复现：运行 Python oracle 到一个新输出路径，设置 `PHDI_ORACLE_DESCRIPTOR` /
`PHDI_ORACLE_RAW`，再运行 `just test-phdi -- --include-ignored`。
普通配方会显式忽略外部测试，不把缺少样本计作通过。

## 公开 API 对照

以下列出固定提交 `include/libphdi.h.in` 的全部导出函数。
Rust 的 String / slices / RAII 代替 C 编码缓冲、计数和 free；主动差异逐项标明。

| libphdi API | Rust 对应 / 主动差异 |
| --- | --- |
| `libphdi_get_version` | Cargo package version；没有冒充 libphdi 版本 |
| `libphdi_get_access_flags_read` | File::open 只读；read/write/write-only 主动不支持 |
| `libphdi_get_access_flags_read_write` | File::open 只读；read/write/write-only 主动不支持 |
| `libphdi_get_access_flags_write` | File::open 只读；read/write/write-only 主动不支持 |
| `libphdi_get_codepage` | 无全局 codepage；只接受 UTF-8 XML |
| `libphdi_set_codepage` | 无全局 codepage；只接受 UTF-8 XML |
| `libphdi_check_file_signature` | is_sparse_signature；描述符由 Descriptor::parse 结构验证；Path 支持 Unicode，未接 libbfio |
| `libphdi_check_file_signature_wide` | is_sparse_signature；描述符由 Descriptor::parse 结构验证；Path 支持 Unicode，未接 libbfio |
| `libphdi_check_file_signature_file_io_handle` | is_sparse_signature；描述符由 Descriptor::parse 结构验证；Path 支持 Unicode，未接 libbfio |
| `libphdi_notify_set_verbose` | 由宿主日志和调用者记录 io::Error；不提供 C 全局日志流 |
| `libphdi_notify_set_stream` | 由宿主日志和调用者记录 io::Error；不提供 C 全局日志流 |
| `libphdi_notify_stream_open` | 由宿主日志和调用者记录 io::Error；不提供 C 全局日志流 |
| `libphdi_notify_stream_close` | 由宿主日志和调用者记录 io::Error；不提供 C 全局日志流 |
| `libphdi_error_free` | io::Error / Display / std::error::Error，RAII；不提供 C 错误 / 回溯对象 |
| `libphdi_error_fprint` | io::Error / Display / std::error::Error，RAII；不提供 C 错误 / 回溯对象 |
| `libphdi_error_sprint` | io::Error / Display / std::error::Error，RAII；不提供 C 错误 / 回溯对象 |
| `libphdi_error_backtrace_fprint` | io::Error / Display / std::error::Error，RAII；不提供 C 错误 / 回溯对象 |
| `libphdi_error_backtrace_sprint` | io::Error / Display / std::error::Error，RAII；不提供 C 错误 / 回溯对象 |
| `libphdi_handle_initialize` | Rust 所有权与 Drop；构造成功才返回 PhdiImage，文件句柄随 drop 释放 |
| `libphdi_handle_free` | Rust 所有权与 Drop；构造成功才返回 PhdiImage，文件句柄随 drop 释放 |
| `libphdi_handle_signal_abort` | open_with_options / read_at_with_cancel 的调用者闭包；无全局中止标志 |
| `libphdi_handle_open` | PhdiImage::open / open_with_options，Path 支持 Unicode；libbfio 句柄入口主动未实现 |
| `libphdi_handle_open_wide` | PhdiImage::open / open_with_options，Path 支持 Unicode；libbfio 句柄入口主动未实现 |
| `libphdi_handle_open_file_io_handle` | PhdiImage::open / open_with_options，Path 支持 Unicode；libbfio 句柄入口主动未实现 |
| `libphdi_handle_open_extent_data_files` | open_with_options 打开所选父链的全部 extent；Rust LRU 池，无 libbfio pool |
| `libphdi_handle_open_extent_data_files_file_io_pool` | open_with_options 打开所选父链的全部 extent；Rust LRU 池，无 libbfio pool |
| `libphdi_handle_close` | Rust 所有权与 Drop；构造成功才返回 PhdiImage，文件句柄随 drop 释放 |
| `libphdi_handle_read_buffer` | Read::read |
| `libphdi_handle_read_buffer_at_offset` | read_at / read_at_with_cancel，位置不变且支持并发 |
| `libphdi_handle_seek_offset` | Seek::seek(SeekFrom) |
| `libphdi_handle_get_offset` | position |
| `libphdi_handle_set_maximum_number_of_open_handles` | OpenOptions.maximum_open_handles；非零、有界 LRU 池，在打开时配置 |
| `libphdi_handle_set_extent_data_files_path` | OpenOptions.extent_directory；Unicode Path，在打开时配置；限制目录内文件 |
| `libphdi_handle_set_extent_data_files_path_wide` | OpenOptions.extent_directory；Unicode Path，在打开时配置；限制目录内文件 |
| `libphdi_handle_get_media_size` | media_size |
| `libphdi_handle_get_utf8_name_size` | descriptor().name: Option<String>；String::len / encode_utf16，Rust 无 NUL 缓冲协议 |
| `libphdi_handle_get_utf8_name` | descriptor().name: Option<String>；String::len / encode_utf16，Rust 无 NUL 缓冲协议 |
| `libphdi_handle_get_utf16_name_size` | descriptor().name: Option<String>；String::len / encode_utf16，Rust 无 NUL 缓冲协议 |
| `libphdi_handle_get_utf16_name` | descriptor().name: Option<String>；String::len / encode_utf16，Rust 无 NUL 缓冲协议 |
| `libphdi_handle_get_number_of_extents` | descriptor().extents.len() / .get(index) |
| `libphdi_handle_get_extent_descriptor_by_index` | descriptor().extents.len() / .get(index) |
| `libphdi_handle_get_number_of_snapshots` | descriptor().snapshots.len() / .get(index) |
| `libphdi_handle_get_snapshot_by_index` | descriptor().snapshots.len() / .get(index) |
| `libphdi_extent_descriptor_free` | Rust 所有权与 Drop；构造成功才返回 PhdiImage，文件句柄随 drop 释放 |
| `libphdi_extent_descriptor_get_range` | ExtentDescriptor.offset / size（字节） |
| `libphdi_extent_descriptor_get_number_of_images` | ExtentDescriptor.images.len() / .get(index) |
| `libphdi_extent_descriptor_get_image_descriptor_by_index` | ExtentDescriptor.images.len() / .get(index) |
| `libphdi_image_descriptor_free` | Rust 所有权与 Drop；构造成功才返回 PhdiImage，文件句柄随 drop 释放 |
| `libphdi_image_descriptor_get_type` | ImageDescriptor.kind: ImageType::Plain / Sparse |
| `libphdi_image_descriptor_get_utf8_filename_size` | ImageDescriptor.filename: String；len / encode_utf16，无 NUL 缓冲协议 |
| `libphdi_image_descriptor_get_utf8_filename` | ImageDescriptor.filename: String；len / encode_utf16，无 NUL 缓冲协议 |
| `libphdi_image_descriptor_get_utf16_filename_size` | ImageDescriptor.filename: String；len / encode_utf16，无 NUL 缓冲协议 |
| `libphdi_image_descriptor_get_utf16_filename` | ImageDescriptor.filename: String；len / encode_utf16，无 NUL 缓冲协议 |
| `libphdi_snapshot_free` | Rust 所有权与 Drop；构造成功才返回 PhdiImage，文件句柄随 drop 释放 |
| `libphdi_snapshot_get_identifier` | Snapshot.identifier: Guid([u8; 16]) |
| `libphdi_snapshot_get_parent_identifier` | Snapshot.parent: Option<Guid>；零 GUID 映射 None |
