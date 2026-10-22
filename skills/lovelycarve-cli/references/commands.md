# lovelycarve-cli command reference

Generated from the CLI command catalogue by `just update-cli-skill`; do not edit by hand.

Each entry is a command's synopsis and its description. Positional arguments come first, then options; `<…>` is required, `[…]` optional, `a|b` lists the accepted values and `…` marks a repeatable value. A command whose synopsis has both a path and a node ID option (`--node-id`, `--parent-id`, `--root-id`) takes one of them. Run `lovelycarve-cli <command> --help` for option descriptions and defaults, and `lovelycarve-cli schema <command>` for the JSON request.

## Options on every command

| Option | Description |
| --- | --- |
| `--timezone <value>` | 文件时间显示时区：auto、evidence、case、UTC、IANA 名称或 ±HH:MM |
| `--events` | 把命令生命周期与后端进度以 JSONL 写到标准错误 |
| `--session <value>` | 使用命名常驻会话（也可设置 LOVELYCARVE_SESSION） |
| `-i, --image <value>` | 检材 ID 或路径（覆盖 LOVELYCARVE_IMAGE） |
| `--case <value>` | 先打开的案件目录（覆盖 LOVELYCARVE_CASE） |
| `-q, --quiet` | 不输出非必要信息 |
| `-h, --help` | 显示帮助 |
| `-v, --version` | 显示版本 |
| `--dry-run` | 解析参数并显示将执行的请求，不实际执行 |
| `--body <json>` | 原始 JSON 参数对象，键为 API 参数名（见 schema）；可写 @文件路径 |
| `--fields <paths>` | 仅返回列表条目的指定字段，逗号分隔；支持点路径 |
| `--text` | 按 UTF-8 解码原始字节后输出 |
| `--force` | 跳过确认（用于脚本和 CI） |

`--body` applies to commands with parameters and `--fields` to list output; `--text` and `--force` appear in the synopses of the commands that take them.

## Contents

- [analysis](#analysis): 插件分析结果：结果树、记录、详情与运行审计
- [bcp](#bcp): BCP 数据包
- [bitlocker](#bitlocker): BitLocker 卷：状态、元数据、解锁与解密导出
- [carve](#carve): 文件恢复：文件系统记录恢复与签名雕刻
- [cases](#cases): 案件：创建、打开、设置与检材
- [context](#context): 会话上下文：当前检材与镜像内目录
- [disks](#disks): 主机物理磁盘
- [encrypted-volumes](#encrypted-volumes): LUKS、VeraCrypt 与 FileVault 加密容器
- [files](#files): 检材内的文件：按路径或节点浏览、读取、解析与导出
- [filevault](#filevault): FileVault 2（CoreStorage）
- [images](#images): 检材：打开、查看、导出、哈希与识别
- [indexes](#indexes): 卷索引：构建、列出与查询
- [ios-backups](#ios-backups): iOS 本地备份
- [mounts](#mounts): 只读挂载与 SMB 共享（Windows）
- [plugins](#plugins): 插件包、分析器与插件任务
- [raid](#raid): RAID 组装
- [searches](#searches): 全局搜索：任务、命中与导出
- [sessions](#sessions): 命名常驻会话：复用已打开检材、组装卷和当前目录
- [shadow-copies](#shadow-copies): NTFS 卷影副本
- [system](#system): 本程序与主机
- [tasks](#tasks): 进程内的后台任务
- [timeline](#timeline): 时间线：构建、查询与导出
- [timezones](#timezones): 文件时间显示：自动读取检材时区或保存案件、检材与会话设置
- [Root commands](#root-commands)

## analysis

插件分析结果：结果树、记录、详情与运行审计

- `lovelycarve-cli analysis records get <record-id>`
  查看一条分析记录
- `lovelycarve-cli analysis records list --run-id <string> --record-type <string> [--legacy] [--keyword <string>] [--sort <string>] [--descending] [--skip <number>] [--limit <number>]`
  分页列出分析记录。按记录类型分页读取指定 runId 的记录，可按关键词过滤、按列排序；runId 来自分析结果树，分页期间固定运行身份。
  - `lovelycarve-cli analysis records list --run-id <runId> --record-type linux.user --sort uid`: 按 UID 排序列出一次运行的用户记录
- `lovelycarve-cli analysis runs get <run-id>`
  查看一次分析运行的审计信息。运行的目标、输入文件、选项、画像事实、告警与读不出而以零填充的源字节。
- `lovelycarve-cli analysis tree get`
  查看分析结果树。分析结果树：检材 → 分类 → 分析器运行 → 记录类型。旧案件的插件结果以 legacy 节点只读列出。未打开案件时返回空树。

## bcp

BCP 数据包

- `lovelycarve-cli bcp rows list --code <string> [--cursor <json>]`
  分页列出 BCP 数据表记录
- `lovelycarve-cli bcp tables list`
  列出检材中的 BCP 数据表

## bitlocker

BitLocker 卷：状态、元数据、解锁与解密导出

- `lovelycarve-cli bitlocker export <partition-id> --destination <string> [--task-id <string>]`
  导出解密后的 BitLocker 卷
- `lovelycarve-cli bitlocker get <partition-id>`
  查看 BitLocker 卷的锁定状态
- `lovelycarve-cli bitlocker metadata get <partition-id>`
  查看 BitLocker 元数据。读取算法、卷 GUID、FILETIME、卷描述与保护器类型，不需要凭据，也不返回密钥。
- `lovelycarve-cli bitlocker unlock <partition-id> [--recovery-password <secret>] [--password <secret>] [--startup-key-path <string>] [--fvek-hex <secret>] [--tweak-hex <secret>] [--task-id <string>]`
  解锁 BitLocker 卷。用密码、48 位恢复密码、启动密钥（BEK）或 FVEK 解锁；都不传时尝试卷内的明文密钥。
  Keywords: unlock bitlocker volume, decrypt, recovery key, password, bek.
- `lovelycarve-cli bitlocker vmks scan <memory-path> --partition-id <number> [--task-id <string>]`
  在内存镜像中搜索 VMK
  Keywords: memory dump, volume master key.
- `lovelycarve-cli bitlocker vmks unlock <partition-id> --vmk-hex <secret> [--task-id <string>]`
  用 VMK 解锁 BitLocker 卷

## carve

文件恢复：文件系统记录恢复与签名雕刻

- `lovelycarve-cli carve artifacts get <artifact-id> --scan-id <string> [--max-bytes <number>] [--text]`
  读取一个雕刻结果的内容
- `lovelycarve-cli carve artifacts list --scan-id <string> [--cursor <string>] [--limit <number>] [--filter <json>]`
  分页列出雕刻结果
- `lovelycarve-cli carve files list --scan-id <string>`
  列出文件系统恢复结果。文件系统阶段恢复出的卷、目录与候选文件；该阶段结束前返回 null。
- `lovelycarve-cli carve formats list`
  列出可雕刻的文件格式
- `lovelycarve-cli carve scans create --options <json>`
  开始文件恢复扫描。在后台开始文件系统记录恢复和签名雕刻；options 选择范围、格式和文件系统。
  Keywords: recover deleted files, carving, undelete, signature.
- `lovelycarve-cli carve scans delete <scan-id> [--force]`
  删除已结束的恢复扫描。删除扫描会话及其内存结果，已恢复的文件不受影响。
- `lovelycarve-cli carve scans export <scan-id> --options <json>`
  导出恢复出的文件。把雕刻结果和文件系统记录写到 options.destination，并生成 JSONL 恢复报告。
  Keywords: recover, save recovered files.
- `lovelycarve-cli carve scans get <scan-id>`
  查看恢复扫描状态
- `lovelycarve-cli carve scans list`
  列出检材的恢复扫描
- `lovelycarve-cli carve scans pause <scan-id>`
  暂停恢复扫描
- `lovelycarve-cli carve scans resume <scan-id>`
  继续恢复扫描

## cases

案件：创建、打开、设置与检材

- `lovelycarve-cli cases close`
  关闭当前案件
- `lovelycarve-cli cases create <dir> --name <string> [--case-number <string>] [--description <string>] [--examiner <string>]`
  创建案件。在目录中创建案件台账并打开它；已加载的无案件检材会登记进新案件。
  Keywords: new case, investigation.
- `lovelycarve-cli cases delete <dir> [--force]`
  删除案件。删除案件自己的文件（台账、锁、检材数据与暂存区）并注销登记，不触碰原始检材。
- `lovelycarve-cli cases edit --name <string>`
  修改当前案件名称
- `lovelycarve-cli cases evidence delete [image] [--force]`
  从案件中移除检材。删除检材在案件中的登记和派生数据（索引、搜索、时间线等），不触碰原始检材。
- `lovelycarve-cli cases evidence list`
  列出案件中的检材
- `lovelycarve-cli cases get`
  查看当前案件
- `lovelycarve-cli cases list`
  列出本机登记的案件
- `lovelycarve-cli cases lock`
  锁定当前案件
- `lovelycarve-cli cases open <dir>`
  打开案件
- `lovelycarve-cli cases options edit --entries <json>`
  写入案件级设置
- `lovelycarve-cli cases options list [--prefix <string>]`
  列出案件级设置
- `lovelycarve-cli cases unlock`
  解除当前案件的锁定

## context

会话上下文：当前检材与镜像内目录

- `lovelycarve-cli context get`
  查看会话上下文。当前案件、当前检材与镜像内工作目录。
- `lovelycarve-cli context set [--dir <string>] [--node-id <string>]`
  切换当前检材或镜像内目录
  Keywords: change directory, cd, working directory.
- `lovelycarve-cli context use [image]`
  选择当前检材并回到检材根目录。选择 images list 或 raid detect 返回的检材 ID，也接受当前会话的 assembled:// 路径或普通检材路径；后续 files list/read 等命令使用该检材。
  - `lovelycarve-cli context use 7f299489-e542-4c19-8abc-984ebed67f0f`: 在 shell 或 JSONL 会话中切换到已加载检材并回到根目录

## disks

主机物理磁盘

- `lovelycarve-cli disks list`
  列出主机物理磁盘
  Keywords: physical drive, device.

## encrypted-volumes

LUKS、VeraCrypt 与 FileVault 加密容器

- `lovelycarve-cli encrypted-volumes unlock --options <json> [--task-id <string>]`
  解锁 LUKS、VeraCrypt 或 FileVault 容器。把检材中的加密区域注册为会话内只读镜像；返回的 image.path（decrypted://…）可再交给 images open。
  Keywords: luks, veracrypt, truecrypt, filevault, decrypt.

## files

检材内的文件：按路径或节点浏览、读取、解析与导出

- `lovelycarve-cli files ancestors list [--node-id <string>] --path <string>`
  列出节点的祖先目录链
- `lovelycarve-cli files binary get <path> [--node-id <string>]`
  解析可执行文件结构。ELF、Mach-O、PE 与 Unix ar，按魔数自动识别。
  Keywords: pe, elf, mach-o, executable, headers.
- `lovelycarve-cli files children count --parent-ids <string>…`
  统计目录的子节点数
- `lovelycarve-cli files content get <path> [--node-id <string>]`
  查看文件内容属性。MIME、魔数、熵与数据运行段；需要原检材。
  Keywords: mime, magic, entropy, data runs.
- `lovelycarve-cli files csv get <path> [--node-id <string>] [--delimiter <string>]`
  解析 CSV 或 TSV 表格
- `lovelycarve-cli files directories export <path> [--node-id <string>] --destination <string> [--task-id <string>] [--mode <tree|zip>]`
  导出目录（目录树或 ZIP）
- `lovelycarve-cli files docker build [--root-id <string>] --path <string>`
  构建 Docker 容器视图。合成并保存所选容器的叠加文件视图（不分析其他容器）。
- `lovelycarve-cli files docker get [--root-id <string>] --path <string>`
  查看 Docker 容器视图状态
- `lovelycarve-cli files empty-dirs list --parent-ids <string>…`
  判断哪些目录的子树中没有文件
- `lovelycarve-cli files export <path> [--node-id <string>] --destination <string>`
  导出单个文件
  Keywords: export file, copy out, save file, extract.
- `lovelycarve-cli files ext4-journal blocks list [--node-id <string>] [--cursor <string>] [--page-size <number>] --path <string>`
  分页列出 ext4 日志块
- `lovelycarve-cli files ext4-journal get [--node-id <string>] --path <string>`
  读取 ext4 jbd2 日志。从日志中恢复 inode 表、目录与位图块的历史版本以及撤销块列表。
- `lovelycarve-cli files extensions count [--parent-id <string>] --path <string>`
  统计子树中的扩展名
- `lovelycarve-cli files flat list [--parent-id <string>] [--allow-all-files] [--keyword <string>] [--sort <string>] [--sortdesc] [--skip <number>] [--limit <number>] [--filter <json>] [--refresh] --path <string>`
  分页平铺列出子树中的文件
- `lovelycarve-cli files get <path> [--node-id <string>]`
  查看文件节点属性。名称、类型、大小、inode、时间戳、删除状态与链接目标；来自卷索引，离线可用。
  Keywords: stat, metadata, timestamps, show file timestamps.
- `lovelycarve-cli files gguf get <path> [--node-id <string>]`
  解析 GGUF 模型文件结构
- `lovelycarve-cli files hash <path> [--node-id <string>] [--algos <md5|sha1|sha256|sm3>…]`
  计算文件哈希
  Keywords: md5, sha1, sha256, sm3, checksum, digest.
  - `lovelycarve-cli --image D:\evidence\disk.E01 files hash /<卷名称>/Users/alice/report.docx --algos md5 sha256`: 只计算 MD5 与 SHA-256
- `lovelycarve-cli files identify <path> [--node-id <string>]`
  识别文件内容类型。用 Magika 识别内容类型、MIME 与置信度，不依赖扩展名；需要原检材。
  Keywords: file type, magika, detect format.
- `lovelycarve-cli files list [--parent-id <string>] [--cursor <string>] [--page-size <number>] [--query <json>] [--path <string>]`
  分页列出目录的子节点。一页子节点；路径首段可用根节点 ID（如 /part:0）代替卷名。--query 可按扩展名、大小、时间等过滤并排序（JSON）。
  Keywords: ls, dir, directory, folder, list files, browse.
- `lovelycarve-cli files lookup --root-id <string> --path <string>`
  按根目录相对路径查找节点。把插件或报告中的根目录相对路径定位到节点，优先使用实时目录，离线时使用已校验的索引。
- `lovelycarve-cli files media create <path> [--node-id <string>] [--ext <string>] [--task-id <string>]`
  把媒体文件物化为临时文件。供外部播放器按路径读取；临时文件在进程退出或 files media release 后删除。
- `lovelycarve-cli files media release <task-id>`
  释放媒体临时文件
- `lovelycarve-cli files ocr scan <path> [--node-id <string>] [--task-id <string>]`
  识别图片中的文字。需要完整构建（QR/OCR）；轻量构建返回功能未启用。
  Keywords: ocr, text recognition.
- `lovelycarve-cli files qr-codes scan <path> [--node-id <string>] [--expected-count <number>] [--task-id <string>]`
  识别图片中的二维码。需要完整构建（QR/OCR）；轻量构建返回功能未启用。
  Keywords: qrcode, barcode.
- `lovelycarve-cli files read <path> [--length <number>] [--offset <number>] [--node-id <string>] [--text]`
  读取文件内容。把文件字节原样写到标准输出（可重定向到文件）；--offset/--length 读取一段，--text 按 UTF-8 解码。
  Keywords: read file content, cat, hex, dump, bytes.
  - `lovelycarve-cli --image D:\evidence\disk.E01 files read /part:0/Windows/System32/config/SAM > SAM`: 以根列表中的卷 ID 定位并保存原始文件
  - `lovelycarve-cli --image D:\evidence\disk.E01 files read /part:0/notes.txt --length 4096 --text`: 以 UTF-8 文本查看开头 4 KiB
- `lovelycarve-cli files realm get <path> [--node-id <string>]`
  查看 Realm 数据库结构
- `lovelycarve-cli files realm rows list [--node-id <string>] --table-number <number> [--offset <number>] [--limit <number>] --path <string>`
  分页列出 Realm 表记录
- `lovelycarve-cli files search <pattern> [path] [--codepage <string>…] [--exclude-deleted] [--ignore-case] [--limit <number>] [--regex] [--search-id <string>] [--node-id <string>]`
  在文件内容中搜索关键词或正则。复用全局搜索检索当前目录子树（或指定路径），需要打开案件且卷索引已建立；完整命中保存在案件中。
  Keywords: grep, keyword, regex, content search.
  - `lovelycarve-cli --case D:\cases\demo --image D:\evidence\disk.E01 files search password / --ignore-case`: 在全部已索引卷中搜索关键词
- `lovelycarve-cli files sqlite deleted-records list [--node-id <string>] [--parent-id <string>] [--key <json>] --path <string>`
  恢复 SQLite 删除记录。从数据库及其 -wal、-journal 中恢复已删除记录。
  Keywords: deleted rows, freelist, recover sqlite.
- `lovelycarve-cli files sqlite get <path> [--node-id <string>] [--parent-id <string>] [--key <json>]`
  查看 SQLite 数据库结构
  Keywords: database, tables, schema.
- `lovelycarve-cli files sqlite rows list [--node-id <string>] [--parent-id <string>] --table <string> [--offset <number>] [--limit <number>] [--order-by <string>] [--descending] [--key <json>] --path <string>`
  分页列出 SQLite 表记录
- `lovelycarve-cli files stats get <path> [--node-id <string>] --path-hint <string>`
  查看目录统计
- `lovelycarve-cli files streams export <path> [--node-id <string>] --stream-name <string> --destination <string>`
  导出命名数据流
- `lovelycarve-cli files streams get <path> [--node-id <string>] --stream-name <string> [--max-bytes <number>] [--text]`
  读取命名数据流内容
- `lovelycarve-cli files streams list [--node-id <string>] --path <string>`
  列出 NTFS 命名数据流
  Keywords: ads, alternate data stream.
- `lovelycarve-cli files strings list <path> [--encoding <ascii|utf-8|utf-16le|utf-16be>] [--length <number>] [--limit <number>] [--min-length <number>] [--offset <number>] [--node-id <string>]`
  提取文件中的可打印字符串
  Keywords: strings, printable text.
- `lovelycarve-cli files text get <path> [--node-id <string>] [--max-bytes <number>]`
  读取文件文本（自动识别编码）。检测 GBK、Big5、Shift-JIS 等编码并解码为 UTF-8，最多 2 MiB。
  Keywords: gbk, encoding, decode text.
- `lovelycarve-cli files timeline export <path> [--node-id <string>] --destination <string>`
  导出文件系统时间线（mactime 格式）。把整个检材或一个子树导出为 TSK mactime body file，逐行流式写入。
  Keywords: mactime, bodyfile, sleuthkit.
- `lovelycarve-cli files tree list [--depth <number>] [--limit <number>] [--name <string>] [--path <string>] [--node-id <string>]`
  列出目录内容。按路径列出目录，首段可用根节点 ID（如 /part:0/Users）代替卷名；结果 path 保留实际名称，重名时用 node.id。--depth 递归、--name 按名称过滤（忽略大小写包含匹配）；优先读取卷索引，离线检材可浏览已索引目录。
  Keywords: tree, find file by name, recursive.
  - `lovelycarve-cli --image D:\evidence\disk.E01 files tree list --path /`: 列出检材根下的分区与卷
  - `lovelycarve-cli --image D:\evidence\disk.E01 files tree list --path /part:0/Users --depth 8 --name report`: 以根列表中的卷 ID 指定子树并按名称查找
- `lovelycarve-cli files usn-journal records list [--node-id <string>] [--cursor <string>] [--page-size <number>] --path <string>`
  分页列出 NTFS USN 日志记录。读取文件所在 NTFS 卷的 $UsnJrnl：文件的创建、删除与改名历史。
  Keywords: $usnjrnl, change journal.
- `lovelycarve-cli files vim-swap get <path> [--node-id <string>]`
  解析 Vim 交换文件
- `lovelycarve-cli files zip get <path> [--node-id <string>]`
  查看 ZIP 归档结构
- `lovelycarve-cli files zip members get <path> [--node-id <string>] --member-path <string> [--max-bytes <number>] [--text]`
  读取 ZIP 成员内容。解压一个成员并原样写到标准输出（最多 1 MiB）。

## filevault

FileVault 2（CoreStorage）

- `lovelycarve-cli filevault get --options <json> [--task-id <string>]`
  查看 FileVault 逻辑卷与取证观察

## images

检材：打开、查看、导出、哈希与识别

- `lovelycarve-cli images bad-regions list`
  列出补零的坏区域。读取故障审计：被合成为零字节的源区间；有案件时合并已保存的审计。
  Keywords: read errors, bad sectors.
- `lovelycarve-cli images close [image]`
  卸载检材。释放检材及其解密子镜像、挂载与缓存。
- `lovelycarve-cli images digests edit --entries <json>`
  保存文件哈希到案件
- `lovelycarve-cli images digests get --node-ids <string>…`
  按节点读取保存的文件哈希
- `lovelycarve-cli images digests list`
  列出案件中保存的文件哈希
- `lovelycarve-cli images drive-letters list`
  列出 Windows 原盘符映射
- `lovelycarve-cli images export [image] --destination <string> [--task-id <string>] [--read-fault-policy <zeroFillAndReport|strict>] [--mode <source|unlockedPartitions>]`
  导出检材为 RAW。把检材的逻辑字节流写成 RAW；mode unlockedPartitions 时已解锁分区写入明文。
  Keywords: dd, convert, raw image.
- `lovelycarve-cli images file-types edit --entries <json>`
  保存类型识别结果到案件
- `lovelycarve-cli images file-types get --node-ids <string>…`
  按节点读取保存的类型识别结果
- `lovelycarve-cli images file-types list`
  列出案件中保存的类型识别结果
- `lovelycarve-cli images get [image]`
  查看检材信息。格式、大小、分区与文件系统。
  Keywords: partitions, info, list partitions, partition table.
- `lovelycarve-cli images hash [image] [--algorithms <md5|sha1|sha256|sm3>…] [--task-id <string>] [--read-fault-policy <zeroFillAndReport|strict>]`
  计算检材哈希
  Keywords: md5, sha1, sha256, sm3, verify, checksum.
- `lovelycarve-cli images list`
  列出已加载的检材
- `lovelycarve-cli images open <path> [--qcow-password <secret>] [--qcow-key-hex <secret>] [--task-id <string>] [--raw-read-mode <strict|force>]`
  打开检材。打开磁盘镜像、目录、归档、iOS 备份、AD1 或物理盘并设为当前检材；也接受已加载检材的 UUID 和会话内 assembled:// 路径。有案件时登记普通源检材并在后台建立卷索引。
  Keywords: open evidence, open image, load, evidence, disk image, e01, vmdk, raw, dd, add evidence.
  - `lovelycarve-cli images open D:\evidence\disk.E01`: 打开检材并显示其信息
  - `lovelycarve-cli images open D:\evidence\disk.qcow2 --qcow-password`: 在终端中隐藏输入 QCOW 密码
  - `lovelycarve-cli images open D:\evidence\disk.001 --raw-read-mode force`: 按连续分片前缀强行读取不完整 RAW 镜像
- `lovelycarve-cli images os detect`
  识别检材中的操作系统
- `lovelycarve-cli images platform detect`
  识别检材平台与类型
- `lovelycarve-cli images tags edit --upserts <json> --delete-node-ids <string>…`
  增改或删除部分文件标签
- `lovelycarve-cli images tags get --node-ids <string>…`
  按节点读取文件标签
- `lovelycarve-cli images tags list`
  列出案件中的文件标签
- `lovelycarve-cli images tags update --entries <json> [--force]`
  替换检材的全部文件标签

## indexes

卷索引：构建、列出与查询

- `lovelycarve-cli indexes create --volume-key <string> [--task-id <string>]`
  构建卷索引
- `lovelycarve-cli indexes delete --volume-key <string> [--force]`
  删除卷索引
- `lovelycarve-cli indexes files list [--volume-key <string>] --filter <json>`
  按条件查询索引中的文件。用结构化过滤条件查询文件元数据索引；省略 --volume-key 时依次查询全部已索引卷。
  Keywords: find files, query metadata.
- `lovelycarve-cli indexes files lookup --query <json>`
  按元数据地址查找文件节点
- `lovelycarve-cli indexes list`
  列出案件中的卷索引
- `lovelycarve-cli indexes volumes list`
  列出已建索引的卷

## ios-backups

iOS 本地备份

- `lovelycarve-cli ios-backups audit [--task-id <string>]`
  审计 iOS 本地备份。返回上游审计发现：代码、严重程度、证据、来源与说明。
- `lovelycarve-cli ios-backups unlock --password <secret> [--task-id <string>]`
  解锁加密的 iOS 本地备份

## mounts

只读挂载与 SMB 共享（Windows）

- `lovelycarve-cli mounts create <path> [--node-id <string>] --mount-point <string>`
  只读挂载卷或目录。通过 Dokan 只读挂载（仅 Windows）；挂载在进程退出时释放，宜在 session 或 shell 中使用。
- `lovelycarve-cli mounts delete <mount-id> [--force]`
  卸载挂载
- `lovelycarve-cli mounts list`
  列出挂载
- `lovelycarve-cli mounts points create --root <string> --name <string>`
  创建文件夹挂载点
- `lovelycarve-cli mounts points get`
  推荐一个可用挂载点
- `lovelycarve-cli mounts points list`
  列出可用挂载点
- `lovelycarve-cli mounts share <mount-id> [--share-name <string>]`
  以 SMB 共享挂载
- `lovelycarve-cli mounts unshare <mount-id>`
  取消挂载的 SMB 共享

## plugins

插件包、分析器与插件任务

- `lovelycarve-cli plugins create <archive>`
  安装插件包。安装 .lcpkg 插件包；已安装的同 ID 插件被替换，内置插件不能替换。
  - `lovelycarve-cli plugins create ./plugin-0.2.0.lcpkg`: 安装插件包
- `lovelycarve-cli plugins delete <plugin-id> [--force]`
  卸载插件包。卸载用户安装的插件包；内置插件不能卸载。已发布的分析结果保留在案件里。
- `lovelycarve-cli plugins functions list`
  列出插件提供的分析器。每个分析器的作用域（system / evidence / files）、适用平台、选项与画像事实。
- `lovelycarve-cli plugins list`
  列出插件包。已安装与内置的插件包：分析器、记录类型、语言包、库文件状态，以及加载失败的包。
- `lovelycarve-cli plugins post-actions update [--actions-json <string>]`
  设置检材登记后的自动任务
- `lovelycarve-cli plugins restart <plugin-id>`
  重启插件的共享实例
- `lovelycarve-cli plugins systems list`
  列出检材上识别出的操作系统。系统作用域分析的目标：每个系统的 id、产品名与它挂载的卷（fstab、盘符）。首次调用会读取检材识别系统。
- `lovelycarve-cli plugins tasks cancel <task-id>`
  取消插件任务
- `lovelycarve-cli plugins tasks create --function <string> [--node-id <string>] [--file-name <string>] [--system-id <string>] [--options <json>]`
  创建插件任务。系统作用域的分析器给 --system-id，文件作用域的给 --node-id；--options 按分析器声明的选项 id 传 JSON 对象。
  - `lovelycarve-cli plugins tasks create --function org.lovelycarve.linuxinfo/linuxbaseinfo --system-id linux@part:2`: 对识别出的 Linux 系统运行基本信息分析
  - `lovelycarve-cli plugins tasks create --function org.lovelycarve.linuxinfo/linuxbaseinfo --system-id linux@part:2 --options '{"items":["users","logins"]}'`: 只分析用户与登录两个子项
- `lovelycarve-cli plugins tasks delete <task-id> [--force]`
  删除插件任务
- `lovelycarve-cli plugins tasks restart <task-id>`
  重新运行插件任务
- `lovelycarve-cli plugins tasks start <task-id>`
  启动插件任务

## raid

RAID 组装

- `lovelycarve-cli raid assemble --params <json>`
  按指定几何手动组装 RAID
  - `lovelycarve-cli raid assemble --params '{"member_image_ids":["<ID1>","<ID2>",""],"level":5,"layout":2,"chunk_sectors":128,"data_offset_sectors":2048}'`: 三盘 RAID5 左同步、64 KiB 块，第三块盘缺失（空字符串）按校验重建
- `lovelycarve-cli raid detect [--degraded-sets <string>…]`
  自动识别并组装 RAID。解析已加载检材中的 LVM 与 mdadm 元数据（0.90 / 1.x，linear、RAID0/1/4/5/6/10），组装成合成检材；所有完整且未作为 md 成员的逻辑卷均注册为可选择的检材。用 context use <assembledImageId> 选择卷，再用 files list 和 context set --dir 浏览，无需构造内部节点 ID。无元数据的等大成员盘按 RAID0/5 推定。元数据仍登记、但未加载或已陈旧的成员会让阵列停在待确认状态，用 --degraded-sets 指定的阵列才降级组装。重复调用复用已组装的检材。
  Keywords: mdadm, lvm, software raid, raid1, raid6, raid10, degraded, nas.
  - `lovelycarve-cli raid detect`: 识别并组装所有已加载成员盘
  - `lovelycarve-cli raid detect --degraded-sets 880f205730e64da65bfdb06f700131bb`: 缺一块盘时按校验降级组装指定阵列

## searches

全局搜索：任务、命中与导出

- `lovelycarve-cli searches create [--search-id <string>] --options <json>`
  运行全局搜索。在检材全部已索引卷中搜索关键词或正则，完成后返回摘要；命中保存在案件中。需要打开案件。
  Keywords: keyword search, regex, grep, find text.
- `lovelycarve-cli searches delete <search-id> [--force]`
  删除一次搜索及其命中
- `lovelycarve-cli searches export <search-id> --destination <string> [--keyword <string>] [--distinct <string>]`
  导出搜索命中为 CSV
- `lovelycarve-cli searches hits list --search-id <string> [--skip <number>] [--limit <number>] [--keyword <string>] [--distinct <file|sha256>] [--after-seq <number>]`
  分页列出搜索命中
- `lovelycarve-cli searches list [--limit <number>]`
  列出搜索历史

## sessions

命名常驻会话：复用已打开检材、组装卷和当前目录

- `lovelycarve-cli sessions attach <name>`
  进入常驻会话的普通命令终端。连接已运行的会话，直接输入 files list、raid detect 等普通命令；cd/pwd 导航，exit 仅断开终端，保留已打开检材。
- `lovelycarve-cli sessions close <name>`
  关闭常驻会话并释放检材。取消后台工作，关闭案件、插件与检材，然后退出会话进程。
- `lovelycarve-cli sessions get <name>`
  查看常驻会话及当前检材
- `lovelycarve-cli sessions list`
  列出本机当前用户的常驻会话
- `lovelycarve-cli sessions open <name>`
  启动命名常驻会话。在后台保持案件、检材、组装卷和当前目录。后续命令用 --session <名称>，或设置 LOVELYCARVE_SESSION；无需手写 JSONL。用 sessions close 关闭。启动选项 --case/--image 只用于新会话。
  Keywords: 连续取证, persistent session, reuse evidence, attach.
  - `lovelycarve-cli --image D:\evidence\disk.E01 sessions open exam`: 启动后用 --session exam 执行后续普通命令

## shadow-copies

NTFS 卷影副本

- `lovelycarve-cli shadow-copies list`
  列出 NTFS 卷影副本
  Keywords: vss, volume shadow copy.
- `lovelycarve-cli shadow-copies open <partition-id> --index <number> [--task-id <string>]`
  打开卷影副本为只读检材

## system

本程序与主机

- `lovelycarve-cli system get`
  查看版本、构建功能与会话协议
  Keywords: capabilities, version, features.
- `lovelycarve-cli system modules list`
  列出功能模块
- `lovelycarve-cli system paths get <path>`
  查看主机路径信息
- `lovelycarve-cli system paths reveal <path> [--select]`
  在系统文件管理器中显示路径

## tasks

进程内的后台任务

- `lovelycarve-cli tasks cancel <task-id>`
  取消任务。内置任务给它的 id，插件任务给 plugin:<ID>。取消是协作式的：任务在下一个检查点停下。
- `lovelycarve-cli tasks get <task-id>`
  查看任务
- `lovelycarve-cli tasks list`
  列出任务中心的任务。插件任务与内置任务（加载、哈希、导出、解锁、平台识别、快照与索引、全局搜索、时间线、文件恢复、Docker 合并）：运行中的、本次会话结束的，以及搜索历史、时间线状态与恢复扫描各自保留的。插件任务的 id 为 plugin:<ID>；不属于任何检材的任务（打开检材）总会列出。
- `lovelycarve-cli tasks wait`
  等待后台任务与插件任务结束。只表示任务不再活动；结果请读取对应的状态或结果命令。

## timeline

时间线：构建、查询与导出

- `lovelycarve-cli timeline buckets list --query <json> [--limit <number>] [--offset <number>]`
  按时间桶统计事件
- `lovelycarve-cli timeline build`
  构建时间线。重建检材时间线并等待完成；取消时保留已写入的部分。需要打开案件。
- `lovelycarve-cli timeline events export --query <json> --destination <string>`
  导出时间线事件为 CSV
- `lovelycarve-cli timeline events list --query <json> [--skip <number>] [--limit <number>] [--order <asc|desc>]`
  分页列出时间线事件
- `lovelycarve-cli timeline get`
  查看时间线状态
- `lovelycarve-cli timeline range get`
  查看时间线的时间范围
- `lovelycarve-cli timeline types list`
  列出时间线事件类型

## timezones

文件时间显示：自动读取检材时区或保存案件、检材与会话设置

- `lovelycarve-cli timezones get`
  查看有效显示时区、来源及自动识别结果。auto 按命令、会话、检材设置、案件设置、检材配置、UTC 的顺序选择；多个系统的时区冲突时回退 UTC 并报告。不会使用本机时区。
- `lovelycarve-cli timezones set <zone> [--scope <session|case|image>]`
  保存文件时间的显示时区。scope=session 保存到当前会话；case/image 保存到当前案件，在重启后生效。auto 清除该层设置。只改变显示，保留原始时间戳。
  - `lovelycarve-cli --case D:\case timezones set Asia/Shanghai --scope case`: 持久保存案件显示时区
  - `lovelycarve-cli --session nas timezones set evidence`: 会话按检材原始时区显示

## Root commands

- `lovelycarve-cli complete [bash|zsh|fish|powershell]`
  生成并处理 shell 补全。输出补全脚本；按 Tab 时 shell 以 complete -- <单词…> 回调本命令取得候选。
  Keywords: tab completion, bash, zsh, fish, powershell.
  - `lovelycarve-cli complete powershell >> $PROFILE`: 安装 PowerShell 补全（重开终端后生效）
  - `lovelycarve-cli complete bash >> ~/.bashrc`: 安装 bash 补全（重开终端后生效）
  - `lovelycarve-cli complete zsh >> ~/.zshrc`: 安装 zsh 补全（重开终端后生效）
  - `lovelycarve-cli complete fish > ~/.config/fish/completions/lovelycarve-cli.fish`: 安装 fish 补全
- `lovelycarve-cli schema [command…] [--list]`
  查看命令的后端请求结构。输出生成命令发送的后端请求：operationId、IPC 命令名与参数（API 名称、类型、是否必填、说明）。--list 列出全部。
  - `lovelycarve-cli schema images open`: 查看 images open 发送的后端请求
  - `lovelycarve-cli schema --list`: 列出全部生成命令的请求结构
- `lovelycarve-cli session [--jobs <number>]`
  以 JSONL 在一个进程内连续执行命令。从标准输入逐行读取 {"id","command","args"} 请求，每条结果写成一行 JSON；检材与案件在请求之间保持打开。
  Keywords: jsonl, batch, automation, agent, stdin.
  - `lovelycarve-cli --image D:\evidence\disk.E01 session --jobs 4`: 对一个检材连续提交 JSONL 请求，最多 4 条查询并发
  - `lovelycarve-cli --case D:\cases\demo session --events`: 同时把进度事件以 JSONL 写到标准错误
- `lovelycarve-cli shell`
  交互式终端。逐行执行命令，检材与案件在命令之间保持打开；cd、pwd 切换镜像内目录，exit 退出。
  Keywords: repl, interactive.
  - `lovelycarve-cli --case D:\cases\demo shell`: 打开案件后进入交互终端
