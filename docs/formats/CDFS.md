# ISO 9660、光盘 BIN 与 CUE

LovelyCarve 使用 vendored `cdfs` 浏览 ISO 9660 / Joliet / Rock Ridge，
`core::evidence::readers::optical_disc` 提供 Mode 1 / XA Form 1 逻辑投影，
`readers::cue` 解析 CUE 并用 `smraw` 只读拼接数据文件。不链接 libodraw，不生成临时整盘 ISO。

## libodraw 对照基准

对照 [libyal/libodraw 固定提交 `67ee410e4e9e82a77fe0427899133dc07df99523`](https://github.com/libyal/libodraw/tree/67ee410e4e9e82a77fe0427899133dc07df99523)：
[公开头文件](https://github.com/libyal/libodraw/blob/67ee410e4e9e82a77fe0427899133dc07df99523/include/libodraw.h.in)、
[CUE 解析](https://github.com/libyal/libodraw/blob/67ee410e4e9e82a77fe0427899133dc07df99523/libodraw/libodraw_cue_parser.y)、
[轨道读取](https://github.com/libyal/libodraw/blob/67ee410e4e9e82a77fe0427899133dc07df99523/libodraw/libodraw_handle.c)、
[扇区转换](https://github.com/libyal/libodraw/blob/67ee410e4e9e82a77fe0427899133dc07df99523/libodraw/libodraw_io_handle.c)和
[RAW 布局说明](https://github.com/libyal/libodraw/blob/67ee410e4e9e82a77fe0427899133dc07df99523/documentation/Optical%20disc%20RAW%20format.asciidoc)。

上游是光盘容器库，不是 ISO / UDF 文件系统库。轨道枚举值、CUE 语法、实际扇区转换和
文件系统浏览是不同能力。上游 README 将多线程列为计划功能，写访问标记为保留，
`handle_set_bytes_per_sector` 在 `#ifdef TODO` 中，不能计作已实现能力。

| 能力 | libodraw 基准 | LovelyCarve |
| --- | --- | --- |
| ISO / BIN | 光盘数据文件读取 | ISO、单轨 Mode 1 / XA Form 1 2352 BIN 可直接添加，内容识别 |
| BIN/CUE、ISO/CUE | CUE、多个数据文件、轨道/索引 | 直接添加 `.cue`；一个或多个 BINARY 文件，每个文件可有多轨 |
| 轨道类型 | 声明 AUDIO、CDG、Mode 1/2、CD-I；转换另有限制 | 识别上述所有模式；文件系统投影范围见下表，其他轨保留原始范围 |
| 多轨与混合模式 | 轨道表与扇区转换 | 独立轨道节点；音频/未知轨不会被磁盘分区探针误识别 |
| INDEX / MSF | INDEX 及轨道位置 | 保留 00–99 索引，01 必须存在；校验数字、时分帧、单调性和边界 |
| PREGAP / POSTGAP | 语法接受，对应动作仅输出调试信息 | 保留声明，不合成未存储的源字节 |
| SESSION、LEAD-OUT、RUN-OUT | 专用 REM 规则及几何 | 保留会话、lead-out 与每轨 run-out，结束标记限制文件系统范围 |
| 顺序、随机、跨文件读取 | 共享当前位置的 handle API | 无共享源游标的 positioned I/O；smraw 并发读取与有界句柄缓存 |
| 源字节与 2048 数据 | 默认逻辑扇区视图，可转换物理布局 | 源保留原始字节，文件系统独立投影；不合成头、ECC 或缺失音频 |
| 源完整性 | 短读返回错误 | 短读/截断失败；CUE 与全部成员进入稳定性检查和证据指纹 |
| 编码和路径 | 窄/宽路径、可配置 codepage，默认 Windows-1252 | ASCII / UTF-8 CUE（可含 BOM）、Unicode/带空格路径，相对 CUE 所在目录解析 |
| 非 BINARY FILE | 语法识别 MOTOROLA、WAVE、AIFF、MP3；声明不等于音频解码 | 明确拒绝，不猜测音频容器头或重采样后的字节位置 |
| TOC 方言 | 另有 DATAFILE 等语法 | 不支持 TOC / DATAFILE，未知非 REM 指令失败 |
| EDC / ECC | 文档描述字段，不代表完整纠错实现 | 不校验/修复；校验头、地址、XA 子头及 ISO 几何 |
| 文件系统及取证操作 | 不在容器库职责内；工具提供信息/挂载 | CDFS 文件树、Joliet/Rock Ridge、预览、哈希、索引和导出，桌面/CLI 共用 |

### 轨道与文件系统投影

| CUE 模式 | 物理扇区 | 文件系统视图 |
| --- | --- | --- |
| MODE1/2048、MODE2/2048 | 2048 | cooked ISO 9660 |
| MODE1/2352、MODE1_RAW | 2352 | 偏移 16 的 2048 字节 |
| MODE2/2352、MODE2_RAW | 2352 | XA Form 1，偏移 24 的 2048 字节 |
| MODE2/2336、CDI/2336 | 2336 | 仅有效 XA Form 1 子头和 ISO 描述符的内容，偏移 8 的 2048 字节 |
| AUDIO | 2352 | 原始轨道字节，不解码为音频文件 |
| CDG | 2448 | 原始轨道字节，不解码子通道 |
| MODE2/2324、CDI/2352 | 2324 / 2352 | 原始轨道字节，不伪装为 2048 文件系统 |

XA 检查两份子头相同、通道小于 32、Form 位为 0。Form 2 的 2324 字节不能截尾后
冒充 Form 1。访问到坏同步头、非法 BCD、非连续 MSF、不同模式、子头冲突或 Form 2
时明确失败，不补零掩盖错误。2352 的模式字节目前只接受普通数据块的 1 / 2。

## 打开、几何与取证语义

单独 BIN 保留 2352 字节扇区几何，分区覆盖整个源文件。CUE 的 `sector_size` 为 1，
表示混合物理宽度下的**字节寻址单位**；每轨宽度在 `optical_disc.tracks[].sector_bytes`。
桌面位置按字节显示，详细信息显示会话/轨道/文件数，轨道标题含会话、轨号和模式。

CUE 的 `size_bytes`、十六进制、整镜像哈希和 RAW 导出是 FILE 顺序的完整原始文件拼接，
包括已存储的 INDEX 00、run-out、lead-out 和填充。CUE 文本不混入数据流，
`on_disk_bytes` 包含 CUE 与全部数据文件。RAW 导出不生成配套 CUE，拼接结果可能有
不同物理宽度；重新解释时必须保留原始 CUE 和文件边界记录。

既有 `image_load` / `image_info` / `image_list` 返回 `ImageInfo.optical_disc`，
CLI 的 `images open` / `images get` 可读取：

CLI 沿用统一 camelCase 输出，字段为 `opticalDisc`、`sectorBytes`、`sourceOffset` 等；
以下字段名为 Rust / IPC 的 snake_case。

- `files`：FILE 顺序及实际来源路径。
- `tracks`：轨号、会话、模式、文件索引、物理宽度、全部 INDEX、声明的 pregap/postgap、
  run-out，以及拼接源中的 `source_offset` / `source_length`。
- `lead_outs`：会话、文件索引及文件内扇区位置。

INDEX 是所属 FILE 内的 MM:SS:FF，不是拼接字节偏移，也不含声明但未存储的间隙。
每轨从 INDEX 01 开始，到下一轨 INDEX 00（若无则 01）、run-out、lead-out 或
文件尾中最早的有效边界。重复轨号/索引、逆序、空轨、缺 INDEX 01、缺文件、重复文件、
截断成员和超界标记拒绝打开，不回退成“把 CUE 文本当 RAW”。

ISO 大小端几何、2048 逻辑块大小和根目录范围必须有效。文件读以主卷描述符声明的
卷长为界，不把轨道尾部填充当文件。完整/流式/范围/导出使用相同投影，源故障保留
原始物理偏移；逻辑文件位置不伪装为连续原始字节范围。

## 限制与差异

- CUE 最多 1 MiB、99 个文件、99 条轨；每个 FILE 内轨道宽度必须相同，不同 FILE 可以不同。
  不猜测一个文件内物理宽度的转换位置。
- 多会话元数据和独立轨道可见，但不重建完整光盘绝对 LBA：只浏览轨内自包含的 ISO，
  不解析 lead-in TOC，不合并跨会话增量 ISO 或自动读取前一会话的 extents。
- 不自动查找同名 CUE。利用混合模式/轨道边界须选择 `.cue`，单独 BIN 仍按单轨识别。
- 不支持非 UTF-8 codepage 参数、MOTOROLA/WAVE/AIFF/MP3、CCD/TOC/MDS、CD-TEXT、
  子通道/音频解码、刻录及 EDC/ECC 纠错。TITLE/PERFORMER/FLAGS 等接受但不展示；
  CDTEXTFILE 不自动打开。普通未知 REM 忽略，SESSION/LEAD-OUT/RUN-OUT 严格校验。
- 裸 UDF 不作为 ISO 打开；CD-I 枚举不代表 CD-I 文件系统。ISO 多 extent、交错文件等
  仍受 `cdfs` 既有能力限制。
- C ABI 的缓冲区分配、错误打印、FILE pool 和可写元数据构造器用 Rust 生命周期、
  anyhow、tracing、smraw 和不可变来源替代。没有全局光盘缓存，卸载随源释放。

<details>
<summary>全部 56 个公开声明的 API 映射（含条件编译声明）</summary>

下表省略 `libodraw_` 前缀；TODO/保留声明不算可用功能。

| 上游声明 | 数量 | 对应或差异 |
| --- | ---: | --- |
| `get_version` | 1 | LovelyCarve 自身版本，无 libodraw 运行时 |
| `get_access_flags_read`, `get_access_flags_read_write`, `get_access_flags_write` | 3 | 仅只读；上游写标记也为保留值 |
| `get_codepage`, `set_codepage`, `handle_get_ascii_codepage`, `handle_set_ascii_codepage` | 4 | 不提供；严格 ASCII/UTF-8，其他编码报错 |
| `notify_set_verbose`, `notify_set_stream`, `notify_stream_open`, `notify_stream_close` | 4 | tracing / 宿主日志 |
| `error_free`, `error_fprint`, `error_sprint`, `error_backtrace_fprint`, `error_backtrace_sprint` | 5 | anyhow 错误链及 Rust 自动释放 |
| `handle_initialize`, `handle_free`, `handle_close` | 3 | CueImage / DiskImage 所有权及 Drop |
| `handle_signal_abort` | 1 | 打开、逐行解析、逐轨探测的 CancellationToken；长任务沿用任务取消 |
| `handle_open`, `handle_open_wide`, `handle_open_handle_io_handle` | 3 | DiskImage 与 Unicode Path；不提供 libbfio 入口 |
| `handle_open_data_files`, `handle_open_data_files_file_io_pool` | 2 | smraw::RawImage::open_segments，有界句柄池 |
| `handle_read_buffer`, `handle_read_buffer_at_offset`, `handle_seek_offset`, `handle_get_offset` | 4 | read_into/read_range、CDFS Read/Seek；无共享源游标 |
| `handle_set_maximum_number_of_open_handles` | 1 | smraw 默认缓存 64 个空闲句柄；宿主不新增配置 |
| `handle_get_number_of_data_files`, `handle_get_data_file` | 2 | optical_disc.files |
| `handle_append_data_file`, `handle_append_data_file_wide` | 2 | 打开时构建，不提供运行中证据映射修改器 |
| `handle_get_media_size`, `handle_get_bytes_per_sector`, `handle_get_number_of_sectors` | 3 | size_bytes、字节寻址单位和每轨物理宽度/长度，不伪造统一扇区数 |
| `handle_set_bytes_per_sector` | 1 | 上游 `#ifdef TODO`；不提供 |
| `handle_get_number_of_sessions`, `handle_get_session`, `handle_append_session` | 3 | tracks[].session 可枚举，不合成跨会话盘、不提供修改器 |
| `handle_get_number_of_lead_outs`, `handle_get_lead_out`, `handle_append_lead_out` | 3 | optical_disc.lead_outs，不提供修改器 |
| `handle_get_number_of_tracks`, `handle_get_track`, `handle_append_track` | 3 | optical_disc.tracks 和 Partition 节点，不提供修改器 |
| `data_file_free` | 1 | Rust 所有权 |
| `data_file_get_filename_size`, `data_file_get_filename`, `data_file_get_filename_size_wide`, `data_file_get_filename_wide` | 4 | Unicode Path / files，不要求调用者分配缓冲区 |
| `data_file_set_filename`, `data_file_set_filename_wide`, `data_file_get_type` | 3 | 打开时固定 BINARY FILE 路径，非 BINARY 拒绝 |

</details>

## 验证

```text
just test raw_cd --lib
just test cue --lib
just verify-rust
just check-examples
just verify-fe
just test-cli
```

夹具逐字段构造 ISO + Joliet、Mode 1、2336/2352 XA Form 1、Unicode/空格路径、
多文件、混合音频/数据、多会话、INDEX 00/01/02、间隙和结束标记，核对源字节与跨文件读、
文件完整/范围/流式/导出一致性及全部来源稳定性。
`scripts/cli_optical_tests.py` 独立构造另一套 ISO/CD 夹具，由 `just test-cli` 调用，
通过 JSONL 公开命令核对四种布局的目录、跨扇区读、文件/源 SHA-256 与文件/RAW 导出。
损坏覆盖截断、几何和 extent 越界、非法 BCD、XA 子头冲突/非法通道/Form 2、
CUE 逆序/重复/缺失/超界、模式不一致、缺文件及取消。

这些是源码/API 对照与确定性夹具，不代表已运行 libodraw 二进制 oracle；没有已提交的
真实多会话、CDG、CD-I 和非 BINARY 音频语料，不声称这些格式已完成真实互操作验证。
可选 MODE1/2352 外部回归：设置 `LOVELYCARVE_RAW_CD_IMAGE=<镜像路径>`，运行
`just test raw_cd_mode1_real_image --lib -- --ignored`，要求卷根有 `data` 目录及非空文件。
不提交真实检材、私有目录清单或哈希。
