# QCOW / QCOW2 能力与 libqcow 对照

2026-10-13 对照 [libyal/libqcow 的固定提交 3e390fbb465f](https://github.com/libyal/libqcow/commit/3e390fbb465f37f3da92c1362c5df3da04ff9efe)，
逐项覆盖 `include/libqcow.h.in` 的全部 **42** 个公开函数（包括条件编译的 wide/BFIO 入口）。
同时核对上游 README、file/header/cluster/encryption 实现，以及
[QEMU QCOW2 格式规范](https://www.qemu.org/docs/master/interop/qcow2.html)。
Rust 实现位于 `src-tauri/vendor/qcow2`；宿主仍保留 `ImageFormat::Qcow2` 这一既有分类，包含 QCOW v1。
来源与许可证见 [LOVELYCARVE.md](../../src-tauri/vendor/qcow2/LOVELYCARVE.md)。

## 格式能力

| 能力 | libqcow | 本地原状 | 本次结果 |
| --- | --- | --- | --- |
| QCOW v1 | 支持 | 不支持 | 48 字节头、独立 L2 几何、8 字节 L1 对齐、随机读取 |
| QCOW v2/v3 | 支持 | 普通簇 | 保留普通读取，补 v2 扩展与现代 v3 头部 |
| 稀疏 / zero cluster | 支持 | 零偏移的 zero 判定不完整 | 显式零簇覆盖父镜像；v2 不接受 v3 的 zero bit |
| raw DEFLATE | 支持 | 明确拒绝 | v1 的字节长度 / v2-v3 的扇区描述符，完整流与解压长度校验 |
| backing file | 支持，调用者设置 parent | 明确拒绝 | 显式 parent API，自动 RAW/QCOW 相对路径链，循环 / 深度限制 |
| AES-128-CBC | 支持 | 明确拒绝 | 密码 / UTF-16 ASCII / 16 字节 key；扇区级 IV 与非对齐读取 |
| 元数据 | 版本、加密、尺寸、父文件名、快照数量 | 尺寸 / 簇大小 | `info()` 提供上述信息及原始 feature masks / backing format |
| read / read-at / seek / tell | 支持 | positioned read | 独立 `Reader` 实现 Read / Seek / ReadAt / Size |
| abort | 支持 | 无 | `read_at_cancellable` 使用操作级 predicate |
| 快照正文切换 | README 列为不支持 | 不支持 | 仅提供数量，读取当前活动 L1，不宣称历史视图 |
| QCOW 内嵌 LUKS / external data file | README 列为不支持 | 不支持 | 打开时明确拒绝；独立 LUKS 卷支持不等于 QCOW LUKS payload |
| Zstd / extended L2 | 此提交不支持 | 不支持 | 明确拒绝，不将未知布局按标准 L2 读取 |
| 写入 / refcount 修复 / compact | 只读 | 只读 | 保持只读；dirty refcount 不参与当前视图寻址 |

## 全部 42 个公开 API

下表比较语义能力，不提供 C ABI 兼容层。Rust 的路径、对象生命周期、错误链和日志沿用宿主机制。

| # | libqcow API | 本地对应 / 边界 |
| --- | --- | --- |
| 1 | `libqcow_get_version` | Cargo package version / 来源固定提交 |
| 2 | `libqcow_get_access_flags_read` | File::open，仅读取 API |
| 3 | `libqcow_get_access_flags_read_write` | 不提供写模式；上游声明不代表支持写镜像 |
| 4 | `libqcow_get_access_flags_write` | 同上 |
| 5 | `libqcow_get_codepage` | 无全局 codepage；Windows 严格 UTF-8、Unix 原路径字节 |
| 6 | `libqcow_set_codepage` | 不自动猜测其他编码；调用者可显式附加 parent |
| 7 | `libqcow_check_file_signature` | `check_signature(File)` |
| 8 | `libqcow_check_file_signature_wide` | Rust Path / File，Unicode 宿主路径 |
| 9 | `libqcow_check_file_signature_file_io_handle` | `check_signature(impl ReadAt)` |
| 10 | `libqcow_notify_set_verbose` | 宿主 tracing / RUST_LOG |
| 11 | `libqcow_notify_set_stream` | 宿主日志设施；不新增全局 C stream |
| 12 | `libqcow_notify_stream_open` | 同上，输出由宿主控制 |
| 13 | `libqcow_notify_stream_close` | 宿主/RAII 释放日志资源 |
| 14 | `libqcow_error_free` | Rust Error drop |
| 15 | `libqcow_error_fprint` | Display / 宿主日志 |
| 16 | `libqcow_error_sprint` | `error.to_string()` |
| 17 | `libqcow_error_backtrace_fprint` | `std::error::Error::source` / anyhow 上下文链；非 C 堆栈回溯 |
| 18 | `libqcow_error_backtrace_sprint` | 同上，anyhow 完整上下文格式化 |
| 19 | `libqcow_file_initialize` | Qcow2::open / open_with_key |
| 20 | `libqcow_file_free` | Rust ownership / drop |
| 21 | `libqcow_file_signal_abort` | read_at_cancellable，返回 Cancelled；不创建轮询线程 |
| 22 | `libqcow_file_open` | open_path，宿主自动识别共享签名 |
| 23 | `libqcow_file_open_wide` | Rust Path 处理 Unicode 路径 |
| 24 | `libqcow_file_open_file_io_handle` | open(impl ReadAt + Size)，Vec/File/自定义源 |
| 25 | `libqcow_file_close` | drop，父源 Arc 保持完整生命周期 |
| 26 | `libqcow_file_is_locked` | is_locked；无 key 仍可检查 header 元数据 |
| 27 | `libqcow_file_read_buffer` | Reader::read / read_exact |
| 28 | `libqcow_file_read_buffer_at_offset` | Qcow2::read_at / Reader::read_at |
| 29 | `libqcow_file_seek_offset` | Reader::seek，检查负数与溢出 |
| 30 | `libqcow_file_get_offset` | Reader::position；read-at 不改变 cursor |
| 31 | `libqcow_file_set_parent_file` | set_backing(Arc<dyn BackingSource>)；支持逻辑 QCOW 和 RAW |
| 32 | `libqcow_file_get_format_version` | info().version |
| 33 | `libqcow_file_get_encryption_method` | info().encryption_method |
| 34 | `libqcow_file_set_keys` | EncryptionKey::from_bytes + open_with_key |
| 35 | `libqcow_file_set_utf8_password` | from_password；只接受无 NUL 的 ASCII，取前 16 字节、短密码补零 |
| 36 | `libqcow_file_set_utf16_password` | from_utf16_password；无损解码并执行同样 ASCII 规则 |
| 37 | `libqcow_file_get_media_size` | guest_size / info().media_size / Size |
| 38 | `libqcow_file_get_utf8_backing_filename_size` | PathBuf / UTF-8 编码长度；Rust 不要求调用者分配 C 缓冲区 |
| 39 | `libqcow_file_get_utf8_backing_filename` | info().backing_filename，保留原始路径 |
| 40 | `libqcow_file_get_utf16_backing_filename_size` | Unicode 路径可 encode_utf16；Unix 非 UTF-8 路径不伪造转码 |
| 41 | `libqcow_file_get_utf16_backing_filename` | 同上，Rust PathBuf |
| 42 | `libqcow_file_get_number_of_snapshots` | info().snapshot_count；不切换快照正文 |

## 读取完整性与限制

- 未分配 L1/L2 项从父源读取；没有父源时按格式读零。若 header 声明父文件却未附加，返回 MissingBacking。
  自动路径打开必须打开完整父链，即使本次读取只触及子镜像分配簇。父文件自己的 EOF 之后读零，
  其声明尺寸内发生的短读仍是错误。显式 backing format 优先，未知 format 明确失败。
- v3 zero bit 与 host offset 无关：无分配零簇、已分配零簇均读零，不回退父源。
  标准簇、L1/L2 表均校验保留位、对齐、范围与乘加；只有被请求的有效逻辑字节需要存在，
  不强制把合法的 v2/v3 最后数据簇补到簇尾。
- 压缩数据使用无 zlib wrapper 的 DEFLATE。读入编码扇区范围（允许物理 EOF 缺少尾部扇区 padding），
  但必须到达 StreamEnd 且解码结果恰好一簇；缺少压缩正文、损坏/膨胀输出均报错。
- legacy AES 使用每个 512 字节 guest sector 的编号（小端，IV 后 8 字节为零）。
  普通随机读仅解密覆盖请求的完整扇区，空洞和 zero 项不解密。密钥与临时明文使用 zeroize。
  **该格式没有密码/密钥验证器**，错误 key 可能返回任意明文，成功打开不证明密码正确。
  同一簇同时标记压缩与加密明确拒绝。
- 簇大小 512 字节–2 MiB；L1 最多 32 MiB，预留表项允许多于当前虚拟盘必需数量。
  QCOW1 L2 bits 为 1–21；media/seek 范围最多 i64::MAX。
  父链最多 32 个文件；路径 canonicalize 后检测重复。手动 parent 由调用者提供正确逻辑源。
- L1 开启时读取一次；L2 descriptor cache 为 4096 项，decoded cache 为 8 簇，
  单镜像最多保留 16 MiB 解压结果。并发读不共享顺序游标。
- dirty bit 的 refcount 不参与只读逻辑寻址，原标记留在 info；corrupt bit 拒绝。
  兼容/auto-clear 位保留，不执行修复。扩展长度/重复/父名称边界均受检；未知兼容扩展跳过。
- cancellation predicate 在簇边界及读取/解码前后检查；单次 I/O 或解码内部不能抢占，
  路径链打开无独立取消接口。宿主任务仍按自己的 CancellationToken 和 chunk 边界取消。
  这不等于所有宿主调用都使用库的 cancellable 方法。

## 宿主与 CLI

普通打开自动支持 v1/v2/v3、压缩簇和 RAW/QCOW 差分父链，沿用原证据身份和逻辑尺寸。
`image_load`（CLI 的 `images open`）的两个可选参数是 `qcowPassword` / `qcowKeyHex`，不可同时提供；
keyHex 必须恰好 16 字节（32 个十六进制字符）。原无参数调用完全兼容。命令行中单写
`--qcow-password` 时在终端中提问或读取管道输入。JSONL `session` 示例（密码是公开测试夹具密码）：

```jsonl
{"id":"qcow","command":"images open","args":{"path":"D:/evidence/encrypted.qcow2","qcowPassword":"qcow-test"}}
```

凭据只进入本次调用和内存读取器，不进入案件清单、task 描述或日志；程序重启后需重新提供。
自动父链用同一 key 打开所有加密成员；不同成员密码可在独立 Rust API 中逐层打开、set_backing。
桌面 `useFileSession.loadFile` 的 options 与类型化 IPC 已能传递凭据；普通文件选择器尚无
QCOW 专用密码输入弹窗。现有分区解锁弹窗也不用于整个 QCOW 容器。

## 验证

`just test-qcow2` / `just clippy-qcow2` 验证独立库；
`just test qcow --lib --jobs 1` 验证宿主 signature / 压缩 / 差分 / AES。
IPC 改动使用 `just verify-fe`、`just check`、`just clippy` 和 `just check-examples`。
CLI 的 `images open` 由命令注册表与 `cli/commands.toml` 生成：命令行用 `--qcow-password` / `--qcow-key-hex`，JSONL 会话用 `qcowPassword` / `qcowKeyHex`。

生成样本来自独立 Python zlib / cryptography-OpenSSL，实现及 SHA-256 见
[测试夹具记录](../../src-tauri/vendor/qcow2/tests/data/README.md)，覆盖各版本、扇区跨界、
随机/顺序/逆序/并发读取、压缩坏尾、reserved bits、扩展溢出、parent 缺失/循环/深度、zero 与取消。
没有真实检材或编译 libqcow 的逐字节 oracle，因此不宣称已经通过这两类验证。

2026-10-13 Windows 验证结果：独立 QCOW 测试 18/18；后端库测试 1036 通过、
12 个外部样本测试忽略；前端测试 777 通过、1 个忽略；CLI 冒烟（含各版本
DEFLATE / 密码 / 原始 key 的逻辑 RAW 导出与错误参数）、fmt、check、clippy、
clippy-qcow2、check-examples、IPC/结构/typecheck/生产构建均通过。
macOS / Linux 构建和真实镜像尚未验证。
