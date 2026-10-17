# 单文件与分段 RAW

`src-tauri/vendor/smraw` 是参照 libyal/libsmraw 重写的独立纯 Rust crate，无第三方运行依赖。
宿主将分段 RAW 接入 `ImageSource::SplitRaw`，格式身份沿用 `raw`；单文件 RAW 保留 mmap。
分区检测、文件系统、雕刻、哈希和 RAW 导出共用完整逻辑字节流，无新增 IPC。

参照提交为 [adfa6b8bc8617226805926180244c774366aad30](https://github.com/libyal/libsmraw/commit/adfa6b8bc8617226805926180244c774366aad30)，
按 `include/libsmraw.h.in` 的 72 个公开函数核对。这是 Rust 语义接口重写，无上游 C 源码或二进制兼容层。

## 公开 API 对照

省略 `libsmraw_` 前缀，计数合计 72。对应不表示 C ABI 或全部行为相同。

| 上游函数 | 数量 | Rust 对应与差异 |
| --- | ---: | --- |
| `get_version` | 1 | crate 版本 0.1.0，不伪装成上游版本 |
| `get_access_flags_read`, `get_access_flags_read_write`, `get_access_flags_write` | 3 | `RawImage` 只读；`RawWriter` 只创建新输出，无访问掩码或检材原位修改 |
| `get_codepage`, `set_codepage` | 2 | Path/OsStr，无进程级代码页；元数据使用 UTF-8/UTF-16 |
| `glob`, `glob_free`, `glob_wide`, `glob_wide_free` | 4 | `discover`、自动所有权，Unicode 路径，扫描家庭编号报告缺段 |
| `notify_set_verbose`, `notify_set_stream`, `notify_stream_open`, `notify_stream_close` | 4 | 调用者处理日志，未移植全局日志对象 |
| `error_free`, `error_fprint`, `error_sprint`, `error_backtrace_fprint`, `error_backtrace_sprint` | 5 | `io::Error`、Result、Display、Drop，未移植 C 错误链格式 |
| `handle_initialize`, `handle_free`, `handle_close` | 3 | 构造器和 Drop，新输出须显式 finish 才保留 |
| `handle_signal_abort` | 1 | `AbortHandle`，每段及每个最多 1 MiB 的 I/O 检查，无轮询线程 |
| `handle_open`, `handle_open_wide`, `handle_open_file_io_pool` | 3 | `RawImage::open/open_segments`，Path 或显式列表，无 libbfio 池接口 |
| `handle_read_buffer`, `handle_read_buffer_at_offset` | 2 | `RawCursor: Read`、`RawImage::read_at`；positioned 读取不移动游标 |
| `handle_write_buffer`, `handle_write_buffer_at_offset` | 2 | `RawWriter: Write`、`write_at`；仅修改其新输出，禁止未写区间空洞 |
| `handle_seek_offset`, `handle_get_offset` | 2 | Read/Write 游标的 Seek/position；负数/溢出报错，读允许超 EOF，写禁止跳过未写区间 |
| `handle_set_maximum_number_of_open_handles` | 1 | `set_maximum_open_handles`，缓存 1–65536 句柄；活动读取可临时持有淘汰句柄 |
| `handle_get_segment_filename_size`, `handle_get_segment_filename`, `handle_set_segment_filename`, `handle_get_segment_filename_size_wide`, `handle_get_segment_filename_wide`, `handle_set_segment_filename_wide` | 6 | `segments`、`open_segments` 和 writer basename；打开后列表不可改，Path 无 NUL 尺寸调用 |
| `handle_get_maximum_segment_size`, `handle_set_maximum_segment_size` | 2 | 读取返回成员最大长度，写入创建时指定非零上限，打开后不可变 |
| `handle_get_filename_size`, `handle_get_filename`, `handle_get_filename_size_wide`, `handle_get_filename_wide` | 4 | `RawCursor::segment`、`RawImage::segment_at`、`Segment::path`，EOF 无所属段 |
| `handle_get_file_io_handle` | 1 | 不暴露可变底层句柄，维护共享只读契约 |
| `handle_get_media_size`, `handle_set_media_size` | 2 | `media_size`、`require_media_size`；读取尺寸不可伪造，写入长度声明并在 finish 验证 |
| `handle_get_bytes_per_sector`, `handle_set_bytes_per_sector` | 2 | 调用者 `MediaMetadata::bytes_per_sector: Option<u32>`，默认未知 |
| `handle_get_media_type`, `handle_set_media_type` | 2 | 调用者 `media_type: Option<u8>`，默认未知 |
| `handle_get_media_flags`, `handle_set_media_flags` | 2 | 调用者 `media_flags: Option<u8>`，默认未知 |
| `handle_get_number_of_information_values`, `handle_get_information_value_identifier_size`, `handle_get_information_value_identifier`, `handle_get_utf8_information_value_size`, `handle_get_utf8_information_value`, `handle_set_utf8_information_value`, `handle_get_utf16_information_value_size`, `handle_get_utf16_information_value`, `handle_set_utf16_information_value` | 9 | `metadata.information` 的有序 `len/get/get_index/iter/set/get_utf16/set_utf16`；不读写 .info |
| `handle_get_number_of_integrity_hash_values`, `handle_get_integrity_hash_value_identifier_size`, `handle_get_integrity_hash_value_identifier`, `handle_get_utf8_integrity_hash_value_size`, `handle_get_utf8_integrity_hash_value`, `handle_set_utf8_integrity_hash_value`, `handle_get_utf16_integrity_hash_value_size`, `handle_get_utf16_integrity_hash_value`, `handle_set_utf16_integrity_hash_value` | 9 | `metadata.integrity_hashes` 同上；仅调用者声明，未验证哈希，宿主用既有扫描引擎 |

## 发现与完整性

- 数字扩展 `.0/.1`、`.00/.01`、`.000/.001`、`disk.raw.001`，无点尾缀
  `00/000`、小写 `aa/aaa`；数字按原始 padding 及进位匹配。basename 探测顺序为
  `.raw/.dmg/.img/.dd/.000/.001/.00/.01/.0/.1/aa/00/aaa/000`。
  不支持 glob 通配符、任意大写字母命名或 GNU split 可变长字母后缀；自定义命名用显式列表。
- 库接受任意成员并返回首段开始的列表，宿主要求选择首段以保留案件检材路径身份。
  EWF 使用既有读取器优先处理。ASCII 文件名大小写匹配遵循宿主平台约定。
- 支持不等长段；多段空文件、重复规范路径、非文件、尺寸溢出及已有后续段之前的编号空缺报错。
  最大 65536 个输入段，发现目录最多 1000000 条目，默认缓存 64 句柄。
- 写入最多 999 段，单段 `<basename>.raw`，多段 `.raw.001` 起；按需 create_new 并保留已创建段的句柄。
  finish 核对完整长度并 sync，未完成输出 Drop 清理，清理失败打印错误。库写入未接入桌面分段导出，现有导出仍生成单个 RAW。
- 源文件长度/时间打开时记录，读前后校验句柄，重开时校验新句柄；宿主稳定性与 fingerprint 包含所有段。
  短读或源变化失败，无坏区补零。缓存读取已打开句柄，路径替换由 validate_sources 或宿主稳定性检查检测。
- RAW 无签名、声明总长、分段总数或校验值。无法证明最后一段未丢失、首段 `.000` 是否本应存在、
  或打开前未截断；不等长段不能自动判为损坏。有独立记录时使用 require_media_size 和独立哈希。
  元数据检查不等同于文件锁或密码学完整性证明。
- `.info` sidecar 不解析、不创建；元数据为调用者内存声明，未知几何保留 None，不声称来自检材。
  每个元数据表最多 4096 值/1 MiB，单值最多 64 KiB；拒绝 NUL、无效 UTF-16 或非法标识符。

## 验证

`just test-smraw` 覆盖逐范围拼接、数字/字母命名与进位、Unicode、空段/缺段、并发与取消、
源截断/丢失、写入回读与回写、目标冲突、输出截断及未完成清理。合成夹具在临时目录生成，AGPL-3.0-only。

`just test smraw --lib --jobs 1` 覆盖 GPT 头/数组非扇区对齐分段后的识别、完整/范围/大缓冲读取与 SHA-256、
所有段快照、后续段拒绝和普通 RAW mmap。`just test-cli` 中 Python 构造三个不等长段，
通过真实的 `images open` / `images export`（`image_load` / `image_export_raw`）验证完整导出和缺段报错。

编译/lint 用 `just check`、`just clippy`、`just clippy-smraw`、`just check-examples`，格式化走 `just fmt`。
未运行真实检材或 libsmraw 二进制 oracle，不声称实际采集软件命名与 .info 已逐字节验证；macOS/Linux 仍需单独验证。
