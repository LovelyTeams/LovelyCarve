# 卷影副本（VSS）

NTFS 卷影副本由 Apache-2.0 的 vendored `vsc-core` 加本地 Rust 实现读取，不加载或分发 libvshadow DLL，也没有移植其 C 代码。
打开的副本按需重建为只读会话镜像，可以像普通镜像一样浏览、哈希和导出。

本文以 [libvshadow 固定提交](https://github.com/libyal/libvshadow/tree/fa4776f06ec965cde633d3637227562609138c4c)
`fa4776f06ec965cde633d3637227562609138c4c`（2026-10-13 核对）的
[公开头文件](https://github.com/libyal/libvshadow/blob/fa4776f06ec965cde633d3637227562609138c4c/include/libvshadow.h.in)、
[格式说明](https://github.com/libyal/libvshadow/blob/fa4776f06ec965cde633d3637227562609138c4c/documentation/Volume%20Shadow%20Snapshot%20%28VSS%29%20format.asciidoc)
及 `libvshadow_io_handle.c`、`libvshadow_store_descriptor.c`、`libvshadow_block_tree.c`、
`libvshadow_volume.c`、`libvshadow_ntfs_volume_header.c`、Python 包装和工具源码为参照。
公开头文件共 48 个函数，含条件编译的 wide / BFIO 入口。下文的对应指读取语义与应用能力，不是 C ABI 兼容承诺。

## 格式与运行能力

| 能力 | libvshadow | LovelyCarve | 验证与边界 |
| --- | --- | --- | --- |
| 卷头 v1/v2、签名、catalog 指针、volume/storage GUID、最大存储尺寸 | 支持 | `check_volume_signature`、`VssVolume::header` | v1/v2、错误类型/版本、截断；签名探针恢复原游标 |
| 无 VSS、启用 VSS 但无 catalog、副本枚举 | 支持 | 没有签名与有头但零副本分开处理 | 合成回归；损坏目录不可伪装零副本 |
| catalog type 0/1/2/3、散布及多块目录 | 支持 | 按 GUID 配对，按 FILETIME 最早在前 | 跨块、指针先于描述符、重复/孤立 GUID、未知类型 |
| 原始 catalog 元数据 | 支持部分 getter | `StoreDescriptor` 保留尺寸、sequence、flags、FILETIME、所有数据指针、NTFS reference、allocated size | 原始数值不因尺寸修正而覆盖 |
| in-volume data 判定 | 支持 | `StoreDescriptor::has_in_volume_data` | 仅表示 type-3 存在，不保证数据可读 |
| 存储头、copy/set GUID、context、属性、机器名 | 支持 | `store_info` 完整读取 16 KiB 并检查 version/type/UTF-16 长度 | 零/异常信息尺寸字段不裁剪身份；非法长度报错；孤立 surrogate 仍按原解析器用 U+FFFD |
| 原始块枚举及全部五个值 | 支持 | `store_blocks` 返回未折叠、非空记录，含覆盖和 not-used 记录 | 原始计数与 `StoreMap::block_count` 的折叠计数不同 |
| diff-area 区间表 | 内部解析 | `store_ranges` 公开枚举 type-5 链 | 对齐、长度、溢出、源范围、循环；真实 dfVFS 样本两组区间表 |
| 普通 COW、替换记录、overlay 合并 | 支持 | 原有 forward/reverse map；512 字节 overlay 仅用于活动副本 | 合成测试及真实双副本完整摘要 |
| forwarder、自转发、反向替换、跨副本回落 | 支持 | 从当前到更新存储区，再到现存卷；迭代读取避免目录长度导致递归栈耗尽 | 20,000 个空 StoreMap 的栈安全回归；真实样本不含 forwarder |
| current/previous bitmap、未分配补零 | 支持 | 仅直接读取最新副本时使用，排除 forwarder targets | 当前与前一位图门控；正常数据短读不可补零 |
| 原始 catalog size 与可读卷尺寸分离 | 支持 | `stores()[i].volume_size` 保留原值；`snapshot_size` / layout 使用可读尺寸 | 一致且在源范围内时用 catalog；异常/不一致时验证 NTFS 主备 BPB |
| NTFS 备份扇区及尺寸溢出 | 支持 | BPB sector count 加最后备份扇区；主备几何必须一致、完整位于源卷 | 512/1024/2048/4096 合成回归；不猜测被截断的卷尺寸 |
| 顺序、定位、范围、seek/read | 支持 | `Snapshot: Read + Seek`、`read_at`、共享 `SnapshotLayout` | 跨 16 KiB/512 字节边界、尾部、负 seek、溢出；定位读取不移动游标 |
| 并发只读 | README 列为进行中，源码有条件锁 | owned layout/map 为不可变 `Send + Sync`，每次传定位 reader | 独立游标；并发共享可变 File 时由调用方同步 |
| 任意证据源、分区 offset、已解锁 BitLocker | BFIO/工具 offset | 泛型 `Read + Seek`；应用 `LiveCursor` 在 NTFS 分区/解密卷上定位 | 主工程合成分区及真实 RAW；尚无 BitLocker 卷内 VSS 的真实样本 |
| 取消、源短读/坏区、源卸载 | abort/错误传播 | 应用 reader 轮询 CancellationToken；源弱引用、范围/坏区校验 | 已取消任务与源卸载回归；不额外创建轮询线程 |
| info、完整副本导出、文件浏览 | vshadowinfo/export/mount | `probe-vsc`；副本作为会话镜像复用 RAW 导出、哈希、NTFS 文件浏览/导出 | 不另建一套导出或挂载生命周期 |
| Python、C ABI、FUSE/Dokan 专用块挂载 | 包装/工具 | Rust API、桌面和现有无窗口 CLI；应用文件挂载走既有路径 | 没有 pyvshadow 模块、C ABI 或 `/vssN` FUSE 兼容路径 |
| Windows 2003 R2 / 外部 diff area | 缺 type-3 可枚举；格式文档仍有未解问题 | 可枚举 type-2，读取明确 `StoreInfoUnavailable` | 不把当前卷当作缺失快照数据 |
| scoped snapshots、已删 catalog/store 雕刻、写入 | scope 不保证完整历史文件；非删除恢复/写库 | 保留原始 context/flags；只读有效目录，不承诺所有文件均有历史正文 | 不提供雕刻恢复，也不把 scope 当作完整备份 |

## 48 个公开函数逐项对应

表中 `StoreDescriptor` 为 `stores()[index]`；所有 Rust 索引均通过 `get` 或
返回 `StoreIndexOutOfRange` 的 reader 方法访问。调用者也可以对公开 Vec 使用 `get`。
日志、代码页、内存释放属于运行环境差异，与格式读取无关。

<details>
<summary>展开全部 48 个公开函数的对应关系</summary>

| libvshadow 函数（省略 `libvshadow_`） | Rust / 应用对应 |
| --- | --- |
| `get_version` | Cargo manifest / package metadata；vendor 来源固定记录 |
| `get_access_flags_read` | reader 仅 `Read + Seek`，没有写访问模式 |
| `get_codepage` | Rust String 为 UTF-8，无全局窄字符代码页 |
| `set_codepage` | OsStr 路径与 UTF-16 元数据独立处理；不提供全局代码页切换 |
| `check_volume_signature` | `check_volume_signature(&mut File)` |
| `check_volume_signature_wide` | `File::open(OsStr)` + 同一签名 reader |
| `check_volume_signature_file_io_handle` | 签名 reader 泛型 `Read + Seek` |
| `notify_set_verbose` | 应用 tracing filter；核心通过 Result 返回诊断 |
| `notify_set_stream` | 应用 tracing subscriber；probe 标准输出 |
| `notify_stream_open` | 宿主日志配置；无库内全局文件句柄 |
| `notify_stream_close` | 宿主日志生命周期 / Drop |
| `error_free` | `VssError` 所有权 / Drop |
| `error_fprint` | Display + 调用方输出 |
| `error_sprint` | Display / `to_string()` |
| `error_backtrace_fprint` | 应用 anyhow context 链和 tracing |
| `error_backtrace_sprint` | 应用 `{error:#}`；不复刻 C backtrace 对象 |
| `volume_initialize` | `VssVolume::open` 构造有效对象 |
| `volume_free` | Drop |
| `volume_signal_abort` | 应用 CancellationToken；独立 Rust 调用者提供可取消 reader |
| `volume_open` | `VssVolume::open(File::open(...))` |
| `volume_open_wide` | 同一构造 + OsStr |
| `volume_open_file_io_handle` | 泛型 `VssVolume<R>` / 应用 LiveCursor |
| `volume_close` | Drop；应用卸载释放父/子会话 |
| `volume_get_size` | `snapshot_size()` 为可读尺寸；`volume_size()` 为底层源长度 |
| `volume_get_number_of_stores` | `store_count()` |
| `volume_get_store` | `stores().get(index)` / `snapshot(index)` |
| `volume_get_store_identifier` | `StoreDescriptor.store_id` / `store_id_string()` |
| `store_free` | Snapshot / StoreInfo / StoreDescriptor Drop |
| `store_has_in_volume_data` | `StoreDescriptor::has_in_volume_data()` |
| `store_read_buffer` | `Snapshot: Read` |
| `store_read_buffer_from_file_io_handle` | borrowed Snapshot 或 layout 的定位 reader 回调 |
| `store_read_buffer_at_offset` | `Snapshot::read_at` / `SnapshotLayout::read_at`（严格填满；EOF 错误） |
| `store_seek_offset` | `Snapshot: Seek`；允许定位到 EOF 后，读取返回零 |
| `store_get_offset` | `Seek::stream_position` |
| `store_get_size` | `Snapshot::layout().volume_size()` |
| `store_get_volume_size` | `StoreDescriptor.volume_size` 原始 catalog 值 |
| `store_get_identifier` | `StoreDescriptor.store_id` / GUID formatter |
| `store_get_creation_time` | `StoreDescriptor.creation_time` 原始 UTC FILETIME（100 ns） |
| `store_get_copy_identifier` | `store_info(index)?.shadow_copy_id`；缺 type-3 返回不可用错误 |
| `store_get_copy_set_identifier` | `StoreInfo.shadow_copy_set_id` |
| `store_get_attribute_flags` | `StoreInfo.attributes.bits()` |
| `store_get_number_of_blocks` | `store_blocks(index)?.len()` 原始非空记录数 |
| `store_get_block_by_index` | `store_blocks(index)?.get(block_index)` |
| `block_free` | Copy record / Vec Drop |
| `block_get_original_offset` | `BlockDescriptor.original_offset` |
| `block_get_relative_offset` | `BlockDescriptor.relative_store_offset` |
| `block_get_offset` | `BlockDescriptor.store_offset` |
| `block_get_values` | 上述三 offset + `flags.bits()` + `allocation_bitmap` |

</details>

## 实现要点与刻意差异

- 遍历遇到损坏块、越界或循环时返回带偏移的错误，不返回部分目录；桌面与 CLI 的卷 `error`
  区分「无副本」和「读取失败」，存储头损坏通过副本的 `unavailable_reason` 报告。
- 存储头按 libvshadow 的方式整块读取并校验 version / type，不会因裁剪显示默认 GUID；应用打开时传播错误。
- catalog 中的原始卷尺寸保留供调查，布局另求共同的可读尺寸。物理 diff-area 地址按源长度校验，
  因此早期较小的卷也能从逻辑卷末尾之外读取保存的数据。
- 存储区链迭代读取，不依赖递归栈；每次源读取独立检查 offset + length。
- 按 GUID 延迟配对，允许 type-3 出现在 type-2 之前（libvshadow 文档假定 type-2 在前）。
  相同 FILETIME 的不同 GUID 保持原目录顺序，不丢弃记录；等时创建的链顺序尚无真实样本验证。
- 循环、重复 GUID / 指针、孤立指针、非对齐数据映射与超限明确失败。catalog 最多 4096 块；
  单个 store 的 bitmap / previous bitmap / block list 合计最多 4096 个 16 KiB 元数据块，
  range 枚举同样有界（约 64 MiB 输入，解码后的 map 内存另计）。超大的合法布局可能触发这些保护，
  此时报错而不是截断为成功；尚无数 TB 差分区样本验证。
- 原始细粒度元数据通过 Rust API 与探针提供，桌面列表和 IPC 字段不含块表，也没有专用块表面板。
  来源存在已记录坏区时，VSS 会话拒绝读取，避免伪造历史字节。

## 复现与验证边界

在仓库根目录运行。探针的输入是**从 NTFS 卷起点开始的 RAW**，不是带分区表的整盘：

```powershell
just probe-vsc '"<NTFS 卷 RAW 路径>"' --blocks --hash
just test-vsc
just clippy-vsc
$env:VSC_ORACLE_IMAGE = '<dfVFS vss.raw 路径>'
just test-vsc
$env:LOVELYCARVE_VSS_TEST = '<dfVFS vss.raw 路径>'
just test vss_image
```

CLI 提供 `shadow-copies list` / `shadow-copies open <分区> --index <序号>`。单次调用可用
`just run-cli --image <整盘或卷路径> shadow-copies list`；打开、浏览、导出等多步操作应在同一个 `shell` 或 JSONL
会话中进行，保持父镜像和副本存活，参数见 `just run-cli shadow-copies open --help`。
副本信息中的原始 `volume_size` 可能已损坏，打开后返回的 `ImageInfo.size` 才是实际可读尺寸。

公开样本使用 dfVFS `test_data/vss.raw`（提交 `0778d3fb20728cbf5c60e14b702527eab0b1c227`，82,771,968 字节，
SHA-256 `e633f0be5fb9ee9a07d44ba5223b786012ce89e9f0024f93d743568e6d052f16`）。
两个快照的完整读出与探针顺序哈希均与 libvshadow-python **20260714** 的对照值一致：

| Store GUID | 快照 SHA-256 | 原始块记录数 |
| --- | --- | --- |
| de81cc22-aa8b-11eb-9339-8cdcd4557abc | d830fe9e2f75e66edfe0f9b0730c0ec8148233ed9c2ab5601195c293d593d9a9 | 31 |
| de81cc2b-aa8b-11eb-9339-8cdcd4557abc | d47ca0497f4aa3098a7d0b5407a6406a4f8d747e6207fdbc197e2224fb4838ae | 38 |

原始块记录数来自 Rust 枚举，没有用 libvshadow 做 getter 级差分。主工程测试还检查重建后的 NTFS
根目录标记与文件浏览；合成回归覆盖 API、损坏、尺寸回退、游标 / EOF 和长链。随机多 store 差分语料的
生成方法与历史结果见 [oracle 说明](../../src-tauri/vendor/vsc/tests/oracle/README.md)。
通过 CLI 的 JSONL 会话打开第一个副本并用 `image_hash_compute` 计算，得到与上表相同的 SHA-256，
且 `bad_regions=[]`、`synthetic_bytes=0`、`read_faults_detected=false`。

尚未验证：所有 Windows 版本与 provider、外部存储区、scoped snapshots、4Kn 真实历史卷和并发压力；
源码对照不能替代这些真实样本。所有可选的外部对照在环境变量缺失时跳过。
