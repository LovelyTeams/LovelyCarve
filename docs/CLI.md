# LovelyCarve CLI

`lovelycarve-cli` 是独立的取证命令行工具：不链接 Tauri、WebView 或任何桌面窗口运行时，
自行初始化后端状态，直接调用与桌面端相同的解析器、案件库、插件和任务调度。
命令统一为 `<集合>… <动词>` 形式，标准输出只放结果（JSON 或原始字节），提示与错误走标准错误，
用 `cli search`、`docs search` 与 `schema` 发现命令和文档，`--dry-run`、`--body`、`--force`
在所有命令上含义一致。

常用章节：[构建与运行](#构建与运行)、[命令结构](#命令结构)、[发现命令](#发现命令)、
[输出与退出码](#输出与退出码)、[参数](#参数)、[浏览检材](#浏览检材)、[交互 shell](#交互-shell)、
[JSONL 会话](#jsonl-会话)、[生命周期与边界](#生命周期与边界)。iOS 备份命令见 [iOS 备份](IOS_BACKUPS.md#cli)。

## 构建与运行

```powershell
just run-cli --help
just run-cli images open "D:\evidence\disk.E01"
just build-cli
```

`just build-cli` 构建 `src-tauri/target/release/lovelycarve-cli.exe`，默认关闭二维码和 OCR，
无需 OpenCV 或 CUDA。需要 QR/OCR 时使用 `just build-cli-full`：Windows 配置 Visual Studio/OpenCV/OpenSSL，
启用带 CPU 回退的 CUDA OCR；Unix 完整配方使用 CPU OCR。轻量构建中相关命令仍然列出，
但返回后端的功能未启用错误；文件系统、搜索、案件和插件不受影响。
`lovelycarve-cli system get` 输出版本、平台、`standalone`（未链接桌面运行时）、命令数、
QR/OCR/挂载功能开关和会话协议。

CLI 构建不启用 Cargo 的 `desktop` 特性，所以不包含 Tauri、WebView2 或 GTK 依赖，
Linux 上也不需要桌面库。Windows 构建在链接时嵌入 `asInvoker` 清单（`src-tauri/cli.manifest`），
普通镜像分析不会弹出 UAC，管道可直接使用；读取物理盘、Dokan 挂载或扫描内存时，从管理员终端运行。
案件登记与偏好放在与桌面端相同的用户数据目录（由 `tauri.conf.json` 的 identifier 决定），
两个程序看到同一份案件列表。

`just pack-cli` 构建并打包轻量 Windows CLI，包含运行库、`lc-plugin-host`、插件及本手册；
`just pack-cli-full` 另启用 QR/OCR 并携带 OpenCV 与模型。`just repack-cli` 按现有二进制
`system get` 报告的实际功能打包。开发配方在 `src-tauri` 目录执行，建议向 `just run-cli`
传绝对检材路径；直接运行 exe 时相对路径以当前工作目录为准。插件包从程序旁的 `plugins/` 与用户数据目录的
`plugins/` 发现，每个插件实例都在程序旁的 `lc-plugin-host` 进程中运行；`plugins create <插件包.lcpkg>` 安装插件包。

## 命令结构

```text
lovelycarve-cli [全局选项] <集合> [<子集合>…] <动词> [<主体>] [选项]
```

```powershell
lovelycarve-cli images open "D:\evidence\disk.E01"
lovelycarve-cli --image "D:\evidence\disk.E01" files tree list --path / --depth 2
lovelycarve-cli --image "D:\evidence\disk.E01" files read "/<卷名称>/Windows/System32/config/SAM" > SAM
lovelycarve-cli --image "D:\evidence\disk.E01" files hash "/<卷名称>/Users/alice/report.docx"
lovelycarve-cli --case "D:\cases\demo" cases get
```

集合是复数名词，最后一个词是动词。动词表固定：`get` 查看单个对象（不用 info/status/show），
`list` 列出（不用 ls/page/query，且不带位置参数），`create`、`update`（整体替换）、`edit`（部分修改）、
`delete`（不用 remove/drop），以及 `open`、`close`、`read`、`export`、`hash`、`identify`、`search`、
`scan`、`detect`、`lookup`、`unlock`、`build`、`audit`、`cancel`、`wait`，以及选择检材的 `context use` 等领域动作。
位置参数是动词作用的对象（检材路径、镜像内路径、分区序号、扫描 ID……），其余参数用 `--选项`。

| 集合 | 内容 |
| --- | --- |
| `images` | 检材：打开、查看、关闭、RAW 导出、哈希、坏区域审计、平台与操作系统识别、案件中的哈希／类型／标签 |
| `files` | 检材内的文件：按路径浏览、读取、搜索、哈希、识别、导出，以及 SQLite、Realm、ZIP、CSV、可执行文件、日志等结构解析 |
| `cases` | 案件：创建、打开、关闭、锁定、设置与案件检材 |
| `indexes` | 卷索引：构建、列出、删除与元数据查询 |
| `searches` | 全局搜索：运行、历史、命中分页与导出 |
| `timeline` | 时间线：构建、事件、计数与导出 |
| `carve` | 文件恢复：文件系统记录恢复与签名雕刻 |
| `bitlocker`、`encrypted-volumes`、`filevault`、`ios-backups`、`shadow-copies` | 加密卷、iOS 备份与卷影副本 |
| `mounts` | 只读挂载与 SMB 共享（Windows） |
| `plugins`、`analysis`、`bcp` | 插件、插件任务、分析结果与 BCP 数据 |
| `raid`、`disks` | RAID 组装与主机物理磁盘 |
| `tasks` | 任务中心：插件任务与内置任务的列出、查看、取消，以及等待后台任务结束 |
| `system`、`context`、`cli` | 本程序与主机、会话上下文、命令搜索 |
| `schema`、`complete`、`session`、`shell` | 根命令：请求结构、shell 补全、JSONL 会话、交互 shell |

桌面端注册的每个后端命令都恰好对应一个 CLI 命令（或在 `src-tauri/src/cli/commands.toml`
中注明被哪条命令覆盖）。新增后端命令未写入该文件、动词不在动词表里或参数名冲突时构建失败，
CLI 不会静默漏接功能。

### 全局选项与上下文

| 选项 | 作用 |
| --- | --- |
| `-i, --image <ID\|路径>` | 命令作用的检材：已加载检材的 ID，或检材路径（第一次用到时打开） |
| `--case <目录>` | 运行前打开的案件 |
| `-q, --quiet` | 不输出横幅、进度等非必要信息 |
| `-h, --help` / `-v, --version` | 帮助与版本 |

省略 `--image` 时依次取环境变量 `LOVELYCARVE_IMAGE`、当前目录 `.env.local` 与 `.env` 中的同名变量
（`.env.local` 优先）和会话当前检材；`--case` 同理取 `LOVELYCARVE_CASE`。进程环境优先于这两个文件，
文件只读取这两个变量，也不会传给插件等子进程。检材取自环境变量或文件时，第一次用到它时在标准错误
提示 `使用检材：<值>（来自 .env.local 中的 LOVELYCARVE_IMAGE）`，`--quiet` 关闭该提示；设置 `DEBUG=1`
时还会提示 `使用案件：<目录>（来自 --case）`。命令需要检材却没有指定时，错误逐条列出可用的指定方式。
有案件时，`images open` 把检材登记进案件并在后台建立卷索引；没有案件时索引只在本进程内使用，
退出即丢弃。

### 不完整 RAW

默认打开会检查可观察到的 RAW 编号缺口。需要读取不完整镜像时显式使用：

```powershell
lovelycarve-cli images open "D:\evidence\disk.001" --raw-read-mode force
```

强行模式只拼接从所选文件开始的连续前缀，遇到缺片即停止；也支持 `disk.dd` 接
`disk.001` 或 `disk.002`。跨容量读取截短，完全越界读取返回有坏区审计的零字节，
运行中短读保留已读部分。`rawReadMode` 返回 `force`；默认值为 `strict`。
模式只在当前进程的已打开检材上生效；用 `shell` 或 JSONL `session` 继续浏览、哈希与导出，
JSONL 参数为 `rawReadMode: "force"`。关闭后需再次指定，不能与 QCOW 凭据一起使用。
已加载为另一模式时先 `images close` 再打开。源变化和最终短读仍会使哈希、导出失败；
导出的可用前缀不表示原始镜像完整。详细边界见 [SMRAW](formats/SMRAW.md#不完整-raw-的强行读取)。

## 发现命令

```powershell
lovelycarve-cli                                  # 入门提示
lovelycarve-cli --help                           # 全部集合
lovelycarve-cli files --help                     # 一个集合的命令
lovelycarve-cli files read --help                # 一条命令的参数、选项与示例
lovelycarve-cli cli search "export a file"       # 按意图查找命令
lovelycarve-cli docs search "BitLocker 恢复密码"  # 搜索内置文档
lovelycarve-cli schema files hash                # 后端请求结构
lovelycarve-cli complete powershell >> $PROFILE
```

帮助采用 yargs 的版式：先是用法行与说明，再依次是「命令」（集合帮助）或「位置参数」「全局选项」「选项」
「示例」表格。说明按终端宽度换行，标准输出不是终端时按 80 列；每行右侧标注 `[字符串]`、`[必填]`、
`[可选值: …]`、`[默认: …]` 等。集合帮助列出完整的命令行（如 `lovelycarve-cli images open <path>`），
命令帮助末尾提示对应的 `schema` 命令。只输入集合名（如 `lovelycarve-cli images`）时显示该集合的帮助。

`cli search "<任务>"` 在本地索引中查找命令，中英文均可（`"计算文件哈希"`、`"recover deleted files"`），
返回最多 5 条 `{command, summary}` JSON，最佳匹配在前。

`docs search "<问题>"` 在随程序附带的文档中按章节搜索：使用指南、本手册、iOS 备份、Linux 基本信息插件
和全部格式专题，返回最多 5 条 `{title, section, description, page}`，无需联网。`docs get <page>` 把页面的
Markdown 原样写到标准输出，`--section <章节>` 只输出一个章节；`docs list` 列出全部页面。JSONL 会话中
`docs get` 返回 `{page, title, section, markdown}`。文档随构建嵌入程序，与该版本的行为一致。

`schema <命令…>` 输出该命令的请求参数结构：`operationId`、`ipcCommand`、`category`、
`hasRequestBody`、`requestBodyFields`（每个参数的 API 名称、Rust 类型、是否必填、说明、
对应的命令行写法，和可选的 `enum`、`default`、`generated`（省略时自动生成）、`sensitive`（会脱敏））
与 `output`（`json` 或 `bytes`）。路径参数解析到的节点参数记在 `pathResolvesTo`，删除类命令带 `destructive`。
`schema --list` 列出全部命令。手写命令也有完整参数结构，`ipcCommand` 为 `null`；
`path` 与节点 ID 的二选一关系见 `targetAlternatives`。`sessionOutput` 描述 JSONL 返回形式，
原始字节在会话中统一返回 `{encoding:"hex", data:"…"}`，可用请求的 `output` 保存到主机文件。

`complete <bash|zsh|fish|powershell>` 输出补全脚本；按 Tab 时 shell 以 `complete -- <单词…>`
回调本程序取候选，候选来自同一份命令目录，包括 `-q`、`-f`、`-j` 等短选项和枚举值。

隐藏命令 `tools` 把全部命令输出为 MCP 工具定义 `{version, tools: [{name, description, inputSchema}]}`，
供 MCP 服务器包装本程序；属性名与 `schema`、`--body`、JSONL 的 camelCase API 参数名一致，检材照常取 `LOVELYCARVE_IMAGE`。
隐藏命令可以运行，也能被 `cli search` 找到，但不出现在帮助、补全和拼写建议中。

由编码代理运行时（`CLAUDECODE=1`、`CODEX_THREAD_ID`、`GEMINI_CLI=1`、`CURSOR_AGENT=1` 等代理文档化的
环境变量），帮助开头附加一段英文说明：优先用 `cli search` 查找命令、用 `docs search` 查阅文档，
搜索词不要包含检材路径、案件名、文件名、哈希或密码等识别信息，并用 `schema` 查看请求结构。

## 输出与退出码

- 结果写标准输出。结构化结果是缩进两格的 JSON，后端结构字段统一使用 camelCase（如 `imageId`、`nextCursor`、`bytesHashed`）；
  证据中的数据库列名、插件字段名和任意 JSON 对象键保留原样。此转换仅在 CLI 输出边界执行，
  桌面 IPC 和案件存储格式不变；只有标准输出是终端且
  不在 CI 中时才着色（`NO_COLOR` 关闭、`FORCE_COLOR` 强制）。内容类命令（`files read`、
  `files zip members get`、`carve artifacts get`、`docs get` 等）输出原始字节，可直接重定向到文件；
  `--text` 按 UTF-8 解码，无效字节替换为 U+FFFD。
- 没有返回值的命令标准输出为空；在终端中标准错误显示一行 `✓ 完成` 之类的确认。
- 横幅 `lovelycarve-cli · v<版本>`、进度、交互提问、提示和错误都写标准错误。横幅只在标准错误是终端时
  出现，`--quiet` 关闭横幅与提示。标准输出和标准错误都是彩色终端时，运行中的命令先显示一条 `│`，
  再显示 `⠋  读取中 [3s]` 式的旋转进度（标签只写动作，命令本身已说明对象），结束时变为
  `◇  读取中 [3s]`，失败时为红色的 `▲`。`LOVELYCARVE_QUIET=1` 同时关闭横幅、旋转进度和终端进度；
  终端进度指 Windows Terminal 等支持 OSC 9;4 的终端在标签页或任务栏显示的进度，`--quiet` 或
  `LOVELYCARVE_NO_OSC_PROGRESS=1` 关闭。
- 错误显示为带边框的块：命令行、参数与本程序的错误为 `┌ 错误` 加说明；后端命令失败为 `┌ 后端错误`、
  `│ [错误代码] 说明`，以及一行 `IPC <后端命令>`。设置 `DEBUG=1` 时附带后端详情。
- 退出码：`0` 成功；`1` 失败（命令行错误、后端错误、未确认的删除、会话中任一请求失败）；
  `130` 被 Ctrl+C / Ctrl+Break / SIGINT / SIGTERM 中断，或在提问时按 Esc / Ctrl+C 取消
  （提问处显示 `■`，不再另外显示错误块）。

## 参数

- 位置参数是命令的主体，选项用 kebab-case（`--read-fault-policy`）。API 名称是后端的 camelCase
  参数名（`readFaultPolicy`），也是 `--body`、`schema` 和 JSONL 会话使用的键；不接受 snake_case 或命令行选项名作为 JSON 键。
- 数字直接写；布尔选项单写 `--descending` 为真，`--descending=false` 为假；数组可在一个选项后
  列出多个值（`--algos md5 sha256`）、重复选项（`--algos md5 --algos sha256`）或传一个 JSON 数组，
  列出多个值时把位置参数写在数组选项之前；枚举的可选值列在帮助中。
  结构体参数传 JSON 文本或 `@文件路径`；不是 JSON 的单个词按字符串处理。
  顶层参数名是 camelCase，嵌套选项沿用原类型的 serde 命名（很多为 snake_case）。
- `--body '<JSON 对象>'` 或 `--body @request.json` 一次给出全部参数，显式选项覆盖其中的同名键；
  必填的位置参数也可以写在其中（`images open --body '{"path":"D:/evidence/disk.E01"}'`），两处都没有时
  报 `缺少参数`。
- 缺少必填值时，终端中逐项提问：`◆` 提出问题，回车后变为 `◇` 并显示答案，枚举给出选择列表；
  脚本、CI 和代理中直接报 `缺少 --<选项>`，不会猜测。
- 密码与密钥类参数（`--password`、`--qcow-password`、`--vmk-hex` 等）：`--password <值>`；
  `--password @文件` 读取文件并去掉末尾换行；单写 `--password` 或 `--password -` 时，
  终端中以 `•` 遮盖输入，否则读取管道传入的标准输入，例如
  `Get-Content secret.txt | lovelycarve-cli bitlocker unlock 0 --password`。
  密码不写入案件、任务描述或日志。
- `--dry-run` 输出将发送的请求 `{command, ipcCommand, body}`，不打开检材、不执行，
  密码等敏感字段显示为 `<redacted>`。
- 删除类命令（`delete` 动词，以及 `images tags update`、`mounts delete` 等会丢弃数据的命令）
  在终端中先确认；没有人可以确认时必须加 `--force`（`-f`），否则显示问题和 `Aborted.` 并以 1 退出。
- 写主机文件的命令（`--destination`）要求目标不存在，避免覆盖已有文件。
- `taskId`、`searchId` 省略时自动生成；需要用 `tasks cancel <taskId>` 取消时可显式指定。

## 浏览检材

镜像内路径用 `/` 分隔。`/` 是检材的合成根，列出分区和卷（目录检材、归档等同样先显示根名称）；
不以 `/` 开头的路径相对会话当前目录，支持 `.` 和 `..`，不跟随符号链接。
每个接受路径的命令也接受节点 ID（`--node-id`、`--parent-id` 或 `--root-id`，取 `files list`
输出的 `items[].id`），用于同名节点、删除条目和特殊流。

| 命令 | 作用 |
| --- | --- |
| `files list [--path 路径 \| --parent-id ID] [--cursor 游标] [--page-size N] [--query JSON]` | 统一调用 `fs_children_page`；默认列当前目录，返回 `{items:[节点], nextCursor, total?, truncated}`。`nextCursor` 非空时继续请求下一页；每页默认 500，范围 1–5000 |
| `files tree list [--path 路径 \| --node-id ID] [--depth N] [--name 文本] [--limit N]` | 递归遍历目录；`--depth` 递归（上限 128），`--name` 忽略大小写包含匹配，最多检查 `--limit` 个节点（默认 10000，上限 100000）。返回 `{items:[{path,node}], truncated}` |
| `files get <路径>` | 节点属性（名称、类型、大小、inode、时间戳、删除状态、链接目标），附 `nodeId`、`path`、`indexOnly` |
| `files read <路径> [--offset N] [--length N] [--text]` | 文件内容流式写到标准输出 |
| `files hash <路径> [--algos …]` | MD5 / SHA-1 / SHA-256 / SM3 |
| `files identify <路径>` | Magika 内容类型识别，不依赖扩展名 |
| `files strings list <路径> [--encoding …] [--min-length N]` | 可打印字符串及文件内偏移，支持 ASCII、UTF-8、UTF-16LE/BE |
| `files search <模式> [路径]` | 在子树内容中搜索关键词或正则 |
| `files export <路径> --destination <主机路径>` | 导出单个文件；目录用 `files directories export --mode tree\|zip` |

`files list`、`files tree list`、`files get` 与路径解析复用目录分页接口，优先查询案件卷索引或本进程的临时索引，
未被索引覆盖的目录沿用后端的快照／实时回退。原检材缺失时（案件中的 `index_only` 检材）
仍可浏览已索引目录，读取字节的命令明确报错。达到深度或数量上限时 `truncated` 为真，
不能当作完整列表。

`files search` 复用全局搜索，默认搜索当前目录子树，路径指向文件时只搜该文件，`/` 搜索全部已索引卷；
需要打开案件、卷索引已建立且原检材可读。默认区分大小写并包含已删除文件：`--ignore-case`、
`--regex`、`--exclude-deleted`，`--codepage UTF-8` 指定编码（可重复，省略时搜索全部支持的编码）。
返回 `{summary, hits, total, truncated}`；`--limit` 只限制返回条数（默认 10000），完整命中保存在案件中，
可用 `searches hits list --search-id <ID>` 继续分页。

字节层面的属性（MIME、熵、数据运行段）用 `files content get`，自动识别编码的文本用 `files text get`；
SQLite、Realm、ZIP、CSV、可执行文件、GGUF、Vim 交换文件、NTFS 流与 USN 日志、ext4 日志分别在
`files sqlite …`、`files realm …`、`files zip …` 等子集合中，`lovelycarve-cli files --help` 列出全部。

### RAID → LVM → Btrfs 的导航

在同一个 `shell` 或 JSONL `session` 中依次打开成员盘并运行 `raid detect`。
所有完整、未用作 md 成员的线性逻辑卷（包含单 PV 卷）都会注册为独立检材；
`volumeGroups[].logicalVolumes[].assembledImageId` 是可直接选择的检材 ID。
不完整或不支持的卷不会伪造这个 ID，具体原因见检测结果的 `diagnostics`。

```text
images open "D:/evidence/member1.img"
images open "D:/evidence/member2.img"
raid detect
images list
context use <逻辑卷的 assembledImageId>
files list
context set --dir "<上一步返回的卷名称>"
files list
files read hello.txt
```

`context use <检材 ID>` 选择检材并回到 `/`；`context set --image <检材 ID>` 也能切换，
切换到不同检材时清除旧目录。对已加载检材的普通查询中，`imageId`／`--image` 只限定该次查询。
`images open <已加载检材 UUID>` 或 `images open "assembled://…"` 会复用已有检材并设为当前检材。
`assembled://` 是当前会话内的引用；程序退出或卷已关闭后，需重新加载成员盘并运行 `raid detect`，
不能把它当作主机磁盘路径。找不到会话时会返回 `notFound` 和恢复步骤。

JSONL 可直接引用检测结果及目录名称，避免抄写 ID：

```jsonl
{"id":"raid","command":"raid detect"}
{"id":"use","command":"context use","args":{"imageId":{"$ref":"raid#/volumeGroups/0/logicalVolumes/0/assembledImageId"}}}
{"id":"roots","command":"files list"}
{"id":"cd","command":"context set","args":{"dir":{"$ref":"roots#/items/0/name"}}}
{"id":"files","command":"files list"}
{"id":"read","command":"files read","args":{"path":"hello.txt","length":4096}}
```

仍可沿原始检材的 LVM 分区进入逻辑卷：识别到的 Btrfs／ext／XFS／NTFS 卷显示为 `kind: "dir"`。
原始 LV 键为 `lvm:<分区ID>:<卷索引>`；Btrfs 文件节点为
`btrfs:lvm:<分区ID>:<卷索引>:<子卷ID>:<inode>`。
例如 `btrfs:lvm:0:1:5:256` 表示分区 0、卷索引 1、子卷 5 的 inode 256；
这些值由目录枚举提供，不能从卷名猜测。默认 FS 树使用子卷 5，常见根 inode 为 256，
但脚本应复制 `files list` 返回的 `items[].id`，或使用上面的普通路径导航。

## 交互 shell

```text
lovelycarve-cli shell
lovelycarve:/> images open "D:\evidence\disk.vmdk"
lovelycarve:/> files list
lovelycarve:/> cd "<卷名称>/Users"
lovelycarve:/<卷名称>/Users> files tree list --depth 3 --name report
lovelycarve:/<卷名称>/Users> files get alice/report.txt
lovelycarve:/<卷名称>/Users> files read alice/report.txt --length 4096 --text
lovelycarve:/<卷名称>/Users> files export alice/report.txt --destination "D:\output\report.txt"
lovelycarve:/<卷名称>/Users> exit
```

`shell` 在一个进程中逐行执行命令，语法与命令行相同（省略程序名），案件与检材在命令之间保持打开。
`cd <路径>` 切换镜像内目录（同 `context set --dir`），`pwd` 显示当前目录，`help` 显示帮助，`exit` 退出。
引号只负责参数分组，Windows 反斜杠保持原样，不做变量展开或管道；单引号适合包住 JSON。
`--case` 只能在启动时给出，会话中改用 `cases open`。

标准输入不是终端时，`shell` 执行管道传入的脚本：忽略空行和 `#` 注释，不显示提示符，
遇到第一个失败的命令即停止并以 1 退出。脚本中的密码参数不会从标准输入读取（标准输入就是脚本），
必须直接给值或用 `@文件`。

## JSONL 会话

```powershell
Get-Content .\workflow.jsonl | lovelycarve-cli --case "D:\cases\demo" session --events
```

```jsonl
{"id":"image","command":"images open","args":{"path":"D:/evidence/disk.img"}}
{"id":"roots","command":"files list","args":{"imageId":{"$ref":"image#/id"},"parentId":""}}
{"id":"scan","command":"carve scans create","args":{"options":{"format_ids":["png","pdf"],"scope":"whole"}}}
{"id":"done","command":"tasks wait"}
{"id":"status","command":"carve scans get","args":{"scanId":{"$ref":"scan#/scanId"}}}
{"id":"recover","command":"carve scans export","args":{"scanId":{"$ref":"scan#/scanId"},"options":{"destination":"D:/output/carved","task_id":"recover-1"}}}
```

每行一个请求：`id`（非空且唯一）、`command`（命令路径，如 `"files read"`）、`args`（只接受 `schema` 中的 camelCase API 参数名），
可选的 `output`（把结果写入新文件）、`force`（确认删除类命令）和
`dryRun`。接受路径的命令既可以传 `path`，也可以传节点 ID。

- 每条响应是一行 `{"id":"…","result":…}` 或 `{"id":"…","error":{"code":"…","message":"…"}}`，
  `details` 仅在后端提供详情时出现，写出后立即 flush。能解析出字符串 ID 的无效请求保留该 ID，
  无法解析的行使用 `"id":null`。`result` 保留后端命令原始的 JSON 字段。
- 原始字节返回 `{"encoding":"hex","data":"…"}`，`files read` 单次最多 16 MiB，可用 `offset`/`length`
  分段；带 `output` 时原始字节或 JSON 写入不存在的新文件，结果为 `{"output":"<路径>"}`。
- `{"$ref":"<id>#/<JSON 指针>"}` 引用先前成功请求的结果；引用尚在执行的请求时先等待它完成，
  失败、未知 ID 和不存在的指针明确报错。
- `--jobs N`（简写 `-j`，1–64，默认 1）允许并发：目录浏览、文件读取／哈希、内容搜索、结果分页、
  任务状态等查询可以重叠执行，其他命令是屏障，先等此前的请求完成再执行。打开／关闭检材、
  切换案件、`context set` 和 `tasks wait` 因此保持顺序；并发查询使用派发时的当前检材和目录。
  并发响应按完成顺序输出，客户端必须按 `id` 关联。
- 删除类命令需要在请求中加 `"force": true`，否则返回 `aborted` 错误。
- `tasks wait` 等待进程内任务与插件任务结束，只表示任务不再活动；结果应读取对应的状态或结果命令。
- 旧版命令名（如 `image_load`、`fs_read_range`、`ls`、`wait`）返回错误，并在消息中给出现在的命令。
- 单行上限 16 MiB，单会话最多 10000 个请求。标准输入保持打开时也持续返回已完成的结果；
  EOF 后先等待所有已派发请求，再等待后台任务。单条失败不影响后续请求，但会话以 1 退出。
- `--events` 把进度与生命周期事件以 `{"event":"…","payload":…}` 行写到标准错误；
  标准输出只有响应，没有横幅、进度或提示。

### 代理调用

代理宜先用 `cli search` 找命令、用 `schema` 查看请求字段，再启动一个长期运行的会话进程，
在同一进程中对已打开的检材连续提交请求，无需解析终端表格或中文提示：

```text
lovelycarve-cli --image "D:\evidence\disk.E01" session --jobs 4
```

```jsonl
{"id":"info","command":"images get"}
{"id":"roots","command":"files list","args":{"path":"/"}}
{"id":"tasks","command":"tasks list"}
{"id":"barrier","command":"tasks wait"}
```

协议版本为 3；旧的 `files children list` 已合并到 `files list`，原先递归浏览改为
`files tree list`，`files list` 不再接受位置参数或 `depth/name/limit`。脚本需改用 camelCase 结果字段和 `$ref` 指针。
`files hash` 直接调用 `fs_digest`，路径与节点 ID 共享同一参数及结果契约。

`system get` 的 `session` 对象描述协议版本、请求字段、并发上限和行长度限制。
标准错误只承载诊断与可选事件，代理应分别读取标准输出和标准错误。

`just test-cli` 覆盖并发、屏障、结果引用、错误关联和保持 stdin 打开的进程级回归。
设置 `LOVELYCARVE_CLI_TEST_IMAGE` 为外部镜像路径后运行 `just test-cli` 或
`just smoke-cli <可执行文件>`，会额外对比顺序与并发的目录查询和有界文件读取；该样本不加入仓库。

## 生命周期与边界

单条命令启动后端、执行命令、等待它启动的后台任务结束后退出；无案件检材的临时卷索引会被取消，
因为退出时本来就会丢弃。退出前取消剩余任务、关闭案件、停止插件并删除预览临时文件。
Ctrl+C / Ctrl+Break 取消任务、释放案件与挂载后以 130 退出；无法取消的阻塞读取最多再等 10 秒。

挂载、SMB 共享和媒体预览临时文件只在进程存活期间有效，应在 `shell` 或 `session` 中使用，
需要长期保留的内容请导出。暂停的雕刻请先恢复或取消再等待；CLI 退出不保存内存中的雕刻记录，
应在同一个会话中扫描、分页与恢复。

案件锁、源读取审计、文件格式限制和任务资源预算与桌面端一致。不要让桌面端和 CLI 同时打开同一案件；
检材以只读方式打开。平台限制沿用后端（例如 Dokan/SMB 挂载仅 Windows）；
`system paths reveal` 会打开系统文件管理器，不属于无界面分析流程。

`RUST_LOG=lovelycarve_lib=debug` 开启后端诊断，日志写标准错误。

## QCOW 镜像

QCOW v1/v2/v3 的压缩簇和差分父链自动读取；legacy AES 镜像用 `--qcow-password` 或
`--qcow-key-hex`（16 字节 key，二选一）打开：

```powershell
lovelycarve-cli images open "D:\evidence\disk.qcow2" --qcow-password
```

单写 `--qcow-password` 时在终端中提问或读取管道输入。JSONL 中写作
`{"id":"qcow","command":"images open","args":{"path":"D:/evidence/disk.qcow2","qcowPassword":"<密码>"}}`。
legacy AES 没有密钥验证器，成功打开不证明密码正确；QCOW 内嵌 LUKS、Zstd、
extended L2、external data file 和历史快照切换不支持。完整对照见 [QCOW](formats/QCOW.md)。

## EWF 后端

E01/Ex01 使用 [ewf-forensic](https://github.com/SecurityRonin/ewf-forensic) 仓库中的
`ewf` 0.4.10 纯 Rust 读取器，与桌面和挂载入口共用，不需要 `libewf.dll`。
支持多分卷、随机读取、zlib、Ex01 bzip2/模式填充、存储哈希与采集元数据。
打开时检查分卷序号和块表覆盖，缺失分卷会报错；坏块通过原有读取故障机制报告。
加密的 EWF2 容器仍会明确返回不支持；BitLocker 卷解锁是独立功能。

```powershell
just run-cli images open D:\evidence\disk.E01
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
just run-cli images open D:\evidence\disk.vmdk
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

`bitlocker metadata get <分区>` 无需凭据，返回算法、卷 GUID、原始 FILETIME（十进制字符串）、
卷描述和保护器 ID/类型，不返回密钥 payload。`bitlocker unlock <分区>` 只选一种凭据：
`--password`、`--recovery-password`、`--startup-key-path` 或 `--fvek-hex`（Diffuser 时加 `--tweak-hex`）；
全不传则尝试卷内明文密钥。原始 FVEK 没有认证标签，须核验解密结果。
`bitlocker vmks scan <内存镜像> --partition-id <分区>` 在内存镜像中搜索 VMK，
`bitlocker vmks unlock <分区> --vmk-hex` 直接认证 FVEK，卷没有恢复密码保护器时也可解锁；
结果 `recovery_key` 在无可提取密码时为空字符串。TPM/SID 不支持。
`bitlocker export <分区> --destination <主机路径>` 导出解密卷，不覆盖已有文件；
遇到已审计的合成源字节会失败，可查询 `images bad-regions list`。

```powershell
lovelycarve-cli shell
lovelycarve:/> images open D:\evidence\encrypted.E01
lovelycarve:/> bitlocker metadata get 0
lovelycarve:/> bitlocker unlock 0 --recovery-password
lovelycarve:/> bitlocker export 0 --destination D:\output\volume.raw
```

源码版本、本地补丁和公开样本范围见 `src-tauri/vendor/bitlocker/LOVELYCARVE.md`；
读取器测试用 `just test-bitlocker`。

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
just run-cli images open D:\evidence\disk.vhdx
just test-vhd
just test-vhdx
just test-cli
```

回归样本来源、固定的上游提交和本地改动见两个 vendor 目录的 `LOVELYCARVE.md`。
全盘 SHA-256 参考值来自 libvhdi 对公开样本的独立读取，测试本身不依赖该 DLL。

## LUKS / VeraCrypt 解锁

`encrypted-volumes unlock` 把已加载检材中的加密区域注册为会话内只读镜像，返回 `image.id` 和
`image.path`（`decrypted://…`），可再交给 `images open`，然后使用现有的文件系统、搜索、哈希、雕刻、
挂载及 RAW 导出命令。桌面镜像信息页「卷与分区」卡片的「解锁加密容器」与 LUKS 分区行的「解锁」
也提供相同入口。`.hc`、`.tc`、`.luks` 文件可直接加载。
桌面提交后立即关闭密码弹窗，在「任务中心」显示后台解锁状态、取消与失败原因，
成功后在源镜像分区内浏览并自动补建索引，不切换或新增镜像标签页；切换页面不取消任务。
从分区行进入时固定该分区。CLI 命令等待解锁结果，便于后续 JSONL 引用。
桌面使用 `options.target: "sourcePartition"`，返回原 `image.id` 与 `volume_key`，文件节点沿用
`part:N`；源镜像哈希仍读取源密文，RAW 导出省略 `mode` 或使用 `"source"` 时也读取源字节。
桌面「导出源镜像」使用 `images export` 的 `mode: "unlockedPartitions"`：以导出开始时的
已解锁状态投影所有 LUKS / VeraCrypt / BitLocker 分区，明文从原分区起始位置写入，
较短明文后的分区尾部补零；分区表、其他分区及整盘大小保留。导出开始后的解锁不改变本次输出，
解密会话关闭、短读或越界时报错。目标文件必须不存在，避免覆盖源镜像或已有导出。
省略 `target` 或使用 `"newImage"` 时保留下方示例的独立解密镜像行为，便于显式导出明文。
原分区接入要求解密内容为单个完整卷。

密码在 `options` 中，宜写入文件后用 `--options @unlock.json` 传入，避免出现在命令行历史中；
`--dry-run` 会把 `options.password` 与 `options.masterKeyHex` 显示为 `<redacted>`。JSONL 会话示例：

```jsonl
{"id":"source","command":"images open","args":{"path":"D:/evidence/container.hc"}}
{"id":"unlock","command":"encrypted-volumes unlock","args":{"imageId":{"$ref":"source#/id"},"options":{"scheme":"veracrypt","password":"example-password","pim":0,"hidden":false},"taskId":"unlock-1"}}
{"id":"plain","command":"images open","args":{"path":{"$ref":"unlock#/image/path"}}}
{"id":"roots","command":"files list","args":{"parentId":""}}
{"id":"export","command":"images export","args":{"destination":"D:/exports/plain.raw","taskId":"export-1","readFaultPolicy":"strict"}}
```

LUKS 使用 `"scheme":"luks"`，不传 `pim`/`hidden`。密码之外，也可只传 `options.masterKeyHex`（已恢复卷主密钥的十六进制，不与密码同时传）；LUKS1/2 均先核验密钥摘要。算法与能力对照见 [LUKS](formats/LUKS.md)。`options.partitionId` 选择分区；
也可指定相对源镜像的字节 `offset` 和 `length`（省略长度时到源末尾），两种区域选择
不能混用。默认整个源镜像。VeraCrypt 没有明文签名，需要用户明确选择格式和隐藏卷。
密码按 UTF-8 字节传入；命令本身不保存密码或导出主密钥。

LUKS1 与 LUKS2（单个动态数据段，PBKDF2 / Argon2i / Argon2id，512 / 4096 字节加密扇区）支持的
密码算法、IV 方案与哈希见 [LUKS](formats/LUKS.md)。VeraCrypt 支持普通／隐藏卷（512 字节扇区）、
上游支持的 AES / Serpent / Twofish 及其级联、PIM 0–1000。分区识别使用 512 字节逻辑扇区。
暂不支持密钥文件、分离头部、备份头恢复、系统加密、LUKS2 完整性 / 重加密 / 多数据段 / 固定长度数据段，
也不声明完整的 TrueCrypt 兼容性。LUKS2 主头校验失败时报错，不自动尝试备份头；超过 KDF 资源预算同样报错。

`tasks cancel <taskId>` 可以取消解锁；KDF 当前执行阶段可能延迟响应。
`images close <ID>` 可锁定返回的解密镜像；关闭源镜像会递归移除其解密镜像。
解密镜像和凭据不会作为可自动恢复的案件检材保存，进程结束后需重新解锁。
读取按需解密；只有显式执行导出、预览或分析缓存操作才会产生相应的明文输出。

## FileVault 2 / CoreStorage 解锁

先用 `filevault get --options '{}'` 读取逻辑卷目录和法证观察，再将选中的 `index` 作为
`logicalVolumeIndex` 传给 `encrypted-volumes unlock`。区域选择沿用 `partitionId` 或
`offset` / `length`；两个命令应使用同一区域。下例的输入是独立 CoreStorage 分区镜像：

```jsonl
{"id":"source","command":"images open","args":{"path":"D:/evidence/corestorage.raw"}}
{"id":"inventory","command":"filevault get","args":{"options":{},"taskId":"fv-inventory"}}
{"id":"unlock","command":"encrypted-volumes unlock","args":{"options":{"scheme":"filevault","password":"example-password","logicalVolumeIndex":0,"target":"newImage"},"taskId":"fv-unlock"}}
{"id":"plain","command":"images open","args":{"path":{"$ref":"unlock#/image/path"}}}
{"id":"files","command":"files list","args":{"parentId":"part:0"}}
```

`password` 同时接受个人恢复密码；也可改用 `masterKeyHex`（16 字节 VMK 或 32 字节
XTS 密钥对）。可选 `encryptedRootPlistPath` 指向本地 XML / EncryptedRoot.plist.wipekey，
应同时传给 inspect 和 unlock。单逻辑卷支持原分区投影；多逻辑卷必须分别使用
`newImage`，以隔离文件 ID 和索引。完整能力、上游 API 对照和不支持的布局见
[FileVault](formats/FILEVAULT.md)。APFS 原生加密使用另一种格式，当前不能由此解锁。

## 文件恢复命令

文件系统恢复与签名雕刻是同一个扫描会话的两个阶段，都由 `carve scans create` 发起，支持 `carve scans pause/resume` 与 `tasks cancel`。`options.filesystems` 的 `fat` / `ntfs` / `exfat` 严格控制各族探测（缺省全部关闭，即只做签名雕刻），`include_existing` 同时列出已知分区的现存文件；`signatures` 缺省为 true，设为 false 时只恢复文件系统记录。`scope` 除 `whole` / `partition` / `unallocated` 外还接受 `range`，配合 `start_sector` 与 `sector_count`。以下为 JSONL 示例（先用 `--image` 指定检材）：

```jsonl
{"id":"scan","command":"carve scans create","args":{"options":{"scope":"whole","filesystems":{"fat":true,"ntfs":true,"exfat":true,"include_existing":false},"signatures":false}}}
{"id":"done","command":"tasks wait"}
{"id":"status","command":"carve scans get","args":{"scanId":{"$ref":"scan#/scanId"}}}
{"id":"files","command":"carve files list","args":{"scanId":{"$ref":"scan#/scanId"}}}
{"id":"export","command":"carve scans export","args":{"scanId":{"$ref":"scan#/scanId"},"options":{"artifact_ids":[],"file_ids":["0"],"destination":"D:/output/recovery","task_id":"recover-files-1"}}}
```

快照的 `phase` 表示正在运行的阶段，`filesystem` 摘要给出阶段进度、卷/目录/文件数、截断、读取故障与阶段错误。`carve files list` 在文件系统阶段结束前返回 `null`；NTFS 重建卷的 `reconstruction` 给出 MFT 位置和推导几何，目录在 `directories` 中，文件在 `candidates` 中，回收站还原项包含 `sourceNode/sourcePath`，其原始数据映射和身份不变。

`carve scans export` 的 `file_ids` 只接受该会话文件系统结果中的候选编号，且候选必须 `readable`，不能传入任意原始数据映射；`artifact_ids` 为 `null`（缺省）时恢复全部雕刻结果，同时传 `file_ids` 时可用 `[]` 只恢复文件系统记录。顶层计数描述雕刻结果，`files` 描述文件系统记录：输出目录、成功/失败数、JSONL 报告路径与读取故障。文件系统记录在输出目录中按 `v<卷号>-<卷标>/原目录/文件名` 保持目录结构，具体文件状态、来源、`relative_output` 和 SHA-256 见 JSONL 报告。能力和限制见[文件恢复](USAGE.md#文件恢复)。

### 插件分析

系统级分析器在检材上识别出的操作系统上运行：`plugins systems list` 列出系统 ID（如 `linux@part:2`）及其挂载的卷，`plugins functions list` 列出分析器的作用域、适用平台与选项。

```bash
lovelycarve-cli plugins tasks create --function org.lovelycarve.linuxinfo/linuxbaseinfo --system-id linux@part:2 --options '{"items":["users","logins"]}'
lovelycarve-cli tasks wait
lovelycarve-cli analysis tree get
lovelycarve-cli analysis records list --run-id <runId> --record-type linux.user --sort uid --descending
lovelycarve-cli analysis runs get <runId>
```

检材级分析器不需要 `--system-id`，文件级分析器给 `--node-id` 与 `--file-name`；省略的选项取默认值。同一（分析器、检材、目标）最多一个未结束的任务，重复创建返回已有任务。插件任务不能暂停：`plugins tasks cancel` 结束执行，`plugins tasks restart` 从头再跑，成功后新结果替换旧结果。

`tasks list` 与桌面任务中心列出同一份任务：插件任务（id 为 `plugin:<任务 id>`，带完整任务行）与内置任务（带种类、对象、阶段、进度、计数与结局），`--image-id` 只列一份检材的任务。`tasks get <id>` 查看一条，`tasks cancel <id>` 取消内置任务或插件任务。全局搜索与时间线构建的记录来自案件，其余内置任务只列出本进程运行过的。

`analysis tree get` 按检材 → 分类 → 分析器运行 → 记录类型返回已发布的结果及其 `runId`。`analysis records list` 必须传 `--run-id` 与 `--record-type`，可用 `--keyword` 检索标题与文本类列、`--sort <列 id 或 title>` 与 `--descending` 排序；同一次分页或导出固定 runId，新任务完成时不会切换。`analysis runs get` 给出运行的输入文件、选项、画像事实、告警与读不出而以零填充的源字节。旧版本插件写下的结果在树中带 `legacy: true`，读取时加 `--legacy`。删除插件任务只删除该任务发布的运行结果。
