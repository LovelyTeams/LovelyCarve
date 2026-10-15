# GZIP 读取器

`src-tauri/vendor/gzipf` 是参照 libyal/libgzipf 重写的独立 Rust crate。
它提供只读 GZIP 流、成员元数据与解压数据 seek，已作为宿主 path 依赖
接入 `.tar.gz` / `.tgz`、Docker / OCI 的 GZIP 层，以及小米 `.bak` 中的
GZIP Android Backup payload。非 GZIP 的 zlib/DEFLATE 仍由原有读取器处理。

参照提交为
[d1b4e6ba5911e1b86a3f9d4ca3fd31f1d69a6b68](https://github.com/libyal/libgzipf/commit/d1b4e6ba5911e1b86a3f9d4ca3fd31f1d69a6b68)，
对照该版本 `include/libgzipf.h.in` 的全部 42 个公开函数。
格式依据为 [RFC 1952](https://www.rfc-editor.org/rfc/rfc1952)。
没有逐行翻译 C 代码，也没有捆绑 libgzipf。

## 公开 API 对照

下表函数名统一省略 `libgzipf_` 前缀；计数合计 42。

| 上游函数 | 数量 | Rust 对应与差异 |
| --- | ---: | --- |
| `get_version`, `get_access_flags_read` | 2 | `VERSION` 是 Rust crate 版本；File::open 天然只读，不暴露访问位掩码 |
| `get_codepage`, `set_codepage` | 2 | Rust Path/OsStr 处理宿主路径，无进程级代码页；GZIP 元数据固定 Latin-1 |
| `check_file_signature`, `check_file_signature_wide`, `check_file_signature_file_io_handle` | 3 | `check_file_signature`, `check_reader_signature`, `check_signature`；Path 支持 Unicode；只有签名探测，不宣称文件有效 |
| `notify_set_verbose`, `notify_set_stream`, `notify_stream_open`, `notify_stream_close` | 4 | 不移植上游进程级调试输出；由调用者处理日志 |
| `error_free`, `error_fprint`, `error_sprint`, `error_backtrace_fprint`, `error_backtrace_sprint` | 5 | std::io::Error、Result、Display 与 Rust 自动释放；不实现 C 错误对象和上游 backtrace 格式 |
| `file_initialize`, `file_free`, `file_close`, `member_free` | 4 | 构造器、所有权与 Drop；无手工释放或关闭函数 |
| `file_signal_abort` | 1 | Options::with_cancellation；原子标记在有界 I/O/解码之间检查，无轮询线程 |
| `file_open`, `file_open_wide`, `file_open_file_io_handle` | 3 | GzipFile::open/open_with_options/from_reader/from_reader_with_options；GzipDecoder 支持不可 seek 的 Read |
| `file_read_buffer`, `file_read_buffer_at_offset` | 2 | Read::read、GzipFile::read_at；后者移动逻辑游标 |
| `file_seek_offset`, `file_get_offset` | 2 | Seek、offset；负数/溢出报错，超过 EOF 可 seek、read 返回零 |
| `file_is_corrupted` | 1 | 严格打开失败；读途中失败使用 Result 和 has_failed。不会返回容错正文及独立 corruption 标记 |
| `file_get_uncompressed_data_size`, `file_get_number_of_members`, `file_get_member_by_index` | 3 | uncompressed_size、members().len()、member(index)；seekable 打开先验证全部成员，流式仅发布已验证成员 |
| `member_get_modification_time`, `member_get_operating_system` | 2 | Member 的原始字段；MTIME 为 POSIX UTC 秒，零表示未记录；未知 OS 保留 |
| `member_get_utf8_name_size`, `member_get_utf8_name`, `member_get_utf16_name_size`, `member_get_utf16_name` | 4 | name_utf8/name_utf16 和 String/Vec 的 len；不含 C NUL；原始字节保留于 name |
| `member_get_utf8_comments_size`, `member_get_utf8_comments`, `member_get_utf16_comments_size`, `member_get_utf16_comments` | 4 | comment_utf8/comment_utf16；缺失与显式空字段可区分，无 C 缓冲区复制 API |

## 格式与完整性

支持单/多成员、空成员、stored/fixed/dynamic Huffman DEFLATE，全部五个
已定义头标记，header CRC16（CRC32 低 16 位）、正文 CRC32 和 trailer
ISIZE。保留 extra/name/comment 的原始字节，记录 header、压缩正文、
trailer、成员结束和逻辑数据区间；extra 子字段不解释为宿主路径或索引。

实际输出长度由 inflater 的 64 位计数确定，ISIZE 只比较 modulo 2^32，
不从 trailer 预测大小或分配正文。算术、头长度、累计头大小、成员数、
输入和输出大小均检查；未知压缩方法、保留头位、校验失败、短读与尾随
杂字节严格报错。底层 I/O 错误传播，不合成零填充。

流式返回的前缀在 trailer 未读完时尚未校验；调用者必须 drain 到 EOF
或调用 finish 后才能记录完整成功。错误使读者失效。seekable 打开先
全量验证并只缓存成员索引，读取期间检材必须保持稳定。

## 限制与成本

- 宿主复用既有桌面与 CLI 证据入口，不新增 IPC 或 GZIP 专用界面；没有
  上游 C ABI、Python binding 或工具。GzipFile 的 seek 接口仍是独立库能力，
  应用按原有模型物化解压 TAR / 层数据，不在日常浏览时重复解压成员。
- seek 重新解压所在成员的前缀，O(成员内偏移)；无压缩块 checkpoint。
- 默认输入/输出各 1 TiB、单头 1 MiB、累计头 64 MiB、65,536 成员，
  可通过 Options 调整。输入缓冲和单次解码输出各 64 KiB；缓存可能提前
  读取一个缓冲区，压缩上限按消费量检查。成员元数据占用与索引大小相关。
- 严格模式拒绝非 RFC 的尾随零填充，不实施损坏数据恢复。
- FEXTRA 保留原始字节，不实现 BGZF 或应用私有索引的语义。
- 没有写入；不把 metadata 名称直接用于宿主文件导出。

## 验证

通过 `just test-gzipf`、`just test-vendor gzipf`、
`just clippy-gzipf -- -D warnings` 和 `just fmt` 验证。
夹具来自 Python/C zlib 的独立编码，覆盖三种 DEFLATE block 和五成员流；
另有独立按位 CRC 与手工 stored block 最小样本。

测试覆盖每个最小样本截断点、可选头截断、CRC/ISIZE/DEFLATE 损坏、
Latin-1、空字段、精确源区间、碎片 I/O、边界/反向/跨成员 seek、EOF、
资源上限、取消与错误后的读取。生成器、许可证和 SHA256SUMS 位于
`src-tauri/vendor/gzipf/tests/data/`。

本次没有运行 libgzipf C 库的逐字节 oracle，也没有验证真实检材或 >4 GiB
单成员；64 位长度与 ISIZE 回绕按格式实现，不能视为大样本已实测。

## 宿主接入

`tar_image::open_gzip` 将完整多成员流解压到 NamedTempFile，全部校验成功
后才建立并发布 TAR 视图；错误自动删除临时文件。证据路径与名称仍为
原始压缩包，导航、范围读取和十六进制读取沿用物化后的 TAR 字节。
扩展名探测沿用既有 RAW 回退：损坏 `.tgz` 可以作为原始字节检查，但
不会暴露部分解压的 TAR 文件树。

Docker / OCI 层保留 8 GiB 解压上限，由 gzipf 的输出计数实施；不使用
Read::take 截出伪 EOF。所有成员、包括尾部空成员的 CRC 都校验成功后
才交给合层器。Docker 合层失败时仍按现有规则回退普通外层 TAR。

小米备份快速索引路径同样完整物化并验证；兼容的逐文件提取入口先解压
到自动清理的临时 TAR，验证压缩流成功后再提取，避免 tar::Archive 的
结束标记使后续 GZIP 成员和 trailer 被忽略。GZIP 分支使用库默认限额，
大归档输入/输出默认各最多 1 TiB；ZLIB 分支维持原路径。

集成回归覆盖 TAR 头/正文跨成员边界、原路径保留与随机读取、Docker
合层/分块读取、备份索引/提取、错误尾部与 RAW 回退。CLI smoke 用
Python/C zlib 现场生成独立的多成员 tar.gz / tgz，通过 image_load、
fs_read、fs_digest 和 image_unload 验证相同路径；运行 `just test-cli`。

宿主接入验证已通过 `just check`、`just clippy`、`just check-examples`、
`just test --lib --jobs 2`（1031 通过、12 项可选测试忽略）和
`just test-cli`。独立库 `just test-gzipf --locked` 为 31 项单元测试与
1 项文档测试通过；未提供的真实检材 / C oracle 不计入已验证范围。
