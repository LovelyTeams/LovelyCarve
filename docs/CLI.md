# LovelyCarve CLI

`lovelycarve-cli` 是独立的取证命令行工具：不链接 Tauri、WebView 或任何桌面窗口运行时，自行初始化后端状态，直接调用与桌面端相同的解析器、案件库、插件和任务调度。

- 检材内的位置写成**地址**：`/` 列出检材，`/<别名>` 列出一份检材的分区与卷，`/<别名>/<卷>/<路径>` 指向文件，`/<别名>/@<节点 ID>` 按 ID 指向任意节点。
- 读取检材内容用熟悉的动词：`ls`、`tree`、`stat`、`cat`、`text`、`strings`、`hash`、`find`、`grep`、`export`；其余命令是「名词 动词」：`case new`、`evidence add`、`unlock`、`carve start`、`results rows`。
- 结果默认是给人看的文本；加 `--json` 输出 JSON，供脚本和编码代理使用。
- 案件是工作区。`case open` 为案件启动一个**后台进程**，之后指向该案件的命令都在其中执行，复用已打开的检材、解锁与组装结果；没有后台进程时每条命令自己打开检材。
- 没有专门命令的后端功能用 `rpc <后端命令>` 按名称调用，参数与桌面端 IPC 相同。

常用章节：[快速开始](#快速开始)、[地址](#地址)、[案件与后台进程](#案件与后台进程)、[输出与退出码](#输出与退出码)、[参数](#参数)、[浏览与读取](#浏览与读取)、[批量请求](#批量请求)。iOS 备份见 [iOS 备份](IOS_BACKUPS.md#cli)。

## 构建与运行

```powershell
just run-cli --help
just run-cli --image "D:\evidence\disk.E01" ls /disk
just build-cli
```

`just build-cli` 构建 `src-tauri/target/release/lovelycarve-cli.exe`，默认关闭二维码和 OCR，无需 OpenCV 或 CUDA。需要 QR/OCR 时使用 `just build-cli-full`：Windows 配置 Visual Studio / OpenCV / OpenSSL，启用带 CPU 回退的 CUDA OCR；Unix 完整配方使用 CPU OCR。轻量构建中 `qr`、`ocr` 仍然列出，但返回功能未启用错误。`identify`（Magika）和 `ocr` 在程序旁的推理组件 `lovelycarve_inference.dll` 中运行，CLI 本身不链接 ONNX Runtime；组件缺失时这两个命令报错，其余功能不受影响。`lovelycarve-cli system` 报告版本、平台、`standalone`（未链接桌面运行时）、QR / OCR / 挂载功能、推理组件（`components.inference`）和后台进程协议。

CLI 构建不启用 Cargo 的 `desktop` 特性，不包含 Tauri、WebView2 或 GTK 依赖，Linux 上也不需要桌面库。Windows 构建在链接时嵌入 `asInvoker` 清单（`src-tauri/cli.manifest`），普通镜像分析不会弹出 UAC，管道可直接使用；读取物理盘、Dokan 挂载或扫描内存时从管理员终端运行。案件登记与偏好放在与桌面端相同的用户数据目录，两个程序看到同一份案件列表。

`just pack-cli` 构建并打包轻量 Windows CLI，包含运行库、推理组件、`lc-plugin-host`、插件、本手册及代理技能；`just pack-cli-full` 另启用 QR/OCR 并携带 OpenCV 与模型。`just repack-cli` 按现有二进制 `system` 报告的实际功能打包。插件包从程序旁与用户数据目录的 `plugins/` 发现，每个插件实例都在程序旁的 `lc-plugin-host` 进程中运行；`plugin inspect <插件包.lcpkg>` 检视、`plugin install <插件包.lcpkg>` 安装插件包，`plugin market` 从插件市场安装。

## 快速开始

新调查先建案件，再登记检材，让后台构建的卷索引、搜索、时间线和分析结果保存在案件中：

```powershell
lovelycarve-cli case new "D:\cases\nas" --name "NAS 检验" --open
lovelycarve-cli -C "D:\cases\nas" evidence add "D:\evidence\nas.E01" --as nas --wait
lovelycarve-cli -C "D:\cases\nas" volumes
lovelycarve-cli -C "D:\cases\nas" ls /nas/part:0/etc
lovelycarve-cli -C "D:\cases\nas" text /nas/part:0/etc/os-release
lovelycarve-cli -C "D:\cases\nas" hash /nas/part:0/etc/shadow --algo sha256
lovelycarve-cli -C "D:\cases\nas" export /nas/part:0/home/alice -o "D:\output\alice"
lovelycarve-cli -C "D:\cases\nas" case close
```

`--open` 启动案件后台进程，后面每条带 `-C` 的命令都在其中执行，不再重复打开 E01。`evidence add --wait` 等待卷索引建好并报告各卷状态。命令在案件目录内运行时可省略 `-C`。

只读一个文件、不需要保存任何结果时，可以不建案件：

```powershell
lovelycarve-cli --image "D:\evidence\disk.E01" cat /disk/part:0/Windows/System32/config/SAM > SAM
```

`lovelycarve-cli` 不带参数时给出入门提示，`--help` 列出全部命令，`<命令> --help` 显示一条命令的参数、默认值与示例，`schema <命令>` 以 JSON 描述同样的内容。

## 命令一览

| 分组 | 命令 |
| --- | --- |
| 案件与检材 | `case`（`status` / `new` / `open` / `close` / `ls` / `lock` / `unlock` / `link` / `rm`）、`evidence`（`ls` / `add` / `show` / `close` / `rm` / `hash` / `export` / `bad-regions`）、`volumes`、`encryption`、`unlock`、`raid`（`detect` / `assemble`）、`vss`（`ls` / `open`） |
| 浏览与读取 | `ls`、`tree`、`stat`、`cat`、`text`、`strings`、`hash`、`identify`、`streams`、`export`、`bodyfile` |
| 查找与解析 | `find`、`grep`、`journal`、`inspect`、`sqlite`（`tables` / `rows` / `query` / `deleted`）、`ocr`、`qr` |
| 分析 | `search`（`ls` / `hits` / `export` / `rm`）、`timeline`（`status` / `build` / `ls` / `stats` / `export`）、`carve`（`ls` / `formats` / `start` / `status` / `results` / `cat` / `pause` / `resume` / `export` / `rm`）、`plugin`、`analyze`、`results`（`ls` / `rows` / `show` / `run` / `import`） |
| 任务与主机 | `tasks`（`ls` / `show` / `wait` / `cancel` / `restart` / `rm`）、`index`（`ls` / `build` / `rm`）、`mount`、`umount`、`tz`（`set`）、`system`、`disks` |
| 程序 | `shell`、`batch`、`rpc`、`schema`、`completion` |

分组命令单独给出时执行其默认子命令（`case` 即 `case status`，`evidence` 即 `evidence ls`）。全部命令的用法由命令树生成，见代理技能的 [`references/commands.md`](../skills/lovelycarve-cli/references/commands.md)。

## 地址

- `/` 列出检材；`/<别名>` 列出一份检材的分区与卷；`/<别名>/<卷>/<路径>` 指向卷内的文件或目录，例如 `/nas/part:0/etc/passwd`。
- 卷这一段写根目录条目的 ID：分区 `part:0`、LVM 逻辑卷 `lvm:0:1`、尚不可读的分区（如锁定的 BitLocker、LUKS 或 FileVault 卷）`unsupported:0`，也可写作 `part:0`；目录与归档检材只有一个根条目，如 `/<别名>/folder:`、`/<别名>/tar:`。这一段也接受该条目的名称，但 ID 稳定、只含 ASCII，不受卷名中的空格、括号、`·` 或重名影响。卷入口名称只保留字母、数字、中文、`.`、`-`、`_`，其余分隔符合并为 `_`（`ext4 (md · host:0)` → `ext4_md_host_0`），转换后重名的卷须用 ID。
- 卷以下的各级按实际文件名解析，支持 `.` 与 `..`。名称优先精确匹配；没有精确命中时，NTFS、FAT/exFAT、HFS+ 与不区分大小写的 APFS 卷接受唯一的忽略大小写匹配，多个候选报歧义；其他文件系统保持精确匹配。合成节点使用中文名，如 `分区0`、`$未分配簇`、`未使用空间1`，以列表为准。
- `/<别名>/@<节点 ID>` 按结果返回的 `id` 指向节点，用于同名文件、已删除条目、命名流和未分配空间；`ls -l` 显示 ID。ID 精确指向该节点，不跟随符号链接。不要按名称拼接 ID。
- 每个列出的条目都带有可直接使用的 `path`。路径解析不到时以 `notFound` 失败，并写明所在目录和找不到的名称。
- 别名默认取文件名到第一个点为止的部分（`nps-2010-emails.E01` → `nps-2010-emails`），也可用 `evidence add --as` 指定；案件检材的别名保存在案件中，每个进程看到同一个地址。派生的检材（解锁的容器、组装的阵列、卷影副本）由命令起别名，或用 `--as` 指定。

### 符号链接

路径像 shell 一样跟随符号链接：路径中间的链接总是跟随；末尾的链接由读取、哈希、导出、文本和列目录等命令跟随，`stat` 描述链接本身并返回 `linkTarget`。经过链接的路径在 `stat` 结果的 `path` 中显示实际到达的位置。

相对目标从链接所在目录解析；绝对目标从链接所在卷的根解析（分区、逻辑卷、APFS 卷、btrfs 子卷、容器或目录 / 归档检材的根），不从工作区根开始，`..` 也不越过该根。一条路径最多跟随 40 个链接。目标位于另一个卷（单独挂载的 `/usr`、`/var`，或根文件系统挂载自另一个 btrfs 子卷）、指向 Windows 盘符（`C:\…`）、卷适配器没有提供目标或成环时明确失败，不会猜测，也不会读取主机上的同名文件。目录导出保留目录树内的链接，不跟随。

### Git Bash / MSYS2

Git Bash 和 MSYS2 启动 Windows 程序时，会把以 `/` 开头的参数改写成主机路径，加引号也一样：`/nas/etc` 变成 `C:/Program Files/Git/nas/etc`；含 `:/` 的地址（如 `/ev/folder:/a.txt`）被当成路径列表，变成以 `;` 分隔的两个主机路径。CLI 识别出改写到 MSYS 安装目录（含 `usr/bin/msys-2.0.dll`）下的这两种形式时还原为原来的地址；无法还原的（例如改写到 `/c` 等挂载目录）报错说明原因。`grep` 的模式等其他以 `/` 开头的参数不会还原，需在这次调用前加 `MSYS2_ARG_CONV_EXCL='*'`（Git Bash 也可用 `MSYS_NO_PATHCONV=1`）：

```sh
MSYS2_ARG_CONV_EXCL='*' lovelycarve-cli -C 'D:/cases/nas' grep /etc/passwd /nas/part:0
```

## 案件与后台进程

命令按以下顺序确定案件：`-C <目录>`、环境变量 `LOVELYCARVE_CASE`、当前目录或其上级中的案件（此时在标准错误提示使用了哪个案件）。`case new`、`case ls`、`case rm` 自己给出或不需要案件目录。

| 命令 | 作用 |
| --- | --- |
| `case new <目录> [--name] [--number] [--description] [--examiner] [--open]` | 创建案件；`--open` 同时启动它的后台进程 |
| `case open [目录]` / `case close [目录]` | 启动 / 停止案件后台进程 |
| `case status` | 当前案件、后台进程、检材与别名、卷索引和运行中的任务 |
| `case ls` | 本机登记的案件，`backend` 标出有后台进程的 |
| `case lock` / `case unlock` | 锁定 / 解锁案件 |
| `case link [路径] [--offset <字节>] [--length <字节>]` | 案件或其中文件、目录的 `lovelycarve://` 链接；在桌面端打开链接即定位到该处，`--offset` / `--length` 给出要高亮的字节（长度默认 1） |
| `case rm <目录>` | 删除案件目录中由 LovelyCarve 创建的台账、检材数据与暂存区；先确认，后台进程运行时拒绝 |
| `evidence add <检材路径>… [--as 别名] [--wait]` | 打开检材；有案件时登记进案件并在后台建立卷索引 |
| `evidence ls` / `evidence show [别名]` | 检材与别名 / 一份检材的格式、大小、分区与采集元数据 |
| `evidence close <别名>` | 在当前进程中关闭检材（不改动案件），同时关闭由它派生的检材 |
| `evidence rm <别名>` | 从案件中移除检材及其卷索引、搜索与分析结果；先确认 |
| `volumes [别名]` | 分区与卷：文件系统、加密状态与索引状态 |
| `index ls` / `index build <卷>` / `index rm <卷>` | 卷索引的列表 / 构建（后台运行）/ 删除 |

**后台进程。** `case open` 为案件启动一个后台 `lovelycarve-cli` 进程，它持有案件、检材句柄、读取缓存、解锁的卷、组装的阵列、雕刻扫描和挂载。之后指向该案件的命令（以及 `shell`、`batch`）都交给它执行，原始字节分块传回，进度和事件照常显示。同一个案件同一时间只由一个进程打开；后台进程运行时不要让桌面端打开同一案件。`case close` 等待仍在运行的工作，取消其余任务，释放检材后退出；关闭前已经失败、但还没有报告的后台任务在此时以 `tasksFailed` 报告。

没有后台进程时，每条命令打开案件和检材、执行、等待自己启动的后台任务后退出。只在进程存活期间有意义的命令——`unlock`、`raid assemble`、`carve start`、`vss open`、`mount`——此时以 `needsBackend` 拒绝：先 `case open`，或在 `shell` / `batch` 中一并执行。

后台进程意外结束（崩溃、被结束、主机重启）时，记录它的文件锁随之释放：下一条命令清理残留记录并以 `caseBackendExited` 失败一次，之后的命令各自运行，直到再次 `case open`。解锁、组装、扫描和挂载属于旧进程，需要重新建立。后台进程只监听本机回环地址，用私有随机凭据认证，`case status` 不显示凭据；连接记录与日志在用户数据目录的 `cli-backends/`（测试可用 `LOVELYCARVE_BACKEND_DIR` 隔离）。主机路径在输入命令处解析为绝对路径，再交给后台进程。

**检材与卷索引。** 有案件时，`evidence add` 把检材登记进案件并在后台为各卷建立索引；`--wait` 等待索引与识别任务并报告各卷状态。`volumes` 与 `index ls` 的 `state` 为 `ready` 时索引覆盖该卷；`building` 尚未完成，`stale`、`truncated` 不代表卷的当前全部内容。缺失或过期的卷用 `index build /<别名>/<卷>` 构建，再 `tasks wait` 并核对。目录与归档检材整体是一个卷，索引显示在 `/<别名>`。没有案件时索引只在本进程内使用，退出即丢弃。

**链接。** `case link` 输出的链接按案件 uid、检材 ID 与节点 ID 指向位置，不含主机路径，例如 `lovelycarve://open?case=<案件 uid>&evidence=<检材 ID>&node=ntfs:part:0:42&offset=256&length=12`；格式与桌面端行为见使用指南的[链接](USAGE.md#链接)。`--json` 结果另给出 `path`、`nodeId`、`offset` 与 `length`。桌面端打开链接时要打开该案件，案件后台进程运行时先 `case close`。

案件中原始检材缺失时（`indexOnly: true`），仍可按索引浏览已索引的目录，读取字节的命令明确报错，不会自动启动快照、哈希或平台识别。锁定的案件以 `caseLocked` 拒绝改名、添加或移除检材、手动指定平台以及构建或删除卷索引。案件检材上任务的开始与结局写入案件审计（`task.started` / `task.settled`）。

## 输出与退出码

**标准输出**只放结果：

- 默认是给人看的文本：列表为对齐的表格并在末尾提示如何继续翻页，单个对象为「字段 值」，`hash` 为 `SHA256 (<地址>) = <摘要>` 形式，可直接用于核对。
- `--json` 输出一份 JSON：标准输出是终端时缩进显示，管道和重定向中为单行。结构字段统一为 camelCase；检材中的数据库列名、插件字段名等保持原样。列表结果中 `nextCursor`、`total`、`truncated` 写在 `items` 之前。
- `--json=字段1,字段2` 只保留列表条目的这些字段，点路径保持嵌套（`timestamps.modified`），分页与时区字段保留；不适用于文件内容，写错格式在执行前失败。
- `cat`、`carve cat` 输出原始字节，可直接重定向到文件；加 `--json` 时改为 `{"encoding":"hex","data":"…"}`，单次最多 16 MiB。
- 没有返回值的命令标准输出为空，终端中在标准错误显示 `✓ 完成`。按 ID 查找且对象必须存在的命令（`tasks show`、`results show`、`results run`）找不到时以 `notFound` 失败。

**标准错误**放进度、提示和错误：

- 普通命令显示命令名、后端阶段、该阶段的完成量与耗时；终端中原地刷新，管道和日志中输出普通文本，长命令每 5 秒报告一次仍在执行。阶段来自真实后端观察，未知总量不显示百分比。支持 OSC 9;4 的终端（如 Windows Terminal）在标签页或任务栏显示进度，`LOVELYCARVE_NO_OSC_PROGRESS=1` 关闭。
- `-q` / `--quiet` 或 `LOVELYCARVE_QUIET=1` 关闭进度、横幅等非必要输出；`--json` 同样关闭进度。错误与「结果不完整」警告不受影响。
- `--events` 把生命周期与进度事件以 `{"event":"…","payload":…}` 行写到标准错误：`cli-command`（`command`、`commandId`、`phase`、`elapsedMs`、`stage`）、`cli-error`、`cli-warning`、任务事件，以及 `image-load-progress`、`raid-detect-progress` 等后端事件。启用后不显示人类进度行；`-q --events` 适合脚本。
- 错误显示为带边框的块：命令行与本程序的错误为 `┌ 错误`，后端命令失败为 `┌ 后端错误`、`│ [错误代码] 说明` 与一行 `IPC <后端命令>`；设置 `DEBUG=1` 时附带详情。加 `--json` 时改为一行 `{"error":{"code":"…","message":"…","details":…,"operation":"…"}}`，不完整结果的警告为一行 `{"warning":{…}}`。
- 只有输出是终端且不在 CI 中时才着色，`NO_COLOR` 关闭、`FORCE_COLOR` 强制。后端诊断默认过滤到 `error`，`RUST_LOG=lovelycarve_lib=debug` 等可放宽。

**退出码**：`0` 成功且结果完整；`1` 失败（命令行错误、后端错误、找不到对象、未确认或拒绝的操作、后台任务失败）；`2` 命令完成但结果不完整；`130` 被 Ctrl+C / Ctrl+Break 中断，或在提问时按 Esc / Ctrl+C 取消。

结果不完整时结果照常输出，随后在标准错误给出 `┌ 警告` 块（`--quiet` 不关闭），列出说明不完整的字段：

| 命令 | 不完整的字段 |
| --- | --- |
| `hash`、`export` | `readFaultsDetected`、`syntheticBytes`（以零填充了读不出的源扇区）、`truncated`、`cancelled` |
| `evidence hash`、`evidence export` | `readFaultsDetected`、`syntheticBytes` |
| `grep` | `summary.readErrors`、`summary.truncated`（有文件读不出，或搜索本身达到上限） |
| `find` | `unindexed`（范围内有卷没有索引） |
| `sqlite query` | `shortenedCells`（长文本被缩短或无效 UTF-8 被替换） |
| `carve export` | 失败文件、合成字节、报告写入失败 |

`--limit` 等调用方给出的上限不算不完整。错误代码与桌面端共用，由错误类型决定：`notFound`、`invalidInput`、`alreadyExists`、`cancelled`、`corruptData`、`unsupportedFormat`、`resourceLimit`、`permissionDenied`、`sourceChanged`、`caseLocked`、`readFaultsDetected`、`internal` 等；CLI 另有 `usage`、`noImage`、`needsBackend`、`aborted`、`tasksFailed`、`indexUnavailable`、`caseBackendExited`、`caseBackendRunning` 等。

## 参数

- 位置参数是命令作用的对象（地址、检材路径、扫描 ID……），其余用 `--选项`。多个值用逗号写在一个选项里（`--algo md5,sha256`、`--types png,pdf`），或重复选项（`--member disk1 --member disk2`），以帮助中的写法为准。
- 大小可带单位：`4096`、`64K`、`10M`、`2G`。时间（`timeline --from`）接受 Unix 秒、RFC 3339 或日期 `2024-06-01`。
- 主机路径在输入命令的位置解析为绝对路径。`assembled://…`、`decrypted://…` 等派生检材引用保持原样。
- **密码与密钥从不写在命令行上**：`--password @文件` 读取文件并去掉末尾换行，`--password -` 读取标准输入，单写 `--password` 时在终端中以 `•` 遮盖输入。在 `batch` 中只能用 `@文件`。密码不写入案件、任务描述或日志，`--dry-run` 显示为 `<redacted>`。
- `--dry-run` 显示解析后的请求，不打开检材、不执行。
- 删除或丢弃数据的命令（`case rm`、`evidence rm`、`index rm`、`search rm`、`carve rm`、`plugin rm`、`tasks rm`、`umount`）在终端中先确认；没有人可确认时必须加 `-y`，否则显示 `Aborted.` 并以 1 退出。
- 写主机文件的命令（`-o`）要求目标不存在：已存在时以 `alreadyExists` 失败，不写入任何内容；重跑批量导出请使用新的目标。文件先在目标目录暂存，成功后才发布；失败的导出不留下半成品。

## 浏览与读取

| 命令 | 作用 |
| --- | --- |
| `ls [地址] [-l] [--limit N] [--cursor 游标] [--all] [--sort 键] [--desc] [--deleted \| --no-deleted] [--ext 扩展名]` | 列目录，每页默认 200 条（1–5000）；`nextCursor` 非空时用 `--cursor` 继续，`--all` 读取全部页；`-l` 同时显示节点 ID、inode 与全部时间；排序与过滤在后端执行，目录总在文件之前。`ls /` 列出检材，`ls /<别名>` 列出分区与卷；对文件 `ls` 显示该文件 |
| `tree <地址> [--depth N] [--name 文本] [--limit N]` | 递归列出目录树：深度默认 2（上限 128），`--name` 忽略大小写包含匹配，最多检查 `--limit` 个条目（默认 10000）；`truncated` 为真表示遍历提前停止 |
| `stat <地址> [--content]` | 属性：名称、类型、大小、inode、时间、删除状态、链接目标、节点 ID 与上级地址；`--content` 加上 MIME、魔数、熵与数据运行段 |
| `cat <地址> [--offset N] [--length N] [--member 成员] [--stream 流名]` | 原始字节；`--member` 读取 ZIP 成员，`--stream` 读取命名数据流 |
| `text <地址> [--encoding 标签] [--offset N] [--max-bytes N]` | 识别编码并解码为 UTF-8，每段最多 2 MiB 源字节；`nextOffset` 非空时用 `--offset` 继续，段末被截断的字符留给下一段，后续段沿用文件开头识别出的编码；`--encoding gbk` 等 WHATWG 标签跳过识别 |
| `strings <地址> [--encoding ascii\|utf-8\|utf-16le\|utf-16be] [--min-length N] [--offset N] [--length N] [--limit N]` | 可打印字符串及其文件内偏移 |
| `hash <地址>… [--algo md5,sha1,sha256,sm3]` | 文件哈希，默认 MD5、SHA-1、SHA-256 |
| `identify <地址>` | 按内容识别类型（Magika），不看扩展名 |
| `streams <地址>` | NTFS 命名数据流、HFS+ 资源分叉，以及 BeFS / GFS2 / OCFS2 的扩展属性 |
| `export <地址>… -o <输出> [--zip] [--stream 流名]` | 导出文件，目录导出为目录树（`--zip` 为 ZIP）；多个地址导出到一个新目录，各自按名称放入 |
| `bodyfile <卷或目录> -o <文件>` | 导出 mactime body 文件 |

`hash` 采用容错并报告策略：读到坏扇区时仍返回摘要，结果中的 `readFaultsDetected`、`syntheticBytes`、`badRegions` 描述以零填充的源盘区间，并以 2 退出；该摘要不代表完整原文件，`truncated: false` 也不表示没有读故障。报告覆盖本次操作命中的源盘绝对区间；格式定义的稀疏零区不算读故障。`cat` 的字节流不能附带审计，因此遇到读故障时以 `readFaultsDetected` 失败（退出码 1），标准输出中可能已有部分数据，必须丢弃；需要保留容错数据与审计报告时用 `export`。

浏览命令优先查询案件卷索引或本进程的临时索引，未被索引覆盖的目录回退到快照或实时读取。`ls`、`tree`、`stat` 按[文件时间与时区](#文件时间与时区)显示时间。

**结构化文件。** `inspect <地址>` 解析 SQLite、Realm、ZIP、CSV、可执行文件（PE / ELF / Mach-O / ar）、GGUF、Vim 交换文件与 KeePass KDBX，`--as` 在类型不明确时指定解析器。KDBX 不给凭据时输出外部头部（版本、加密算法、压缩、密钥派生参数、种子与头部 SHA-256）；`--password @文件`（或 `-`、终端输入）、`--keyfile <主机文件>`、`--transformed-key @文件` 解密并输出群组、条目、历史版本、附件与元数据，受保护的值按明文输出。凭据错误以 `invalidInput` 失败，见 [KDBX](formats/KDBX.md)。SQLite 另有：

- `sqlite tables <db>` 列出表、视图与索引的结构和行数；`sqlite rows <db> <表> [--order-by 列] [--desc] [--offset N] [--limit N]` 分页读取记录；`sqlite deleted <db>` 从空闲页、日志与 WAL 恢复已删除的记录。
- `sqlite query <db> "<SQL>" [--param 值]… [--offset N] [--limit N]` 执行单条只读 `SELECT` 或 `WITH … SELECT`，`?1`、`?2`… 按顺序绑定 `--param`；禁止写操作、`ATTACH`、`PRAGMA` 和扩展加载。单页最多 100 行，有后续时用相同 SQL、参数和返回的 `nextOffset` 继续，应使用能唯一排序的 `ORDER BY`。`columns` 按 SELECT 顺序对应单元格，整数编码为十进制字符串以保留 64 位精度。
- 同目录的 WAL 自动读取（`sidecars` 列出用到的文件）。SQLCipher 数据库用 `--passphrase @文件` 或 `--raw-key @文件`，必要时配合 `--cipher-compat`、`--cipher-page-size`、`--kdf-iter`。

```powershell
lovelycarve-cli -C "D:\cases\nas" sqlite query /nas/part:0/data/msgstore.db "SELECT chat_id, COUNT(*) FROM messages WHERE sent_at >= ?1 GROUP BY chat_id ORDER BY chat_id" --param 2026-08-01 --limit 25
```

`ocr <图片>` 识别文字，`qr <图片> [--expect N]` 识别二维码，需要完整构建。

## 查找

**按文件名查找。** `find <通配符>` 在卷索引中按完整文件名做忽略大小写的匹配：`*`、`?`、`[abc]`、`[a-z]`、`[^abc]`；`%`、`_` 是普通字符，`[[]` 匹配字面的 `[`；不展开花括号，`**` 不表示递归目录。`--under <地址>` 限定检材、卷或目录，`--type file|dir|symlink`、`--deleted` / `--no-deleted`、`--min-size` / `--max-size` 进一步筛选；条件在分页前生效。每个卷最多返回 `--limit` 条（默认 200），`truncated` 表示有卷达到上限，用 `--offset` 继续。范围内没有索引的卷列在 `unindexed` 中并使结果不完整；一个可查的索引都没有时以 `indexUnavailable` 失败。

```powershell
lovelycarve-cli -C "D:\cases\nas" find "auth*.log.*" --under /nas/part:0/var/log --type file
```

**按内容搜索。** `grep <模式> [地址]` 用全局搜索查找文件内容：省略地址时搜索全部检材，地址可以是检材、卷、目录或单个文件。需要案件和可读的原始检材，命中保存在案件中。默认区分大小写并包含已删除文件：`-i` 忽略大小写，`-E` 按正则匹配，`--no-deleted` 排除已删除文件，`--encoding UTF-8,GBK` 限定编码（默认全部支持的编码）。结果为 `items`（每条带 `path`、`offset`、`context` 和 `searchId`）、`total`、`truncated`、`summary` 与每份检材的 `searches`；`--limit`（默认 1000）只限制本次返回的条数。

| 命令 | 作用 |
| --- | --- |
| `search ls [--evidence 别名]` | 案件中保存的搜索 |
| `search hits <搜索ID> [--offset N] [--limit N] [--keyword 文本] [--distinct file\|sha256]` | 分页读取一次搜索的全部命中 |
| `search export <搜索ID> -o <新 CSV>` | 把命中导出为 CSV |
| `search rm <搜索ID>` | 删除一次搜索及其命中 |

`journal <卷或卷中的文件> [--limit N] [--cursor 游标]` 分页读取 NTFS USN 日志或 ext4 jbd2 日志块。

## 文件时间与时区

`ls`、`tree`、`stat` 转换时间用于显示；文件字节、哈希、案件索引、插件结果与时间线保持原值。默认 `auto` 依次使用检材的显示设置、案件的显示设置、检材内的系统时区，最后回退 UTC；不会采用运行 CLI 的计算机时区。

| 用法 | 作用 |
| --- | --- |
| `--tz <时区>` | 只对这条命令生效：`auto`、`evidence`（强制检材配置，无法识别或有歧义时报错）、`case`（强制案件设置，未设置时报错）、`UTC`、IANA 名称（按每个日期的历史偏移与夏令时）或固定偏移 `+05:45` / `-04:00`；无效时区在打开任何检材之前失败 |
| `tz [地址]` | 有效时区、来源、配置路径、候选与告警 |
| `tz set <时区>` / `tz set <时区> /<别名>` | 保存在案件中：前者对整个案件，后者只对该检材；`auto` 清除对应设置 |

自动识别只探测同一检材内的 Linux / NAS 文件系统根，以及 Btrfs 下一层至多 32 个目录 / 子卷，不递归扫描用户数据。优先读取 `/etc/localtime`：链接中的 `zoneinfo/<IANA 名称>` 直接识别，并优先使用检材 `/usr/share/zoneinfo/` 内的原始 TZif 规则；复制的 TZif 文件按内容解析，标识为 `TZif:sha256:…`。其次检查 `/etc/timezone`、`/etc/sysconfig/clock` 的 `ZONE` / `TIMEZONE`、`/etc/TZ`、OpenWrt `/etc/config/system` 的 `zonename`。文本配置上限 4 KiB、TZif 上限 1 MiB；只在检材内解析路径，`/etc`、`/usr` 等可为检材内的相对、绝对或链式符号链接，每次查找最多 40 次跳转。越过系统根的 `..`、主机盘符 / UNC 路径、断链和循环链接报告为读取限制，不会读取主机上的同名配置。配置冲突或多个系统时区不一致时不随意选第一个；回退的 UTC 只表示显示回退，不表示检材原来使用 UTC。

节点的 `timestamps` 是显示结果；转换后与原值不同时附带 `timestampsOriginal`。`stat` 的 `timestampDisplay.timeZone` 记录所选时区、来源与告警；列表把同一时区合并到顶层 `timestampDisplay.timeZones`，混合时区时条目的 `timestampTimeZone` 是该数组的索引。FAT/exFAT 等没有已知偏移的本地时间和 `-00:00` 保持原值，列入 `timestampDisplay.unconvertedFields`（列表条目为 `timestampUnconvertedFields`），不会补成 UTC。

## 加密卷

| 命令 | 作用 |
| --- | --- |
| `encryption <地址>` | 加密类型与状态：BitLocker 元数据（算法、卷 GUID、FILETIME、描述、保护器类型，不含密钥）、FileVault 逻辑卷目录、LUKS 状态 |
| `unlock <地址> [凭据] [--scheme bitlocker\|luks\|veracrypt\|filevault\|ios]` | 解锁；BitLocker、LUKS、VeraCrypt 与 FileVault 走同一条解锁路径，类型按分区识别，VeraCrypt（没有签名）须指定 |
| `evidence export <别名> -o <新文件> --unlocked` | 整盘导出，已解锁的分区写成明文，其余字节保持源数据 |

解锁只在进程存活期间有效，需在后台进程、`shell` 或 `batch` 中进行；解锁的卷和凭据不保存为案件检材，后台进程重启后需重新解锁。`tasks cancel <任务ID>` 可取消解锁，KDF 执行中可能延迟响应。锁定的分区列为 `unsupported:N`，也可以写 `part:N`；`volumes` 显示每个加密卷的格式与锁定状态。

- **BitLocker**：只给一种凭据 `--password`、`--recovery-password`、`--startup-key <.BEK>`、`--fvek`（Elephant Diffuser 另给 `--tweak`）或 `--vmk`；都不给时尝试卷内明文密钥。`--vmk-from <内存镜像>` 在内存中搜索 VMK 并逐个尝试，卷没有恢复密码保护器时也可解锁；结果的 `vmkOffset` 是生效的候选，`recoveryPassword` 是由它派生的恢复密码。原始 FVEK 没有认证，须核验解密结果；TPM / SID 保护器不支持。明文卷按卷首识别文件系统（NTFS、ReFS、FAT / exFAT 等），之后照常浏览。能力与限制见 [BitLocker](formats/BITLOCKER.md)。
- **LUKS**：LUKS1 / LUKS2 用 `--password`，或已恢复的卷主密钥 `--master-key`（按头部摘要核验）。整份镜像的 LUKS 容器显示为 `unsupported:0`。见 [LUKS](formats/LUKS.md)。
- **VeraCrypt / TrueCrypt**：`--scheme veracrypt`，按需给 `--pim`（卷 0–2147468，系统加密 0–65535）、`--keyfile <文件或文件夹>`（可重复；只用密钥文件时省略 `--password`）、`--prf sha512|sha256|blake2s|whirlpool|streebog|ripemd160|argon2id` 与 `--header auto|normal|hidden|backup|system`。默认 `auto` 同时尝试标准卷与隐藏卷；`backup` 读取卷内嵌的备份卷头；`system` 从磁盘第 62 扇区读取系统加密卷头，地址给被加密的分区（`/<别名>/part:N`）时明文接在该分区上，给整份检材时生成整盘明文的新检材。结果的 `veracrypt` 字段给出 PRF、迭代次数、算法、卷头与加密是否完成。VeraCrypt 卷总是整数个扇区，区域长度不是检材扇区大小整数倍时直接拒绝（`invalidInput`）；较大文件中的容器用 `--offset` / `--length` 指出它的范围。见 [VeraCrypt](formats/VERACRYPT.md)。
- **FileVault 2**：先用 `encryption` 列出逻辑卷，再用 `--logical-volume <index>` 解锁；`--password` 也接受个人恢复密码，也可给 `--master-key`；`--plist` 指向本地 EncryptedRoot.plist(.wipekey)，`encryption` 与 `unlock` 应给同一个。多个逻辑卷须各自用 `--new-image`。APFS 原生加密不能由此解锁。见 [FileVault](formats/FILEVAULT.md)。
- **iOS 备份**：`unlock /<别名> --password @文件`（`--scheme ios`）。见 [iOS 备份](IOS_BACKUPS.md#cli)。

BitLocker、LUKS、VeraCrypt 与 FileVault 默认把明文接在源检材上：结果的 `path` 指向可读的卷（如 `/lk/part:0`），源检材 ID 不变，案件随后为它补建索引。`--new-image [--as 别名]` 改为注册一份独立的解密检材（`decrypted://…`），关闭源检材时一并关闭；`--offset` / `--length`（相对源检材的字节）选择非分区区域，总是生成独立检材。明文接在源检材上时，`evidence hash` 和普通 `evidence export` 仍读取源密文，`evidence export <别名> --unlocked` 才按导出开始时的解锁状态把各已解锁分区的明文写进原分区位置（较短明文之后补零，分区表、其他区域及整盘大小保留）。不支持的布局（分离头部、LUKS2 完整性 / 重加密 / 多数据段）和超过 KDF 资源预算时明确报错。

```powershell
lovelycarve-cli case open "D:\cases\laptop"
lovelycarve-cli -C "D:\cases\laptop" encryption /laptop/part:2
lovelycarve-cli -C "D:\cases\laptop" unlock /laptop/part:2 --recovery-password "@D:\keys\recovery.txt"
lovelycarve-cli -C "D:\cases\laptop" ls /laptop/part:2
lovelycarve-cli -C "D:\cases\laptop" evidence export laptop -o "D:\output\laptop-decrypted.raw" --unlocked
```

## RAID、LVM 与卷影副本

完整的单 PV LVM 卷可直接在源检材的分区下浏览（`/<别名>/lvm:…`），不必组装。跨盘阵列先把各成员盘登记进同一案件，再在后台进程中运行 `raid detect`：读取 LVM、md 与固件 RAID 元数据，组装完整的阵列，并把每个组装出的阵列或逻辑卷注册为带别名的检材（结果的 `assembled[].path`）；再次运行复用已组装的结果。不完整或不支持的卷不会组装，原因见 `diagnostics`。无元数据的等大成员盘按 RAID0 / 5 推定。

- 克隆主机可能有相同的 VG/PV UUID：`raid detect --member <别名>`（可重复）只检测这些源检材，可在同一进程内分别组装各主机；组装出的检材和卷影副本不能作为成员。
- 成员缺失或过期的 md 阵列等待确认：`raid detect --degraded <setUuid>` 降级组装该阵列。
- `raid assemble --members a,b,c --level 5 [--layout] [--chunk-sectors] [--data-offset-sectors] [--missing 列] [--as 别名]` 按给定几何手动组装，缺失的列写 `-`。
- `assembled://` 引用只在组装它的进程中有效；后台进程结束后需重新检测。

LVM 卷中的文件节点 ID 形如 `btrfs:lvm:<分区>:<卷索引>:<子卷>:<inode>`，由目录枚举给出，不能从卷名推测；复制返回的地址或 ID。`vss ls /<别名>` 列出 NTFS 卷影副本，`vss open /<别名>/part:N <序号> [--as 别名]` 把一个副本打开为只读检材（见 [卷影副本](formats/VSS.md)）。

## 文件恢复

文件系统记录恢复与签名雕刻是同一次扫描的两个阶段，由 `carve start` 在后台运行，需在后台进程、`shell` 或 `batch` 中进行；扫描记录只在进程内存中，应在同一进程中扫描、读取和导出。

| 命令 | 作用 |
| --- | --- |
| `carve start [别名] [--fs fat,ntfs,exfat] [--existing] [--types png,pdf] [--no-signatures] [--scope whole\|unallocated\|partition\|range]` | 开始扫描；`--fs` 启用对应文件系统的记录恢复（默认只做签名雕刻），`--existing` 同时列出已知分区的现存文件；`--partition`、`--start-sector`、`--sectors` 选择范围；`--max-file-size`、`--max-results` 限制雕刻结果 |
| `carve status <扫描ID>` | 阶段、进度与摘要；`filesystem` 给出卷 / 目录 / 文件数、截断、读取故障与阶段错误 |
| `carve results <扫描ID> [--format] [--category] [--min-confidence] [--cursor] [--limit]` | 雕刻出的文件 |
| `carve results <扫描ID> --fs` | 文件系统恢复出的记录：目录在 `directories`，文件在 `candidates`；该阶段结束前为空 |
| `carve cat <扫描ID> <结果ID>` | 一个雕刻结果的内容 |
| `carve pause` / `carve resume` | 暂停 / 继续；暂停的扫描须先继续或取消，`tasks wait` 才会返回 |
| `carve export <扫描ID> -o <新目录> [--artifacts ID,…] [--no-artifacts] [--files 编号,…]` | 导出：默认全部雕刻结果，`--files` 加入可读的文件系统候选 |
| `carve ls` / `carve formats` / `carve rm <扫描ID>` | 扫描列表 / 可雕刻的格式 / 删除已结束的扫描 |

导出的文件系统记录按 `v<卷号>-<卷标>/原目录/文件名` 保持目录结构，每个文件的状态、来源、`relative_output` 与 SHA-256 写在 JSONL 报告中。NTFS 重建卷的 `reconstruction` 给出 MFT 位置与推导的几何；回收站还原项带 `sourceNode` / `sourcePath`。`--files` 只接受本次扫描中可读的候选编号，不接受任意数据映射。能力与限制见[文件恢复](USAGE.md#文件恢复)。

```powershell
lovelycarve-cli -C "D:\cases\usb" carve start usb --fs fat,exfat --no-signatures
lovelycarve-cli -C "D:\cases\usb" tasks wait
lovelycarve-cli -C "D:\cases\usb" carve results "<扫描ID>" --fs
lovelycarve-cli -C "D:\cases\usb" carve export "<扫描ID>" -o "D:\output\recovered" --no-artifacts --files 0,3
```

## 插件与分析结果

| 命令 | 作用 |
| --- | --- |
| `plugin ls` / `plugin rm <插件ID>` / `plugin readme <插件ID>` | 插件包（含说明、发布者与许可证）、卸载、说明文档 |
| `plugin inspect <插件包>` | 检视 `.lcpkg`：标识、版本变化、权限、分析器与压缩包和库文件的 SHA-256，不安装 |
| `plugin install <插件包>` | 先检视，再安装同一份字节；需要确认（`-y`），暂存的库通过启动校验后才替换已安装的版本 |
| `plugin market [sources]` / `plugin market add <索引源>` / `plugin market rm <索引源>` | 插件市场索引源：`https://` 地址，或本地市场目录、`index.json` 的绝对路径 |
| `plugin market ls [--source 索引源] [--refresh]` | 一个索引源的插件、最新版本与安装状态；省略 `--source` 取第一个 |
| `plugin market install <插件ID> [--version 版本] [--source 索引源]` | 下载当前平台的插件包，按索引校验大小与 SHA-256，检视后安装（需要确认）；省略版本取最新正式版 |
| `plugin market index <目录> --title <标题> [--description 说明]` | 为目录中的 `.lcpkg` 写出 `index.json` |
| `analyze ls [--evidence 别名]` | 分析器（作用域、平台与选项）及检材上识别出的系统（ID 如 `linux@part:2`） |
| `analyze run <分析器> [--system 系统ID] [--file 地址] [--option 键=值]… [--options-json JSON]` | 创建插件任务：系统级给 `--system`，文件级给 `--file`，检材级都不给；选项值是 JSON 时保持类型，省略的取默认值 |
| `results ls` | 结果树：检材 → 分类 → 分析器运行 → 记录类型，带 `runId` |
| `results rows <运行ID> <记录类型> [--keyword] [--sort 列] [--desc] [--offset] [--limit] [--legacy]` | 分页读取记录；同一次分页固定 runId，新任务完成不会切换 |
| `results show <记录ID>` / `results run <运行ID>` | 单条记录 / 一次运行的输入、选项、画像事实、告警与补零的源字节 |
| `results import [别名] [--replace]` | 把 BCP 数据包发布为分析结果 |

同一分析器、检材与目标最多一个未结束的任务，重复创建返回已有任务。插件任务不能暂停：`tasks cancel plugin:<n>` 结束执行，`tasks restart plugin:<n>` 从头再跑，成功后新结果替换旧结果；`tasks rm plugin:<n>` 删除任务及其发布的结果。旧版本插件写下的结果在树中带 `legacy: true`，读取时加 `--legacy`。

```powershell
lovelycarve-cli -C "D:\cases\nas" analyze run org.lovelycarve.linuxinfo/linuxbaseinfo --system linux@part:2 --option 'items=["users","logins"]'
lovelycarve-cli -C "D:\cases\nas" tasks wait
lovelycarve-cli -C "D:\cases\nas" results rows "<运行ID>" linux.user --sort uid --desc
```

**BCP 数据包。** 把 BCP ZIP、已解包的 BCP 目录或只含一个 BCP ZIP 的外层目录作为检材登记进案件，再 `results import`：ZIP 根目录须有 `GAB_ZIP_INDEX.xml`，按索引顺序合并同表分片，无需解包或构建卷索引。在结果树「BCP 取证结果」下查看 `artifactType` 与记录数；记录的 `sources` 保留分片及源内位置。记录类型的 `bcpGroups` 列出原始应用编码、账号、总数和已删除数，`identifiedApplication` 为通过导出元数据识别的应用；`results rows --bcp-app <编码> --bcp-account <账号>` 按分组查询（空字符串匹配无账号的记录）。重复导入复用上一次发布的运行，`--replace` 重新读取源件并替换。

## 时间线

| 命令 | 作用 |
| --- | --- |
| `timeline build [--evidence 别名]` | 构建或重建时间线（需要案件）；不运行分析器，需要其记录的先运行对应分析器 |
| `timeline status` | 构建状态、时间范围与事件类型 |
| `timeline ls [--from] [--to] [--app 应用]… [--type 编号]… [--text 文本] [--deleted] [--offset] [--limit] [--desc]` | 分页列出事件 |
| `timeline stats [同上的过滤] [--by year\|month\|day\|hour\|minute\|second] [--utc-offset 小时]` | 按时间桶计数；`--utc-offset` 是统计用的固定偏移，与显示时区 `--tz` 无关 |
| `timeline export [同上的过滤] -o <新 CSV>` | 导出事件 |

没有已知偏移的时间不会投影为确定时刻；缺少事件不能证明没有发生活动。

## 后台任务

后台任务是命令返回后仍在运行的任务（`carve start`、`index build`、插件任务），以及检材登记到案件后自动开始的卷索引、平台识别与插件任务。

- 一次性命令输出结果后等待本进程的后台任务，任一失败或结束时没有报告结果时显示 `tasksFailed` 并以 1 退出；取消不算失败。无案件检材的临时索引只是缓存，退出时直接取消。
- `tasks ls [--evidence 别名]` 与桌面任务中心列出同一份任务：插件任务（ID 为 `plugin:<n>`）与内置任务（种类、对象、阶段、进度、计数与结局）。没有后台进程时，一个进程只看得到自己的内置任务。
- `tasks show <ID>` 查看一条，找不到时以 `notFound` 失败；`tasks cancel <ID>` 取消运行中的任务，内置任务在下一个检查点停下，找不到或已结束的任务报错。
- `tasks wait` 在没有活动任务时返回 `{"tasks":[…]}`：上次报告以来结束的后台任务及其 `status`；任一失败时以 `tasksFailed` 失败，取消的任务列出但不算失败。扫描、搜索和分析的结果用各自的命令读取。

## 交互 shell

```text
lovelycarve-cli -C D:\cases\nas shell
lovelycarve:/> cd /nas/part:0/home
lovelycarve:/nas/part:0/home> ls
lovelycarve:/nas/part:0/home> tree alice --depth 3 --name report
lovelycarve:/nas/part:0/home> stat alice/report.txt
lovelycarve:/nas/part:0/home> export alice/report.txt -o "D:\output\report.txt"
lovelycarve:/nas/part:0/home> exit
```

`shell` 在一个进程中逐行执行命令，语法与命令行相同（省略程序名），案件、检材、解锁与扫描在命令之间保持；案件有后台进程时每行交给它执行。`cd <地址>` 切换目录（也接受 `/<别名>/@<ID>`），`pwd` 显示当前目录，`help` 显示帮助，`exit` 退出；相对地址从当前目录开始，`ls`、`grep` 省略地址时、`find` 省略 `--under` 时作用于当前目录。引号只负责分组，Windows 反斜杠保持原样，不做变量展开或管道。`-C`、`--image` 只能在启动 shell 时给出。

标准输入不是终端时，`shell` 执行传入的脚本：忽略空行和 `#` 注释，不显示提示符，遇到第一个失败的命令即停止并以 1 退出；有不完整结果时以 2 退出。脚本中的密码须用 `@文件`。

## 批量请求

`batch` 从标准输入逐行读取请求，每行写一行响应。请求就是命令行本身：

```jsonl
{"id":"add","argv":["evidence","add","D:/evidence/disk.img","--as","disk"]}
{"id":"roots","argv":["ls","/disk"]}
{"id":"scan","argv":["carve","start","disk","--types","png,pdf"]}
{"id":"done","argv":["tasks","wait"]}
{"id":"status","argv":["carve","status",{"$ref":"scan#/scanId"}]}
{"id":"recover","argv":["carve","export",{"$ref":"scan#/scanId"},"-o","D:/output/carved"]}
```

```powershell
Get-Content .\workflow.jsonl | lovelycarve-cli -C "D:\cases\demo" batch --jobs 4
```

- 每行一个对象：`id`（非空且唯一）、`argv`（命令行各词，与终端中输入的相同）和可选的 `output`（把结果写入新的主机文件，响应为 `{"output":"<路径>"}`）。旧格式的 `command` / `args` 请求以 `usage` 错误提示改写为 `argv`。
- 响应为 `{"id":…,"result":…}` 或 `{"id":…,"error":{"code","message","details"}}`；结果不完整时在 `result` 后附 `"warning"`。能解析出字符串 ID 的无效请求保留该 ID，无法解析的行使用 `"id":null`。
- `argv` 中的 `{"$ref":"<id>#/<JSON 指针>"}` 取先前成功结果中的值；引用尚在执行的请求时先等待它完成，失败、未知 ID 和不存在的指针明确报错。
- 请求中可用 `--json=字段`、`--tz`、`-y`、`--dry-run`；`-C`、`--image`、`-q`、`--events` 属于 `batch` 进程本身，写在 `batch` 之前。删除类命令需要 `-y`，否则返回 `aborted`；密码只能用 `@文件`。
- 原始字节返回 `{"encoding":"hex","data":"…"}`，单次最多 16 MiB，用 `--offset` / `--length` 分段，或用 `output` 写入文件。
- `--jobs N`（简写 `-j`，1–64，默认 1）允许只读查询重叠执行；其余命令是屏障，先等此前的请求完成。并发时响应按完成顺序输出，按 `id` 关联。
- 案件有后台进程时，每个请求交给它执行；否则 `batch` 进程自己持有案件和检材，解锁与扫描在输入结束前一直有效。
- 标准输入保持打开时也持续返回已完成的结果；输入结束后等待已派发的请求和后台任务再退出。单条失败不影响后续请求，但以 1 退出；没有失败而有不完整结果时以 2 退出。单行上限 16 MiB，一次最多 10000 个请求。

## rpc 与 schema

`rpc <后端命令> --body <JSON>` 按名称调用任意后端命令，参数名与桌面端 IPC 相同（camelCase），`--body` 可写 `@文件`；`imageId` 也可以写检材别名。`rpc --list` 列出全部后端命令及参数。它用于没有专门命令的功能，例如目录大小树、iOS 备份审计、插件相对路径定位或手动指定平台：

```powershell
lovelycarve-cli -C "D:\cases\nas" rpc fs_size_tree --body '{"imageId":"nas","nodeId":"part:0"}'
lovelycarve-cli -C "D:\cases\nas" rpc case_evidence_platform_set --body '{"imageId":"nas","platform":"linux"}'
```

`schema` 列出全部命令，`schema <命令…>` 输出一条命令的参数、选项、默认值、可选值、输出类型（`json` / `bytes`）、是否可并发、是否需要确认、不完整字段和对应的批量请求写法。`completion <bash|zsh|fish|powershell>` 输出补全脚本，候选来自同一棵命令树：

```powershell
lovelycarve-cli completion powershell >> $PROFILE
```

## 挂载

`mount <地址> [挂载点] [--share 共享名]` 把卷或目录以只读方式挂载（Windows，Dokan），`--share` 同时以 SMB 共享；不带参数时列出挂载，`umount <挂载ID>` 卸载（先确认）。挂载只在后台进程存活期间有效，需要长期保留的内容请导出。

## 检材格式说明

- **不完整的分段 RAW**：默认检查可观察到的编号缺口。`evidence add <第一段> --raw-read-mode force` 只拼接从所选文件开始的连续前缀，只在当前进程有效，不能与 QCOW 凭据同用；导出的前缀不表示原始镜像完整。已按另一模式加载时先 `evidence close`。见 [SMRAW](formats/SMRAW.md#不完整-raw-的强行读取)。
- **QCOW**：压缩簇与差分父链自动读取；legacy AES 镜像用 `--qcow-password @文件` 或 `--qcow-key @文件`（16 字节，二选一）。该格式没有密钥验证器，成功打开不证明密码正确。见 [QCOW](formats/QCOW.md)。
- **EWF、VMDK、VHD / VHDX**：纯 Rust 读取器，不依赖 libewf / libvmdk / libvhdi。能力与限制见 [EWF](formats/EWF.md)、[VMDK](formats/VMDK.md)、[VHD / VHDX](formats/VHD.md)。
- **光盘**：ISO、Mode 1 / XA Form 1 的 BIN 和 CUE；多轨或多数据文件选择 `.cue`，`evidence show` 的 `opticalDisc` 给出轨道、会话与来源范围。见 [CDFS](formats/CDFS.md)。
- **UBI 与小米 HDR1 固件**：命名卷列在 `/<别名>/part:0` 下，`<卷名>.img` 提供原始逻辑字节。见 [UBI](formats/UBI.md)。
- **bcachefs**：按列出的目录项进入原生子卷与 snapshot，不同视图中相同的 inode 编号指向各自的内容；只支持单设备。见 [bcachefs](formats/BCACHEFS.md)。
- **NILFS2**：显示最新提交的目录树；稀疏区读零，已分配数据缺失或日志 CRC 不匹配报错，dirty / recovery 卷明确拒绝；CLI 暂不选择历史检查点。见 [NILFS2](formats/NILFS2.md)。
- **BeFS / GFS2 / OCFS2**：内联与外联扩展属性用 `streams`、`cat --stream`、`export --stream` 读取，GFS2 / OCFS2 保留 `user.` 等命名空间前缀；需要日志重放的卷明确拒绝。

## 编码代理

CLI 安装包的 `skills/lovelycarve-cli/` 是供编码代理使用的技能，复制到代理的技能目录即可使用：

| 文件 | 内容 |
| --- | --- |
| `SKILL.md` | 任务入口：地址、先建案件、限制输出与检查完整性 |
| `bin/` | 独立安装时随附的 CLI、运行库、插件加载器与插件，由 `just stage-cli-skill` 生成，不提交到 Git |
| `references/commands.md` | 全部命令的用法与说明，由命令树生成 |
| `references/workflows.md` | 按需读取的流程：案件与后台进程、分页与批量请求、文件定位与解析、内容搜索、RAID/LVM、解锁、恢复、插件、时间线、哈希与导出 |

Windows 独立安装时，先用 `just build-cli` 与 `just build-plugins --release` 构建所需程序，再运行 `just stage-cli-skill`，把现有 release 的 CLI、DLL、插件及许可证放进 `skills/lovelycarve-cli/bin/`。技能按自身位置解析 `bin/lovelycarve-cli.exe`，不依赖固定主机路径或修改 `PATH`。`references/commands.md` 由 `just update-cli-skill` 生成，命令、选项或说明变化后内容过期时后端测试失败；手写页面中的命令行与批量请求同样由测试按命令树解析。代理应使用 `--json`：结果为单行 JSON，进度关闭，错误为一行 JSON，按退出码判断完整性。

## 生命周期与边界

没有后台进程时，每条命令执行后等待它启动的后台任务结束再退出；退出前取消剩余任务、关闭案件、停止插件并删除预览临时文件。Ctrl+C / Ctrl+Break 取消任务、释放案件与挂载后以 130 退出；无法取消的阻塞读取最多再等 10 秒。检材以只读方式打开；案件锁、源读取审计、文件格式限制和任务资源预算与桌面端一致。平台限制沿用后端（例如 Dokan / SMB 挂载仅 Windows）。

## 测试

`just test-cli` 构建开发版 CLI 并运行端到端测试 `scripts/test-cli.py`：读取器夹具、镜像容器与加密卷、批量请求、浏览、SQLite 与 BCP、雕刻、案件、后台进程、地址、输出、事件、时区、读故障、光盘和无窗口启动。`just smoke-cli <可执行文件>` 对现有二进制运行同一套测试；开发时可只跑其中几节，例如 `python scripts/test-cli.py <可执行文件> backend batch`。设置 `LOVELYCARVE_CLI_TEST_IMAGE` 为外部镜像路径时，额外对比顺序与并发的目录查询和有界读取；`BCACHEFS_ORACLE_DIR`、`NILFS2_ORACLE_DIR`、`GFS2_ORACLE_DIR`、`OCFS2_ORACLE_DIR`、`BEFS_ORACLE_IMAGE` 加入对应格式的原生样本。这些外部样本不加入仓库。
