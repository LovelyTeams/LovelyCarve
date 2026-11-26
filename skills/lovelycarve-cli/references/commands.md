# lovelycarve-cli command reference

Generated from the CLI command tree by `just update-cli-skill`; do not edit by hand.

Each entry is a command's synopsis and its description. Positional arguments come first, then options; `<…>` is required, `[…]` optional, `a|b` lists the accepted values and `…` marks a repeatable value. A group given without a subcommand runs the subcommand marked as its default. Evidence addresses start at the workspace root: `/` lists the evidence, `/<alias>` an evidence item's partitions and volumes, `/<alias>/<volume>/<path>` a file, and `/<alias>/@<node id>` any node by its ID. Run `lovelycarve-cli <command> --help` for option descriptions and defaults, and `lovelycarve-cli schema <command>` for the JSON description.

## Options on every command

- `-C|--case <目录>`
  案件目录；默认取 LOVELYCARVE_CASE，或当前目录及其上级中的案件
- `--image <检材路径>…`
  先打开这份检材（可重复）；别名取文件名
- `--tz <时区>`
  文件时间的显示时区：auto、evidence、case、UTC、IANA 名称或 ±HH:MM
- `--json[=<字段>]`
  输出 JSON（文件内容为十六进制）；--json=字段1,字段2 只保留列表条目的这些字段
- `--jq <表达式>`
  用 jq 表达式筛选或重组 JSON 结果（自动启用 --json）
- `-q|--quiet`
  不输出进度等非必要信息
- `--events`
  生命周期与进度事件以 JSONL 写到标准错误
- `--dry-run`
  只显示解析后的请求，不执行
- `-y|--yes`
  确认删除或丢弃数据的操作
- `-h|--help`
  显示帮助
- `-V|--version`
  显示版本

## Contents

- [案件与检材](#案件与检材)
- [浏览与读取](#浏览与读取)
- [查找与解析](#查找与解析)
- [分析](#分析)
- [任务与主机](#任务与主机)
- [程序](#程序)

## 案件与检材

- `case status`
  当前案件：后台进程、检材、卷索引与运行中的任务（默认）
- `case new <目录> [--name <name>] [--number <number>] [--description <description>] [--examiner <examiner>] [--open]`
  创建案件；--open 同时启动它的后台进程
- `case open [目录]`
  启动案件后台进程：之后指向该案件的命令复用已打开的检材、解锁与组装结果
- `case close [目录]`
  停止案件后台进程：取消任务、释放检材后退出
- `case ls`
  列出本机登记的案件
- `case lock`
  锁定当前案件：拒绝添加或移除检材、构建或删除卷索引等修改
- `case unlock`
  解除案件锁定
- `case link [路径] [--offset <字节>] [--length <字节>]`
  当前案件或其中文件的 lovelycarve:// 链接：桌面端打开后定位并高亮
- `case rm <目录>`
  删除案件：移除案件目录中由 LovelyCarve 创建的台账、检材数据与暂存区
  (asks first; `-y` in scripts)
- `evidence ls`
  列出检材与别名（默认）
- `evidence add <检材路径>… [--as <别名>] [--raw-read-mode <strict|force>] [--qcow-password[=<@文件>]] [--qcow-key[=<@文件>]] [--wait]`
  打开检材；有案件时登记进案件并在后台建立卷索引
- `evidence show [别名]`
  检材的格式、大小、分区与采集元数据
- `evidence close <别名>`
  在当前进程中关闭检材（不改动案件）；关闭源检材也关闭由它派生的检材
- `evidence rm <别名>`
  从案件中移除检材
  (asks first; `-y` in scripts)
- `evidence hash [别名] [--algo <md5|sha1|sha256|sm3>…] [--strict]`
  计算整份检材的哈希
  (exit 2 when set: readFaultsDetected, syntheticBytes)
- `evidence export <检材> -o|--output <文件> [--unlocked] [--strict]`
  导出整份检材为 RAW；加 --unlocked 时已解锁的分区写成明文
  (exit 2 when set: readFaultsDetected, syntheticBytes)
- `evidence bad-regions [别名]`
  读取时以零填充的坏区域
- `volumes [别名]`
  检材中的分区与卷：文件系统、加密与索引状态
- `encryption <路径> [--plist <文件>]`
  加密卷的类型、状态与元数据
- `unlock <路径> [--scheme <bitlocker|luks|veracrypt|filevault|ios>] [--password[=<@文件>]] [--recovery-password[=<@文件>]] [--startup-key <文件>] [--fvek[=<@文件>]] [--tweak[=<@文件>]] [--vmk[=<@文件>]] [--vmk-from <内存镜像>] [--master-key[=<@文件>]] [--pim <pim>] [--header <auto|normal|hidden|backup|system>] [--prf <sha512|sha256|blake2s|whirlpool|streebog|ripemd160|argon2id>] [--keyfile <文件>…] [--logical-volume <序号>] [--plist <文件>] [--offset <offset>] [--length <length>] [--new-image] [--as <别名>]`
  解锁 BitLocker、LUKS、VeraCrypt、FileVault 或加密的 iOS 备份
  (needs a case backend, shell or batch)
- `raid detect [--member <别名>…] [--degraded <uuid>…]`
  识别并组装已打开检材中的 mdadm、LVM 与固件 RAID；无元数据的等大成员盘按 RAID0/5 推定
- `raid assemble --members <别名>… --level <level> [--layout <layout>] [--chunk-sectors <chunk-sectors>] [--data-offset-sectors <data-offset-sectors>] [--missing <列>…] [--as <别名>]`
  按给定几何手动组装 RAID
  (needs a case backend, shell or batch)
- `vss ls <路径>`
  列出检材中 NTFS 卷的卷影副本
- `vss open <分区> <序号> [--as <别名>]`
  把一个卷影副本打开为只读检材
  (needs a case backend, shell or batch)

## 浏览与读取

- `ls [路径] [-l|--long] [--limit <limit>] [--cursor <游标>] [--all] [--sort <name|size|modified|created|accessed|ext|kind|inode>] [--desc] [--deleted] [--no-deleted] [--ext <扩展名>…]`
  列出目录
- `tree <路径> [--depth <depth>] [--name <文本>] [--limit <limit>]`
  递归列出目录树
- `stat <路径> [--content]`
  文件或目录的属性
- `cat <路径> [--offset <offset>] [--length <length>] [--member <成员路径>] [--stream <流名>]`
  输出文件内容（原始字节）
  (raw bytes on stdout)
- `text <路径> [--encoding <encoding>] [--offset <offset>] [--max-bytes <max-bytes>]`
  读取文本：识别编码并转为 UTF-8，分段读取
- `strings <路径> [--encoding <ascii|utf-8|utf-16le|utf-16be>] [--min-length <min-length>] [--offset <offset>] [--length <length>] [--limit <limit>]`
  提取可打印字符串
- `hash <路径>… [--algo <md5|sha1|sha256|sm3>…]`
  计算文件哈希
  (exit 2 when set: items.readFaultsDetected, items.syntheticBytes, items.truncated)
- `identify <路径>`
  识别文件内容类型（不看扩展名）
- `streams <路径>`
  列出命名数据流、资源分叉与扩展属性
- `export <路径>… -o|--output <输出路径> [--zip] [--stream <流名>]`
  导出文件或目录到本机
  (exit 2 when set: items.readFaultsDetected, items.syntheticBytes, items.truncated, items.cancelled)
- `bodyfile <路径> -o|--output <文件>`
  导出 mactime body 文件（文件系统时间线）

## 查找与解析

- `find <通配符> [--under <路径>] [--type <file|dir|symlink>] [--deleted] [--no-deleted] [--min-size <大小>] [--max-size <大小>] [--limit <limit>] [--offset <offset>]`
  按文件名查找（卷索引）
  (exit 2 when set: unindexed)
- `grep <模式> [路径] [-i|--ignore-case] [-E|--regex] [--encoding <编码>…] [--no-deleted] [--limit <limit>]`
  在文件内容中搜索关键词或正则
  (exit 2 when set: summary.readErrors, summary.truncated)
- `journal <路径> [--limit <limit>] [--cursor <游标>]`
  文件系统日志：NTFS USN 与 ext4 jbd2
- `inspect <路径> [--as <auto|sqlite|realm|zip|csv|binary|gguf|vim-swap|kdbx>] [--delimiter <字符>] [--passphrase[=<@文件>]] [--raw-key[=<@文件>]] [--cipher-compat <版本>] [--cipher-page-size <字节>] [--kdf-iter <次数>] [--password[=<@文件>]] [--keyfile <文件>] [--transformed-key[=<@文件>]]`
  解析文件结构：SQLite、Realm、ZIP、CSV、可执行文件、GGUF、Vim 交换文件、KeePass 数据库
- `sqlite tables <路径> [--passphrase[=<@文件>]] [--raw-key[=<@文件>]] [--cipher-compat <版本>] [--cipher-page-size <字节>] [--kdf-iter <次数>]`
  表、视图与索引的结构及行数
- `sqlite rows <路径> <表> [--offset <offset>] [--limit <limit>] [--order-by <列>] [--desc] [--passphrase[=<@文件>]] [--raw-key[=<@文件>]] [--cipher-compat <版本>] [--cipher-page-size <字节>] [--kdf-iter <次数>]`
  分页读取一张表的记录
- `sqlite query <路径> <sql> [--param <值>…] [--offset <offset>] [--limit <limit>] [--passphrase[=<@文件>]] [--raw-key[=<@文件>]] [--cipher-compat <版本>] [--cipher-page-size <字节>] [--kdf-iter <次数>]`
  执行一条只读 SELECT（或 WITH … SELECT）；自动读取同目录的 WAL
  (exit 2 when set: shortenedCells)
- `sqlite deleted <路径> [--passphrase[=<@文件>]] [--raw-key[=<@文件>]] [--cipher-compat <版本>] [--cipher-page-size <字节>] [--kdf-iter <次数>]`
  从空闲页、日志与 WAL 中恢复删除的记录
- `ocr <路径>`
  识别图片中的文字
- `qr <路径> [--expect <expect>]`
  识别图片中的二维码

## 分析

- `search ls [--evidence <别名>] [--limit <limit>]`
  列出案件中保存的搜索（默认）
- `search hits <搜索ID> [--evidence <别名>] [--offset <offset>] [--limit <limit>] [--keyword <文本>] [--distinct <file|sha256>]`
  分页读取一次搜索的命中
- `search export <搜索ID> -o|--output <文件> [--evidence <别名>] [--keyword <文本>] [--distinct <file|sha256>]`
  把命中导出为 CSV
- `search rm <搜索ID> [--evidence <别名>]`
  删除一次搜索及其命中
  (asks first; `-y` in scripts)
- `timeline status [--evidence <别名>]`
  构建状态、时间范围与事件类型（默认）
- `timeline build [--evidence <别名>]`
  构建或重建时间线
- `timeline ls [--evidence <别名>] [--from <时间>] [--to <时间>] [--app <应用>…] [--type <编号>…] [--text <文本>] [--deleted] [--offset <offset>] [--limit <limit>] [--desc]`
  分页列出事件
- `timeline stats [--evidence <别名>] [--from <时间>] [--to <时间>] [--app <应用>…] [--type <编号>…] [--text <文本>] [--deleted] [--by <year|month|day|hour|minute|second>] [--utc-offset <utc-offset>] [--limit <limit>]`
  按时间桶统计事件数
- `timeline export [--evidence <别名>] [--from <时间>] [--to <时间>] [--app <应用>…] [--type <编号>…] [--text <文本>] [--deleted] -o|--output <文件>`
  把事件导出为 CSV
- `carve ls [--evidence <别名>]`
  列出检材的恢复扫描（默认）
- `carve formats`
  可雕刻的文件格式
- `carve start [别名] [--scope <whole|unallocated|partition|range>] [--partition <序号>] [--start-sector <扇区>] [--sectors <扇区数>] [--types <格式>…] [--fs <fat|ntfs|exfat>…] [--existing] [--no-signatures] [--max-file-size <大小>] [--max-results <max-results>]`
  开始扫描：文件系统记录恢复与（或）签名雕刻，在后台运行
  (needs a case backend, shell or batch)
- `carve status <扫描ID>`
  扫描的阶段、进度与摘要
- `carve results <扫描ID> [--fs] [--format <格式>] [--category <类别>] [--min-confidence <min-confidence>] [--cursor <游标>] [--limit <limit>]`
  扫描结果：雕刻出的文件，或 --fs 文件系统恢复出的文件
- `carve cat <扫描ID> <结果ID> [--max-bytes <max-bytes>]`
  输出一个雕刻结果的内容（原始字节）
  (raw bytes on stdout)
- `carve pause <扫描ID>`
  暂停扫描
- `carve resume <扫描ID>`
  继续暂停的扫描
- `carve export <扫描ID> -o|--output <目录> [--artifacts <id>…] [--no-artifacts] [--files <编号>…]`
  把恢复出的文件导出到新目录
  (exit 2 when set: failed, syntheticArtifacts, reportError, files.failed, files.readFaults.readFaultsDetected)
- `carve rm <扫描ID> [--evidence <别名>]`
  删除已结束的扫描及其结果
  (asks first; `-y` in scripts)
- `plugin ls`
  列出插件包（默认）
- `plugin inspect <插件包>`
  检视插件包（.lcpkg）：标识、版本、权限、摘要与已安装版本的关系，不安装
- `plugin install <插件包>`
  安装插件包（.lcpkg）：检视后安装同一份字节，替换已安装的同 ID 版本
  (asks first; `-y` in scripts)
- `plugin rm <插件ID>`
  卸载插件包
  (asks first; `-y` in scripts)
- `plugin readme <插件ID>`
  插件包的说明（README.md）
- `plugin market sources`
  列出索引源（默认）
- `plugin market add <索引源>`
  添加索引源：https:// 地址，或本地市场目录、index.json 的绝对路径
- `plugin market rm <索引源>`
  移除索引源
- `plugin market ls [--source <索引源>] [--refresh]`
  列出一个索引源的插件及其安装状态
- `plugin market install <插件ID> [--version <版本>] [--source <索引源>]`
  从索引源下载插件包，按索引校验大小与 SHA-256，检视后安装
  (asks first; `-y` in scripts)
- `plugin market index <目录> --title <标题> [--description <说明>]`
  为目录中的 .lcpkg 生成 index.json
- `analyze ls [--evidence <别名>]`
  列出分析器，以及检材上识别出的系统
- `analyze run <分析器> [--evidence <别名>] [--system <系统ID>] [--file <路径>] [--option <键=值>…] [--options-json <json>]`
  创建插件任务：系统级分析器给 --system，文件级分析器给 --file
- `results ls`
  分析结果树：检材 → 分类 → 分析器运行 → 记录类型（默认）
- `results rows <运行ID> <记录类型> [--evidence <别名>] [--keyword <文本>] [--sort <列>] [--desc] [--offset <offset>] [--limit <limit>] [--legacy] [--bcp-app <编码>…] [--bcp-account <账号>]`
  分页读取一次运行中某种记录
- `results show <记录ID> [--evidence <别名>]`
  一条记录的详情
- `results run <运行ID> [--evidence <别名>]`
  一次运行的审计：输入、选项、画像事实、告警与补零的源字节
- `results import [别名] [--replace]`
  把 BCP 数据包（作为检材打开的 ZIP 或目录）发布为分析结果

## 任务与主机

- `tasks ls [--evidence <别名>]`
  列出任务（默认）：插件任务与本进程的内置任务
- `tasks show <任务ID>`
  查看一个任务
- `tasks wait`
  等待后台任务与插件任务结束；有任务失败时以 tasksFailed 失败
- `tasks cancel <任务ID>`
  取消运行中的任务
- `tasks restart <任务ID>`
  从头重新运行插件任务
- `tasks rm <任务ID>`
  删除插件任务及其发布的结果
  (asks first; `-y` in scripts)
- `index ls`
  列出案件中的卷索引（默认）
- `index build <卷>`
  构建卷索引（后台运行；用 tasks wait 等待）
- `index rm <卷>`
  删除卷索引
  (asks first; `-y` in scripts)
- `mount [路径] [挂载点] [--share <共享名>]`
  只读挂载（Windows）；不带参数时列出挂载
  (needs a case backend, shell or batch)
- `umount <挂载ID>`
  卸载挂载
  (asks first; `-y` in scripts)
- `tz [路径]`
  文件时间的显示时区
- `tz set <时区> [检材]`
  保存显示时区：给检材路径时只对该检材，否则对整个案件；auto 清除设置
- `system`
  版本、构建功能与组件
- `disks`
  本机物理磁盘

## 程序

- `shell`
  交互式 shell：在一个进程中连续执行命令
- `batch [-j|--jobs <jobs>]`
  从标准输入逐行执行 JSONL 请求
- `rpc [命令] [--body <json>] [--list]`
  按名称调用任意后端命令（与桌面端 IPC 相同的参数）
- `schema [命令…]`
  命令的参数与输出结构（JSON）
- `completion <bash|zsh|fish|powershell>`
  生成 shell 补全脚本
