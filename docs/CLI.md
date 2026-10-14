# LovelyCarve CLI

`lovelycarve-cli` 是独立的无窗口取证终端，不连接、不启动 LovelyCarve 桌面界面。
在一个会话中打开检材、选择分区、浏览目录、执行分析。
解析器、案件库、插件和任务调度直接复用 LovelyCarve 后端。

常用章节：[构建与运行](#构建与运行)、[交互浏览](#交互浏览)、[全功能命令](#全功能命令)、[JSONL 自动化](#jsonl-自动化会话)、[生命周期与边界](#生命周期与边界)。iOS 解锁与检查命令见 [iOS 备份](IOS_BACKUPS.md#cli)。

## 构建与运行

```powershell
just run-cli --help
just run-cli
just build-cli
```

`just build-cli` 构建 `src-tauri/target/release/lovelycarve-cli.exe`，默认关闭二维码和 OCR，
无需 OpenCV 或 CUDA。
需要 QR/OCR 时使用 `just build-cli-full`：Windows 配置 Visual Studio/OpenCV/OpenSSL，
启用带 CPU 回退的 CUDA OCR；Unix 完整配方使用 CPU OCR。轻量构建中，
相关命令仍列出，但会返回后端的功能未启用错误。文件系统、搜索、案件和插件不受此限制。
`lovelycarve-cli capabilities --json` 可检查当前构建的功能开关。
Windows CLI 构建配方单独写入 `asInvoker` 清单，普通镜像分析不会弹出 UAC，管道可直接使用；
需要物理盘、Dokan 或系统级权限时，从管理员终端运行。桌面程序保留原有提权清单。

`just pack-cli` 构建并打包轻量 Windows CLI，包含运行库、`lc-plugin-host`、插件及本手册；
`just pack-cli-full` 另启用 QR/OCR 并携带 OpenCV 与模型。`just repack-cli` 根据现有二进制的
实际功能打包，轻量产物省去 OpenCV DLL 与 QR/OCR 模型。
开发配方在 `src-tauri` 目录执行，建议向 `just run-cli` 传绝对检材路径；直接运行 exe 时
相对路径以当前工作目录为准。执行单个分析命令不需要 Vite、浏览器或 WebView 窗口。
CLI 目前仍链接 Tauri 的原生运行时；Linux 构建保留其 GTK/系统依赖，并非无依赖静态程序。
插件是原生动态库，从程序旁的 `plugins/` 发现；默认隔离方式的插件由程序旁的
`lc-plugin-host` 加载，不需要额外的解释器或运行时。

运行中添加插件目录后，可执行 `plugin-rescan --json` 手动检查；`plugin-scan-report --json` 查看最近诊断，`plugin-list --json` 读取已经发布的列表。JSONL 会话中的命令名分别为 `plugin_rescan`、`plugin_scan_report`、`plugin_list`。查询列表不会扫描磁盘；库更新需要重启。字段说明见 [插件配置参考](PLUGIN_MANIFEST.md)。

## 交互浏览

```text
lovelycarve-cli
lovelycarve:/> open "D:\evidence\disk.vmdk"
lovelycarve:/> info
lovelycarve:/> list-partitions
lovelycarve:/> select-partition 0
lovelycarve:/...> ls
lovelycarve:/...> cd Users
lovelycarve:/.../Users> pwd
lovelycarve:/.../Users> tree --depth 3 --limit 10000
lovelycarve:/.../Users> find . --name report
lovelycarve:/.../Users> stat report.txt
lovelycarve:/.../Users> cat report.txt --length 4096
lovelycarve:/.../Users> hex report.txt --offset 0 --length 128
lovelycarve:/.../Users> strings report.txt --encoding utf-8 --min-length 4
lovelycarve:/.../Users> file report.txt
lovelycarve:/.../Users> sha256sum report.txt
lovelycarve:/.../Users> export report.txt "D:\output\report.txt"
lovelycarve:/.../Users> exit
```

- 镜像内路径使用 `/`，支持相对路径、绝对路径、`.` 和 `..`。绝对路径从检材合成根开始，
  包含 `ls /` 显示的分区/卷名称。目录检材和归档同样先显示合成根，用 `cd "根名称"` 进入。
- `ls` 输出 NODE ID；同名节点、删除条目和特殊流可用后端命令的 `--node-id` 精确指定。
  不自动跟随符号链接。
- `ls/tree/find` 默认最多检查 10000 个节点；`--limit` 上限 100000，`--depth` 上限 128。
  达到遍历/深度边界会显示 `truncated`，目录读取失败会报错，不能视为完整列表。
  `find --name` 是忽略大小写的包含匹配。路径解析和 `ls/tree/find` 复用目录分页接口，
  优先查询案件卷索引或无案件会话索引；未被索引覆盖的目录沿用后端的快照／实时回退。
  离线检材可浏览已索引目录，未索引目录会报错。
- `grep <关键词> [路径]` 复用全局搜索，默认搜索当前目录子树，文件路径限制到单文件，
  `/` 搜索当前检材全部索引卷；需要先创建／打开案件并等待卷索引构建完成（`wait`）。
  默认区分大小写并包含删除项；`-i` 忽略大小写，`-E` 使用正则，`--exclude-deleted` 排除删除项，
  `--codepage UTF-8` 指定编码（可重复，省略搜索全部支持编码）。例如 `grep -i password /`、
  `grep -E 'pass(word)?' report.txt --codepage UTF-8`。
  命中输出路径、文件内字节偏移、编码、上下文和 NODE ID；JSON 返回 `summary/hits/total/truncated`。
  `--limit` 只限制显示条数（默认 10000，上限 100000），引擎的命中上限、读取错误、取消状态见摘要。
  搜索历史与完整已落库命中保存在案件中，可用 `global-search-hits-page` 继续分页；
  `--search-id` 可指定任务 ID，省略自动生成，取消沿用 `task-cancel`。
- `cat/hex` 是有界预览，默认 4096 字节，最多 16 MiB，返回大小、偏移和截断标记；
  `cat` 用 UTF-8 损失替换显示非 UTF-8 内容，`hex` 在终端显示偏移、十六进制和 ASCII 列。
  完整文件使用流式 `export`。
- `cd/pwd/tree` 共享当前检材的目录上下文；目录解析和验证优先走索引，支持离线使用。
  `stat <路径>` 返回索引中的节点属性（名称、类型、大小、inode、时间戳、删除状态与链接目标），
  附带 `node_id/path/index_only`；未覆盖节点沿用目录接口回退。
  字节相关属性（MIME、熵、数据运行段）需要原检材，可通过完整后端命令 `fs-stat` 查询。
- `file <路径>` 调用后端 Magika 内容识别，返回类型标签、MIME、描述和置信度；需要原检材。
- `strings <路径>` 流式提取可打印字符串，默认 ASCII、至少 4 个字符；`-n/--min-length` 调整最短长度，
  `-e/--encoding` 支持 `ascii/utf-8/utf-16le/utf-16be`，显示文件内字节偏移。
  `--offset` 指定起点，`--length` 指定扫描范围（省略到文件末尾），`--limit` 限制记录数
  （默认 10000，上限 100000）。单条文本最多 64 KiB，总文本最多 16 MiB，超限明确标记 `truncated`。
  JSON 返回 `strings/encoding/offset/size/bytes_read/truncated/task_id/read_faults`；可通过 `task-cancel` 取消。
- `export` 和 `call --output` 用临时文件加不覆盖提交，已有目标不会被改写。
  低层 `fs-export`、`fs-export-dir` 等保持桌面后端自己的覆盖和命名语义。
- `hash` 同时计算 MD5/SHA-1/SHA-256/SM3；另有 `md5sum`、`sha1sum`、`sha256sum`。
- `use <image-id>` 切换已加载检材；`image-list` 列出检材；`tasks` / `wait` 查看或等待任务。
- 引号只负责参数分组，Windows 反斜杠保持原样。不执行系统 shell、变量展开或管道。
  单引号适合包住 JSON；复杂参数推荐使用 `call --args-file`。

## 全功能命令

桌面注册的所有命令均生成 CLI 入口。每个命令同时接受连字符和原始下划线名称，
如 `fs-children-page` / `fs_children_page`。新增桌面命令会自动进入 CLI；不支持的注入参数
会让构建失败，避免静默漏接。可通过下面的命令获取实际构建的完整清单和参数类型：

```powershell
lovelycarve-cli commands --json
lovelycarve-cli commands fs-export-dir
lovelycarve-cli fs-export-dir --help
lovelycarve-cli --image "D:\evidence\disk.img" fs-children --parent-id ""
lovelycarve-cli --image "D:\evidence\disk.img" image-hash-compute --algorithms '["sha256"]' --task-id hash-1
```

| 功能 | 主要命令入口 |
| --- | --- |
| 镜像、物理磁盘、RAID、RAW 导出 | `image-*`、`physical-disks-list`、`raid-*` |
| NTFS 卷影副本枚举、只读历史会话 | `shadow-copy-list`、`shadow-copy-open`；父/子镜像保持同一会话，见 [VSS](formats/VSS.md) |
| 文件系统、文件索引、目录分页、定位、Docker | `fs-*`、`filedb-*` |
| 签名雕刻、状态、暂停/恢复、结果、恢复报告 | `carve-*` |
| 文件/目录/ZIP 导出、命名数据流 | `fs-export*`、`fs-stream-*` |
| 搜索、历史、结果分页与导出 | `global-search-*` |
| 时间线、计数、范围、结果与导出 | `timeline-*`、`fs-timeline-export` |
| BitLocker 状态、解锁、内存扫描、解密导出 | `bitlocker-*` |
| 只读挂载、卸载、SMB 共享 | `fs-mount-*`、`fs-unmount-*`、`fs-share-mount` |
| 哈希、文件类型、标签 | `image-hash-compute`、`fs-digest*`、`fs-tags-*` |
| SQLite、Realm、ZIP、CSV、二进制等预览 | `fs-*-preview`、`fs-*-table-rows` |
| NTFS/EXT4 日志、OS/平台检测、原盘符 | `fs-usn-*`、`fs-ext4-*`、`os-detect`、`autodetect-evidence`、`win-drive-letters` |
| 二维码、OCR | `qr-scan`、`ocr-scan`（完整构建） |
| 插件、任务调度、分析结果 | `plugin-*`、`sched-*`、`analysis-*` |
| 案件、证据、设置、快照 | `case-*`、`snapshot-*` |
| 通用任务、能力与路径查询 | `task-*`、`module-summary`、`system-path-info` |

常用别名：`carve` → `carve-start`，`carve-list` → `carve-results-page`，
`carve-export` → `carve-recover`，`rawsearch` → `global-search-start`，
`timeline` → `timeline-start`，`mount` / `unmount` → `fs-mount-readonly` / `fs-unmount-readonly`。
这些别名沿用对应命令的参数，不模拟其他工具的未实现算法或命令选项。

字符串参数直接传文本；数字、布尔、数组、枚举和结构体参数传 JSON，枚举需包含 JSON 引号。
顶层 JSON 参数名沿用 IPC 的 camelCase；嵌套 options 使用原类型的 serde 命名（很多为 snake_case）。
`imageId` 可省略以使用当前检材。其他必填参数不会被猜测，未知参数会报错。

```text
case-create --dir "D:\cases\demo" --name demo
open "D:\evidence\disk.img"
rawsearch --search-id search-1 --options '{"keys":["password"],"codepages":["UTF-8"]}'
timeline
carve --options '{"format_ids":["jpeg","png","pdf"],"scope":"whole"}'
wait
carve-scan-list
```

搜索和时间线持久化到活动案件，先创建/打开案件；插件函数由实际安装的插件提供。
挂载应在交互/JSONL 会话中使用，保持进程存活；退出会释放挂载和共享。
平台限制沿用后端（例如 Dokan/SMB 挂载仅 Windows）。`theme-set` 仅验证主题值；
`reveal-path` 是显式打开系统文件管理器的辅助命令，不属于无界面取证分析流程。

## JSONL 自动化会话

```powershell
Get-Content .\workflow.jsonl | lovelycarve-cli --events session
```

`workflow.jsonl` 示例（路径按实际机器修改）：

```jsonl
{"id":"case","command":"case_create","args":{"dir":"D:/cases/demo","name":"demo"}}
{"id":"image","command":"image_load","args":{"path":"D:/evidence/disk.img"}}
{"id":"roots","command":"fs_children","args":{"imageId":{"$ref":"image#/id"},"parentId":""}}
{"id":"scan","command":"carve_start","args":{"options":{"format_ids":["png","pdf"],"scope":"whole"}}}
{"id":"done","command":"wait"}
{"id":"status","command":"carve_scan_status","args":{"scanId":{"$ref":"scan#/scan_id"}}}
{"id":"recover","command":"carve_recover","args":{"scanId":{"$ref":"scan#/scan_id"},"options":{"destination":"D:/output/carved","task_id":"recover-1"}}}
```

每条响应为 `{"id":"...","result":...}` 或 `{"id":"...","error":...}`；响应立即 flush。
浏览和文件快捷命令也可在 JSONL 中直接调用，包括 `cd/pwd/ls/tree/find/stat/hex/strings/file/grep`。
例如 `{"id":"props","command":"stat","args":{"path":"/卷名称/report.txt"}}`；参数采用 CLI 的长参数名，
可用下划线代替连字符（如 `min_length/ignore_case`），重复编码参数用数组 `"codepage":["UTF-8","UTF-16LE"]`。
`cd` 修改后续请求的当前目录，`pwd` 返回路径字符串；无输出操作返回 `null`，未知参数和类型错误仍报错。
请求 ID 使用非空唯一字符串；`{"$ref":"id#/json/pointer"}` 引用先前成功结果。
二进制返回 `{"encoding":"hex","data":"..."}`；请求加 `"output":"D:/output/data.bin"`
可直接保存原始二进制。请求/参数行上限 16 MiB；单会话最多保存 10000 个结果引用。
默认 `session --jobs 1` 顺序执行；`session --jobs N`（简写 `-j N`，范围 1–64）允许独立查询并发执行。
目录浏览、文件读取／哈希、内容搜索、结果分页和任务状态等查询支持并发；
`commands --json` 中每个后端命令的 `parallel` 字段给出本构建的实际支持情况。
未标为并发的命令是屏障：先完成此前查询，再执行该命令，然后继续读取后续请求。
因此加载／卸载检材、案件切换、`use/cd/select-partition` 和 `wait` 保持有序；
并发查询固定使用派发时的当前检材和目录，不会改写会话上下文。
`$ref` 可引用尚在执行的先前请求，使用前会等待其完成；失败、未知 ID 和不存在的指针明确报错。
等待依赖期间不继续派发后续请求，独立查询宜放在依赖请求之前。
并发响应按完成时间输出，每条立即 flush，客户端必须按 `id` 关联，不能按行号配对。
stdin 保持打开时也会持续返回完成结果；EOF 先等待所有已派发请求，再等待后台任务。
单条错误后继续处理后续 JSONL，请求全部结束后仍以非零退出。

### AI agent 调用

推荐先调用 `capabilities --json` 和 `commands --json` 发现协议、并发上限、命令名和参数，
再启动一个长期运行的 JSONL 子进程；无需解析终端表格或中文提示。

```text
lovelycarve-cli --image "D:\evidence\disk.E01" session --jobs 4
```

向该进程 stdin 连续写入以下请求（每条必须以换行结束），可在收到结果前提交多条：

```jsonl
{"id":"info","command":"image_info"}
{"id":"roots","command":"fs_children","args":{"parentId":""}}
{"id":"tasks","command":"task_list"}
{"id":"barrier","command":"wait"}
```

stdout 只包含逐行 JSON 响应，无进度条、提示符或额外完成消息；无需加 `--json`。
成功为 `{"id":"info","result":...}`；失败为
`{"id":"info","error":{"code":"invalidInput","message":"...","details":...}}`，
`details` 仅在有后端详情时出现。能解析出字符串 ID 的无效请求保留该 ID，
不可解析的 JSON／输入读取错误使用 `"id":null`。
`result` 保留各后端命令的原始 JSON 类型与字段，二进制使用明确的 `encoding/data` 包装。
协议能力位于 `capabilities` 的 `session` 对象；终端／单次命令需要显式 `--json`。
日志及可选 `--events` 事件写入 stderr，agent 应分别读取 stdout 和 stderr，避免混入响应。
退出码用于整个会话状态；每个请求是否成功仍以其 `result/error` 字段为准。

`just test-cli` 包含并发、屏障、结果引用、错误关联和保持 stdin 打开的进程级回归。
可设置 `LOVELYCARVE_CLI_TEST_IMAGE` 为外部镜像路径，再运行 `just test-cli` 或
`just smoke-cli <可执行文件>`，额外对比顺序／并发的目录和有界文件读取；该样本不加入仓库。

`wait` 和正常 EOF 等待当前进程内任务与活动插件调度任务结束。它只表示任务已不再活动，
业务结果应读取对应状态/结果接口（例如 `carve-scan-status`、`sched-task-list`），
不要把任务不在队列中解释成分析成功。暂停的雕刻请先恢复/取消再等候。
CLI 退出不保存内存雕刻记录；应在同一个会话中扫描、分页与恢复。

`--events` 将进度和生命周期事件写到 stderr；stdout 只承载结果。
`RUST_LOG=debug` 可开启后端诊断。Windows Ctrl+C 取消任务、释放案件/挂载并以 130 退出。
`call fs_read --args-file params.json --output file.bin` 支持单次低层调用；
`run commands.txt` 执行逐行终端命令，忽略空行和 `#` 注释，遇错停止。
命令成功退出 0，运行/协议失败退出 1，命令行语法错误退出 2。

## QCOW 镜像

QCOW v1/v2/v3 的压缩簇和差分父链自动读取；legacy AES 镜像可在 `image_load` 中
传递 `qcowPassword` 或 `qcowKeyHex`（16 字节 key，二选一），例如：

```jsonl
{"id":"qcow","command":"image_load","args":{"path":"D:/evidence/disk.qcow2","qcowPassword":"qcow-test"}}
```

以上密码来自公开测试夹具。真实凭据宜经 stdin 传递；不写入案件、任务描述或日志。
legacy AES 没有密钥验证器，成功打开不证明密码正确；QCOW 内嵌 LUKS、Zstd、
extended L2、external data file 和历史快照切换不支持。完整对照见 [QCOW](formats/QCOW.md)。

## 生命周期与边界

CLI 与 GUI 复用后端初始化，但清空 Tauri 配置的窗口列表，绝不创建隐藏 WebView。
预览临时文件的租约归 CLI 会话所有，显式 release 或退出后删除；需要长期保留时使用导出。
案件锁、源读取审计、文件格式限制和任务资源预算与桌面版一致。请用独立案件避免与 GUI
同时占用同一案件；检材以只读方式打开。

二维码 / OCR 仍需要发布包内相应的 DLL、模型及平台环境。这里的“全功能”指
LovelyCarve 已注册的全部后端命令；交互 shell 不提供命令历史、Tab 补全或管道。

## EWF 后端

E01/Ex01 使用 [ewf-forensic](https://github.com/SecurityRonin/ewf-forensic) 仓库中的
`ewf` 0.4.10 纯 Rust 读取器，与桌面和挂载入口共用，不需要 `libewf.dll`。
支持多分卷、随机读取、zlib、Ex01 bzip2/模式填充、存储哈希与采集元数据。
打开时检查分卷序号和块表覆盖，缺失分卷会报错；坏块通过原有读取故障机制报告。
加密的 EWF2 容器仍会明确返回不支持；BitLocker 卷解锁是独立功能。

```powershell
just run-cli open D:\evidence\disk.E01
just test-ewf
```

源码版本及本地兼容补丁见 `src-tauri/vendor/ewf/LOVELYCARVE.md`。

## VMDK 后端

VMDK 使用 [vmdk-forensic](https://github.com/SecurityRonin/vmdk-forensic) 中的
`vmdk-core` 0.8.4 纯 Rust 读取器，桌面、CLI 和挂载共用，不需要 `libvmdk.dll`。
支持 monolithicFlat、monolithicSparse、streamOptimized 压缩镜像、分卷 flat/sparse，
以及读取器支持的 COWD / seSparse 格式。打开描述符时需保留相邻分卷，缺失文件会报错；
直接打开 `*-flat.vmdk` 会优先寻找描述符，无描述符时按原始磁盘读取。

单文件稀疏快照及 seSparse 描述符按 `parentFileNameHint` 读取父盘，检查 CID、容量和循环；
所有父盘、描述符和分卷都纳入源变化检测。父盘需位于当前镜像目录或其子目录。
多分卷差异快照（含 vmfsSparse/vmfsThin/custom 差异层）暂不支持，须先合并为完整镜像；
缺少父盘不会被当成空洞补零。损坏块仍通过原有读取故障机制报告。
LDPlayer 备份内部的内存 VMDK 保留已有纯 Rust 读取路径。

```powershell
just run-cli open D:\evidence\disk.vmdk
just test-vmdk
```

源码版本、补丁和测试素材见 `src-tauri/vendor/vmdk/LOVELYCARVE.md`。

## BitLocker 后端

[bitlocker-forensic](https://github.com/SecurityRonin/bitlocker-forensic) 的 `bitlocker-core`
0.3.5 负责密码与 48 位恢复密码解锁，不需要 `libbde.dll`。VMK 内存扫描、
从 VMK 恢复恢复密码、解锁及解密导出都可以在 CLI 中使用。直接读取 RAW / EWF / VMDK 等镜像内的
分区，无需将整个加密卷转储到临时目录；源镜像关闭后读取失败，不延长检材句柄生命周期。

解锁进度仍使用原事件：密钥派生前为 0，认证完成后为卷大小；它不表示已复制的数据。
取消在派生前后检查，派生进行中可能稍后才响应。错误密码/密钥认证失败和截断密文均报错。

支持六种 AES-CBC / Elephant Diffuser / XTS 方法（包括 0x8001），按 BPB 扇区大小解密；
另支持明文密钥、BEK、直接 VMK / FVEK，以及 Vista 与 used-space-only（EOW）卷。
完整 libbde 对照、真实样本验证范围和限制见 [BITLOCKER](formats/BITLOCKER.md)。

`bitlocker_metadata` 无需凭据，返回算法、卷 GUID、原始 FILETIME（十进制字符串）、卷描述和
保护器 ID/类型，不返回密钥 payload。`bitlocker_unlock` 只选一种参数：`password`、
`recoveryPassword`、`startupKeyPath` 或 `fvekHex`（Diffuser 时加 `tweakHex`）；全不传则尝试卷内
明文密钥。原始 FVEK 没有认证标签，须核验解密结果。`bitlocker_unlock_with_vmk` 直接认证 FVEK，
卷没有恢复密码保护器时也可解锁；结果 `recovery_key` 在无可提取密码时为空字符串。
TPM/SID 不支持。导出不覆盖已有文件，遇到已审计的合成源字节会失败，可查询 `image_bad_regions`。

```powershell
just run-cli open D:\evidence\encrypted.E01
just test-bitlocker
```

在同一个 `session` 中调用 `image_load`、`bitlocker_unlock`（`partitionId` 与 `password`
或 `recoveryPassword`），之后可使用 `bitlocker_export_decrypted_image` 导出。
源码版本、本地补丁和公开样本范围见 `src-tauri/vendor/bitlocker/LOVELYCARVE.md`。

## VHD / VHDX 后端

VHD 使用 [vhd-forensic](https://github.com/SecurityRonin/vhd-forensic) 的 `vhd-core` 0.1.4，
VHDX 使用 [vhdx-forensic](https://github.com/SecurityRonin/vhdx-forensic) 的 `vhdx-core` 0.4.1。
桌面和 `lovelycarve-cli` 共用这些 Rust 读取器，运行和打包均无需 `libvhdi.dll`。

支持固定盘、动态盘、随机读取与 RAW 导出，按签名识别改名后的镜像。VHD 使用 footer 的
CurrentSize（当前容量），校验和错误会报错，不再通过 RAW 回退绕过。VHD 本地补丁只读取
footer、动态头和 BAT（分配表上限 64 MiB），不会为打开大镜像而读入整个文件；截断的已分配
数据块会报错。VHDX 使用按需读取，脏日志在内存覆盖层中重放，不修改检材。

VHD 上游不支持差分盘；VHDX 上游虽有显式父盘 API，现有命令尚无父盘绑定参数，
所以差分 VHD/VHDX 均明确拒绝，不会将缺失父盘的数据伪装为零。

```powershell
just run-cli open D:\evidence\disk.vhdx
just test-vhd
just test-vhdx
just test-cli
```

回归样本来源、固定的上游提交和本地改动见两个 vendor 目录的 `LOVELYCARVE.md`。
全盘 SHA-256 参考值来自 libvhdi 对公开样本的独立读取，测试本身不依赖该 DLL。

## LUKS / VeraCrypt 解锁

`encrypted-volume-unlock` / `encrypted_volume_unlock` 把已加载检材中的加密区域注册为
会话内只读镜像，返回 `image.id` 和 `image.path`（`decrypted://…`）。然后可使用现有
文件系统、搜索、哈希、雕刻、挂载及 RAW 导出命令。桌面镜像信息页「卷与分区」卡片的
「解锁加密容器」与 LUKS 分区行的「解锁」也提供相同入口。`.hc`、`.tc`、`.luks` 文件可直接加载。
桌面提交后立即关闭密码弹窗，在「任务中心」显示后台解锁状态、取消与失败原因，
成功后在原镜像分区内浏览并自动补建索引，不切换或新增镜像标签；切换页面不取消任务。
从分区行进入时固定该分区。CLI 命令仍等待解锁结果，便于后续 JSONL 引用。
桌面使用 `options.target: "sourcePartition"`，返回原 `image.id` 与 `volume_key`，文件节点沿用
`part:N`；原镜像哈希仍读取源密文，RAW 导出省略 `mode` 或使用 `"source"` 时也读取源字节。
桌面「导出原始镜像」使用 `image_export_raw` 的 `mode: "unlockedPartitions"`：以导出开始时的
已解锁状态投影所有 LUKS / VeraCrypt / BitLocker 分区，明文从原分区起始位置写入，
较短明文后的分区尾部补零；分区表、其他分区及整盘大小保留。导出开始后的解锁不改变本次输出，
解密会话关闭、短读或越界时报错。目标文件必须不存在，避免覆盖源镜像或已有导出。
省略 `target` 或使用 `"newImage"` 时保留
下方 CLI 示例的独立解密镜像行为，便于显式导出明文。原分区接入要求解密内容为单个完整卷。

JSONL 会话示例（`lovelycarve-cli session`，逐行从标准输入发送）：

```jsonl
{"id":"source","command":"image_load","args":{"path":"D:/evidence/container.hc"}}
{"id":"unlock","command":"encrypted_volume_unlock","args":{"imageId":{"$ref":"source#/id"},"options":{"scheme":"veracrypt","password":"example-password","pim":0,"hidden":false},"taskId":"unlock-1"}}
{"id":"plain","command":"image_load","args":{"path":{"$ref":"unlock#/image/path"}}}
{"id":"roots","command":"fs_children","args":{"parentId":""}}
{"id":"export","command":"image_export_raw","args":{"destination":"D:/exports/plain.raw","taskId":"export-1","readFaultPolicy":"strict"}}
```

LUKS 使用 `"scheme":"luks"`，不传 `pim`/`hidden`。密码之外，也可只传 `options.masterKeyHex`（已恢复卷主密钥的十六进制，不与密码同时传）；LUKS1/2 均先核验密钥摘要。算法与能力对照见 [LUKS](formats/LUKS.md)。`options.partitionId` 选择分区；
也可指定相对源镜像的字节 `offset` 和 `length`（省略长度时到源末尾），两种区域选择
不能混用。默认整个源镜像。VeraCrypt 没有明文签名，需要用户明确选择格式和隐藏卷。
密码按 UTF-8 字节传入；命令本身不保存密码或导出主密钥，敏感输入宜走标准输入，
避免把真实密码放进命令行参数或保存的脚本。

LUKS1 与 LUKS2（单个动态数据段，PBKDF2 / Argon2i / Argon2id，512 / 4096 字节加密扇区）支持的
密码算法、IV 方案与哈希见 [LUKS](formats/LUKS.md)。VeraCrypt 支持普通／隐藏卷（512 字节扇区）、
上游支持的 AES / Serpent / Twofish 及其级联、PIM 0–1000。分区识别使用 512 字节逻辑扇区。
暂不支持密钥文件、分离头部、备份头恢复、系统加密、LUKS2 完整性 / 重加密 / 多数据段 / 固定长度数据段，
也不声明完整的 TrueCrypt 兼容性。LUKS2 主头校验失败时报错，不自动尝试备份头；超过 KDF 资源预算同样报错。

`task_cancel` 可以取消解锁；KDF 当前执行阶段可能延迟响应。
`image_unload` 可锁定返回的解密镜像；卸载源镜像会递归移除其解密镜像。
解密镜像和凭据不会作为可自动恢复的案件检材保存，进程结束后需重新解锁。
读取按需解密；只有显式执行导出、预览或分析缓存操作才会产生相应的明文输出。

## FileVault 2 / CoreStorage 解锁

先用 `filevault_inspect` 读取逻辑卷目录和法证观察，再将选中的 `index` 传入
`encrypted_volume_unlock` 的 `logicalVolumeIndex`。区域选择沿用 `partitionId` 或
`offset` / `length`；两个命令应使用同一区域。下例的输入是独立 CoreStorage 分区镜像：

```jsonl
{"id":"source","command":"image_load","args":{"path":"D:/evidence/corestorage.raw"}}
{"id":"inventory","command":"filevault_inspect","args":{"options":{},"taskId":"fv-inventory"}}
{"id":"unlock","command":"encrypted_volume_unlock","args":{"options":{"scheme":"filevault","password":"example-password","logicalVolumeIndex":0,"target":"newImage"},"taskId":"fv-unlock"}}
{"id":"plain","command":"image_load","args":{"path":{"$ref":"unlock#/image/path"}}}
{"id":"files","command":"fs_children","args":{"parentId":"part:0"}}
```

`password` 同时接受个人恢复密码；也可改用 `masterKeyHex`（16 字节 VMK 或 32 字节
XTS 密钥对）。可选 `encryptedRootPlistPath` 指向本地 XML / EncryptedRoot.plist.wipekey，
应同时传给 inspect 和 unlock。单逻辑卷支持原分区投影；多逻辑卷必须分别使用
`newImage`，以隔离文件 ID 和索引。完整能力、上游 API 对照和不支持的布局见
[FileVault](formats/FILEVAULT.md)。APFS 原生加密使用另一种格式，当前不能由此解锁。
