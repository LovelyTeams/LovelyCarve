# LovelyCarve 插件协议（LPP）v2

LovelyCarve 插件是原生动态库（`.dll` / `.so` / `.dylib`），连同清单、记录结构与语言包组成一个**插件包**。插件可以分析一个识别出的操作系统、整个检材或用户选中的文件，也可以解析文件系统。插件通过只读视图与文件句柄读取检材，拿不到检材路径；分析结果是带类型的**记录**，按插件声明的记录结构存进检材的 `records.db`。

每个插件实例运行在独立的 `lc-plugin-host` 进程里，LovelyCarve 通过它的标准输入输出交换 LPP 帧。只支持 LPP v2：LPP v1 的插件包会被拒绝并提示重新构建；旧案件里 v1 插件写下的结果只读展示（见[旧版本数据](#旧版本数据)）。

## 插件包

```text
org.example.demo/
├── plugin.toml        # 清单：身份、库、内存上限、权限、分析器与选项
├── demo.dll           # 库（Linux 为 libdemo.so，macOS 为 libdemo.dylib）
├── schema.json        # 构建时从库导出的分析器与记录结构
└── locales/
    ├── en.toml        # 英文标签
    └── zh-CN.toml     # 可选：清单无字段承载的中文名（如分类名）
```

### 发现

LovelyCarve 启动时扫描以下根目录，每个根目录本身可以是插件包，也可以包含若干插件包目录：

1. 用户数据目录的 `plugins`（安装位置，排在最前）；
2. 程序目录旁的 `plugins`（安装包附带的内置插件）；
3. 环境变量 `LOVELYCARVE_PLUGIN_DIR` 指定的目录；
4. 开发构建另外扫描当前目录的 `plugins` 与仓库的 `plugins/`。

同一插件 ID 以先扫描到的为准。清单、`schema.json` 或语言包无效的插件包不会加载，原因列在「设置 → 插件」；LPP v1 清单得到明确的“请用 SDK 0.2 重新构建”提示。

发现即信任：没有审批或停用开关。插件目录的 SHA-256 指纹只用于展示，首次列出插件时计算（上限 10,000 个文件、512 MiB），不参与执行判断。已加载的库不会卸载，替换库文件需要重启应用或重新安装插件包。

### 安装与卸载

`.lcpkg` 是插件包目录的 ZIP 压缩包：`plugin.toml` 位于压缩包根部，或位于唯一的顶层目录中。安装时：

- 压缩包最多 4096 个条目、单个条目 512 MiB、解压后总计 2 GiB；条目路径不能离开包目录，不能是链接；
- 先解压到安装根目录下的临时目录，校验清单、`schema.json`、语言包与当前平台的库文件，全部通过后再以 `<安装根目录>/<插件 ID>` 原子替换；
- 同 ID 的已安装版本被替换，替换前停止它的共享实例；内置插件不能被替换或卸载；
- 卸载删除插件包目录；已发布的分析结果保留在案件里，结果树与列名继续使用存下的标签。

### 日志

插件进程的 stderr 记入 LovelyCarve 日志（target `plugin`），同时写入程序目录下的 `logs/plugins/<插件 ID>.log`（8 MiB 轮转、保留一代）。SDK 的日志调用带 `ERROR` / `WARN` / `INFO` / `DEBUG` 前缀，加载器自身的诊断记为 `HOST`。

## plugin.toml

```toml
id = "org.example.demo"
name = "示例插件"
version = "1.2.0"
protocol = 2
library = "demo"          # demo.dll / libdemo.so / libdemo.dylib
memory = "1GiB"           # 插件进程内存上限，缺省 2GiB
permissions = []          # file_materialize、network

[[analyzer]]
id = "base"
name = "基本信息"
scope = "system"          # system、evidence 或 files
platforms = ["linux"]     # 仅 system：适用的系统族，缺省为全部
trigger = "manual"        # manual（缺省）或 auto
category = "baseinfo"     # 结果树分类，缺省 analysis
provides = ["timezone"]   # 成功运行后发布的画像事实
requires = []             # 自动运行等待的画像事实
inputs = ["etc/passwd", "home/*/.bash_history"]

[[analyzer.option]]
id = "items"
type = "multi"            # multi、choice、flag 或 text
label = "子项"
choices = [{ id = "users", label = "用户" }, { id = "groups", label = "用户组" }]
default = []              # multi 为空 = 全部

[[analyzer]]
id = "mail"
name = "邮件文件"
scope = "files"
extensions = ["eml", "msg"]
mime_types = ["message/rfc822"]

[[filesystem]]            # 文件系统解析（冻结的 fs.* 方法）
id = "vendorfs"
priority = 100
```

| 字段 | 说明 |
| --- | --- |
| `id` | 1–128 个 ASCII 字母、数字、`.`、`-`、`_` |
| `protocol` | 必须为 `2` |
| `library` | 包目录内的相对路径；没有库扩展名时按平台补前后缀 |
| `memory` | `64MiB`–`64GiB`，缺省 `2GiB`，Windows 以 Job Object 强制 |
| `permissions` | `file_materialize`：允许 `file.materialize`；`network`：向加载器声明联网意图（`LOVELYCARVE_NETWORK_PERMISSION`），不是操作系统防火墙 |
| `analyzer.scope` | `system`：一个识别出的系统；`evidence`：检材的全部卷；`files`：用户选中的文件 |
| `analyzer.platforms` | 仅 system 作用域，取值 `linux`、`windows`、`macos`、`esxi`、`mikrotik`、`openwrt`、`dingtalk` |
| `analyzer.trigger` | `auto` 的分析器在检材上线或所需事实发布时自动建任务；files 作用域只能手动 |
| `analyzer.inputs` | system / evidence 作用域的输入路径模式（最多 64 个），宿主在运行前解析成输入文件；evidence 作用域的模式在每个卷下匹配 |
| `analyzer.extensions` / `mime_types` | 仅 files 作用域：接受的扩展名（小写、无点）与 MIME |
| `analyzer.internal` | 默认任务列表中隐藏 |
| `analyzer.single_instance` | 缺省 `true`：同一作业最多一个未结束的任务 |
| `analyzer.option` | 运行前可选的选项；`multi` / `choice` 必须有 `choices`，`default` 类型须与 `type` 一致 |

清单中的名称与标签是中文默认值；其他语言放在 `locales/` 中。

## 记录结构

分析结果由带类型的记录组成。每种记录类型有一份结构（`RecordSchema`）：

```json
{
  "type": "linux.login",
  "label": "登录记录",
  "title": "{user} @ {terminal}",
  "fields": [
    { "id": "user", "label": "用户", "kind": "text", "key": true },
    { "id": "terminal", "label": "终端", "kind": "text" },
    { "id": "time", "label": "登录时间", "kind": "timestamp", "timeline": "login" },
    { "id": "raw", "label": "原始记录", "kind": "json", "hidden": true },
    { "id": "result", "label": "结果", "kind": "enum", "choices": [{ "id": "ok", "label": "成功" }] }
  ]
}
```

- `type` 是带命名空间的小写标识（`linux.login`）；一个插件最多 256 种记录类型，每种最多 128 个字段。
- 字段类型（`kind`）决定存储、显示、排序与检索：`text`、`int`、`float`、`bool`、`timestamp`、`size`（字节数）、`duration`（秒）、`path`、`hash`、`ip`、`enum`、`list`（文本列表）、`json`。文本类值最长 256 KiB，`list` 最多 4096 项，`json` 最深 8 层、4096 个条目。
- `key` 字段组成记录身份与缺省标题；`title` 是基于字段 id 的标题模板；`hidden` 字段只在详情中显示；`category` 覆盖结果项的分类。
- `timestamp` 值为 RFC 3339。带时区偏移的时间可设 `timeline` 角色（`login`、`modified` 等），并投影到案件时间线；没有偏移的本地时间（时区未知）照常存储、显示与排序，但不进入全局时间线，避免被当作 UTC。

宿主为每种记录类型建一张类型化的表（时间戳另有排序列），按字段类型排序与检索；结构随结果一起保存，插件卸载后结果仍可读。

### schema.json 与结构哈希

库在 `plugin.hello` 中报告自己实现的分析器、文件系统和记录结构，以及结构哈希 `schema_hash`（`fnv1a64:` 前缀）。插件构建（`just build-plugins`）运行 `lc-plugin-host --describe --library <库>`，把这份信息写成包内的 `schema.json`：界面在首次运行前就能显示记录类型与列，每次握手也用它核对库——库的分析器与清单不符、结构与 `schema.json` 不一致或哈希不匹配时，启动失败并提示重新构建插件包。

## 语言包

`locales/<语言>.toml` 是键值表，键使用稳定 id：

| 键 | 内容 |
| --- | --- |
| `plugin.name` | 插件名 |
| `analyzer.<分析器>` | 分析器名 |
| `category.<分类>` | 分类名（清单没有分类名字段，中文名也放在 `zh-CN.toml`） |
| `option.<分析器>.<选项>`、`option.<分析器>.<选项>.<选项值>` | 选项及其取值 |
| `record.<记录类型>`、`field.<记录类型>.<字段>` | 记录类型与列 |
| `choice.<记录类型>.<字段>.<取值>` | 枚举字段的取值 |

文件名形如 `en.toml`、`zh-CN.toml`；单个键最长 256 字节、文本最长 4096 字节。界面按当前语言取包内译文，缺失时显示插件声明的中文。常见分类（`baseinfo`、`system`、`browserinfo` 等）由 LovelyCarve 命名。

## C ABI v1

库导出 `lovelycarve_plugin_v1`。宿主传入自己的函数表（`call`、`free_buffer`、`log` 与不透明上下文），得到插件函数表（`describe`、`call`、`cancel`、`free_buffer`、`destroy` 与不透明实例）。每张表以 ABI 版本和结构体大小开头，版本不支持时握手失败。`plugins/sdk/include/lovelycarve_plugin.h` 为 C / C++ 声明布局，`plugins/sdk/src/abi.rs` 是参考定义。

- 插件调用 `call(instance, call_id, method, params, out)` 在调用线程上执行一个宿主 → 插件方法，`out` 填 MessagePack 结果（`LC_STATUS_OK`）或 `{code, message}` 错误（`LC_STATUS_ERROR`）；`call_id` 供 `cancel` 使用。
- 宿主调用 `call(context, method, params, out)` 以同样方式服务一个插件 → 宿主方法。
- 回复缓冲由被调方填充、调用方通过被调方的 `free_buffer` 释放，两侧可用不同分配器。
- 所有函数都可能被多线程并发调用；调用可重入：插件调用中可以发起宿主调用。
- 重启后入口可能再次被调用以创建新实例，旧实例可能仍在收尾；实例不得共享进程级全局状态。
- panic 等内部故障不得越过边界；Rust SDK 把它们报告为 `PLUGIN_PANIC`。

C / C++ 插件按本文手写载荷：载荷是以字段名为键的 MessagePack map，未知字段被忽略，可选字段可以省略。

## 传输与握手

LovelyCarve 以 `lc-plugin-host --library <库>` 启动加载器（在程序目录查找；开发构建另查 `plugins/target/{debug,release}`；`LOVELYCARVE_PLUGIN_HOST` 可覆盖位置）。加载器把宿主请求转成工作线程上的插件调用，把插件的宿主调用转成发回 LovelyCarve 的请求：

- stdin 是协议输入、stdout 是协议输出、stderr 是日志；
- 每帧是 4 字节大端长度加 MessagePack，帧版本字段为 `2`，单帧上限 8 MiB；
- 宿主请求 ID 为偶数，加载器请求 ID 为奇数；二进制数据使用 MessagePack bin，不用 Base64；
- 读写循环在等待回复期间保持运行，插件处理宿主请求时可以继续调用宿主；
- 宿主最多 256 个待回复的出站请求，同时处理最多 16 个插件 → 宿主请求。版本、ID 奇偶、重复 ID、超大帧或意外消息会关闭连接并让待回复调用失败。超过帧上限的回复返回 `RESOURCE_LIMIT`。

加载器的第一条消息是奇数 ID 的 `plugin.hello`（库的 `describe` 输出：协议版本、插件 ID、SDK 版本、分析器、文件系统、记录结构与结构哈希）。宿主核对清单与 `schema.json` 后回复 `{protocol: 2, host, version}`；协议不符、ID 不符、分析器或文件系统缺失、结构不一致都会结束启动。握手必须在 5 秒内完成。

## 分析运行

每个任务启动一个专属插件实例（独立的 `lc-plugin-host` 进程，带清单的内存上限）；同一插件的多个任务并行运行，停止一个任务只结束它自己的进程。宿主先准备运行，再发送 `run.start`：

1. **目标与视图**：system 作用域打开所选系统的系统视图，evidence 作用域打开检材视图，files 作用域没有视图，输入就是选中的文件。
2. **输入**：system / evidence 作用域按清单 `inputs` 在视图中查找文件（evidence 作用域的模式加上 `*/` 前缀，在每个卷下匹配），按路径排序，最多 100,000 个；打不开的输入记为告警。
3. **选项与画像事实**：用户选项按清单校验，缺省值补齐；画像事实取整个检材与该目标已发布的事实。
4. **暂存运行**：在 `records.db` 中写入一条暂存运行，记录插件版本、目标、选项、事实、输入清单与结构哈希。

```text
run.start  → { run_id, analyzer, target, inputs: [FileInfo], options, profile, locale }
           ← { summary? }
target     = { kind: "files" }
           | { kind: "system",   view: ViewInfo }
           | { kind: "evidence", view: ViewInfo }
ViewInfo   = { view_id, label, system?: { id, family, product, version?, root }, mounts: [{ path, root, source }] }
FileInfo   = { file_id, name, size, mime?, view_path?, root?, root_path?, node_id?, deleted, timestamps }
```

`locale` 是界面语言，供插件自己生成的文字使用。单次运行最长 24 小时；进度上报让调度器的看门狗知道运行仍在进行。

运行期间插件可以调用：

| 方法 | 载荷 | 说明 |
| --- | --- | --- |
| `run.emit` | `{ run_id, records: [{ type, values, title?, severity?, sources? }] }` → `{ accepted }` | 一批最多 4096 条记录；`values` 按结构字段顺序；宿主写入后才回复，批次即背压 |
| `run.warn` | `{ run_id, code, message, file_id? }` | 运行无法完成的部分，随结果保存，不使运行失败；每次运行最多保存 10,000 条 |
| `run.progress` | `{ run_id, current, total?, message? }` | 任务进度 |
| `file.read` / `file.readv` | `{ file_id, offset, length }` / `{ file_id, ranges }` | 单次读取最多 4 MiB；`readv` 最多 64 段、合计 8 MiB 减 4 KiB；读到文件末尾截断 |
| `file.materialize` / `file.release` | `{ file_id }` / `{ lease_id }` | 需要 `file_materialize` 权限：在宿主临时目录生成副本；单个文件 4 GiB，每次运行最多 16 个租约、合计 8 GiB；运行结束时全部清理 |
| `view.stat` / `view.open` | `{ view_id, path }` | 节点信息，或打开文件得到 `FileInfo`；不存在时返回 nil |
| `view.list` | `{ view_id, path, skip, limit }` → `{ nodes, total }` | 目录分页，每页最多 512 个节点 |
| `view.find` | `{ view_id, patterns, cursor? }` → `{ paths, visited, cursor? }` | 1–64 个 glob 模式（`**` 跨目录，`*` / `?` 限于一级名称，ASCII 大小写不敏感）；游标一次性；每次最多遍历 64 个目录、返回 1024 条路径，总计最多访问 400 万个节点、深度 128；每次运行最多 8 个未完成的查找 |

记录的 `sources` 是来源：`{ root, path, node_id?, offset?, length?, note? }`，`root` 是卷键、`path` 是卷内路径，`note` 说明记录在来源中的位置（如“第 12 行”）。每条记录最多 16 个来源。从视图或输入打开的文件，SDK 自动填写卷键、卷内路径与节点 ID；界面据此定位源文件。

插件读取检材遇到坏扇区时，宿主按检材的容错规则以零填充并记录坏区，这些位置随运行保存（运行审计中的“读不出的源字节”）；完全读不出的位置返回错误。

### 视图

视图是一棵只读路径树，路径为 POSIX 风格、相对视图根：

- **系统视图**：所选系统的系统卷在根部，它挂载的卷出现在挂载点——Linux 按 `/etc/fstab` 匹配 `UUID=` / `LABEL=`（超级块）、`PARTUUID=`（GPT 分区 GUID）、`/dev/mapper/<vg>-<lv>` 与 `/dev/<vg>/<lv>`（LVM 名称），`subvol=` 挂载 Btrfs 子卷；`/dev/sda2` 这类依赖启动顺序的设备路径不映射。Windows 按 `MountedDevices` 的盘符挂载，系统卷同时在自己的盘符下可达。
- **检材视图**：检材的每个卷是一个以卷键命名的顶层目录（`part:2/etc/passwd`）；LVM 容器本身不列出，它的逻辑卷各是一个卷。文件夹、压缩包、AD1 等逻辑检材的根卷 `root` 直接是其内容。

名称先精确匹配，再按 ASCII 大小写不敏感匹配；同名时现存条目优先于已删除条目。`stat` / `open` 跟随符号链接（最多 16 跳），绝对链接目标按视图根解析，因此 `/etc/os-release → ../usr/lib/os-release` 或指向另一挂载卷的链接与运行中的系统一致；`list` 不跟随链接。`..` 只在链接目标中生效，不能离开视图。

### 系统识别

识别对检材的每个卷运行操作系统识别；Btrfs 卷与逻辑检材的根卷还会向下探查一级目录（Btrfs 子卷 `@`、文件夹检材中的 `rootfs/`）。每个系统得到一个检材内稳定的 ID（`linux@part:2`、`linux@root/rootfs`）。识别结果按检材缓存，卸载检材时清除。

## 结果发布

运行成功后，宿主在一个事务中把暂存运行发布为该**作业**的当前结果。作业是（分析器、检材、目标），目标是系统 ID、文件节点或整个检材：

- 同一作业的上一次已发布运行变为“已替换”：保留审计行，删除记录及其时间线投影；
- 带时区偏移、声明了时间线角色的时间戳投影到案件时间线；
- 写入运行摘要、记录数、告警数与读不出的源字节；
- 失败、取消或超时时丢弃暂存运行；暂存运行带应用实例的所有者标记，异常退出遗留的暂存运行在该检材下一次开始运行时清理。

删除任务会删除它发布的运行及其时间线投影。结果树按检材 → 分类 → 分析器运行 → 记录类型组织，分页查询固定在结果树节点给出的运行上，翻页或导出期间不会混入新发布的结果。

### 画像事实与自动任务

画像事实（`timezone`、`hostname`、`users` 等）是记录类型 `profile.fact` 的记录，按目标保存；Rust SDK 的 `run.provide(key, value)` 写入它们。分析器在清单 `provides` 中声明会发布的事实，在 `requires` 中声明自动运行前需要的事实：

- 检材上线时，为每个自动触发、无需事实的分析器建任务：evidence 作用域一个，system 作用域在每个适用的识别系统上各一个；
- 任务成功后，等待它所发布事实的自动分析器在同一系统（或整个检材）上建任务；所需事实都要有成功的发布者；同一作业已有过任务时不再自动创建，自动链因此不会循环；
- 失败或取消不创建后续任务；files 作用域只由用户发起。

运行开始时，`run.start` 的 `profile` 带上整个检材与该目标已发布的事实，同名时目标自身的事实优先。

## 任务调度

调度器（`src-tauri/src/core/extensions/task_scheduler/`）把任务保存在案件台账 `case.db` 的 `task` 表中：

- **状态**：`created → queued → running → success | failed | cancel`，`offline` 为可恢复的中间态。任务不能暂停：取消即结束执行，重新分析从头开始。旧案件里处于 `pause` 的任务行可以开始（从头再跑）或取消。
- **作业唯一**：同一作业最多一个未结束的任务；`sched_task_create` 命中时返回已有任务（`created: false`）。整个检材的作业另有台账的部分唯一索引兜底。
- **并发**：派发循环按创建顺序领取 `created` 任务，排队与运行中的任务总数不超过 `LOVELYCARVE_TASK_MAX_RUNNING`（缺省 4，最多 32）。
- **取消与重启**：取消运行中的任务会撤销其令牌并结束它的插件进程，同插件的其他任务继续；被替代的执行迟到的结果被丢弃。终态任务重启时创建新任务（`info.parentTid` 指向原任务），离线任务重新排队。只有终态任务可以删除。
- **看门狗**：运行中的任务在更新超时（`LOVELYCARVE_TASK_UPDATE_TIMEOUT`，缺省 900 秒）内没有进度时判为失败（通知 `taskTimeout`）；插件进程消失超过离线超时（`LOVELYCARVE_TASK_OFFLINE_TIMEOUT`，缺省 60 秒）时判为离线（通知 `taskOffline`），自动任务重新排队、手动任务判失败。打开案件时，上次会话遗留的 `queued` / `running` 任务按同样规则处理。
- **收尾动作**：检材的全部任务结束后去抖 5 秒，发出 `allTasksFinished` 通知并执行 `task_followup.actions` 中的动作。
- **任务中心**：插件任务与内置任务一起由 `task_list` 列出（插件任务的 id 为 `plugin:<任务 id>`，条目带完整任务行），变更走 `task-entry` 事件：`{ kind: "changed", entry }`、`{ kind: "removed", id }` 或 `{ kind: "notice", notice }`；`task_cancel` 也接受 `plugin:<任务 id>`。

任务行的 `params` 是作业目标与选项（`nodeId` / `fileName`、`systemId` / `targetLabel`、`options`），`result` 是摘要、记录数、时间点数、告警数、读不出的源字节数与运行 ID。

## 文件系统插件

声明 `[[filesystem]]` 的插件包为浏览提供一个共享实例（与任务实例分开），沿用冻结的方法：

- `fs.probe`：识别逻辑源，返回文件系统、置信度与可选标签；置信度 0 表示不匹配。
- `fs.open`：创建会话并返回不透明根节点；`fs.children`、`fs.children_page`（每页 1000 项，单目录上限 1,000,000 项）、`fs.stat`、`fs.read_range`（大范围按 4 MiB 分块）、`fs.close`。
- 插件 → 宿主：`source.info`、`source.read`（单次 4 MiB）、`source.readv`（最多 128 段、合计 8 MiB 减 4 KiB）。

元数据调用 15 秒超时，目录调用 60 秒，打开与读取 120 秒；超时的请求被取消。插件节点 ID 在到达前端前包上插件与会话身份，宿主不让插件解释宿主的路由 ID。共享实例按需启动；崩溃时状态记为 `crashed`，下次使用时重新启动。

## 进程边界与资源限制

`lc-plugin-host` 以清空的环境变量启动（只保留定位系统库与临时目录所需的少数变量），Windows 上不带控制台窗口，并加入 Job Object：关闭即终止、单进程、清单声明的内存上限。所有平台都有 kill-on-drop、RPC / 帧 / 并发上限、超时、句柄归属与读取范围检查。非 Windows 构建尚无原生的进程树或内存沙箱，不受信任的插件应部署在操作系统沙箱中。

句柄（文件、视图、租约、查找游标）属于发起的运行；运行结束、任务取消、插件重启或检材卸载时全部失效。

## Rust SDK

`plugins/sdk`（crate `lovelycarve-plugin`）实现 ABI 与协议，`plugins/sdk-derive` 提供 `#[derive(Record)]`：

```rust
use lovelycarve_plugin::{export_plugin, Plugin, Record, Result, Run, Timestamp};

#[derive(Record)]
#[record(id = "demo.user", label = "用户", title = "{name}")]
struct User {
    #[field(label = "用户名", key)]
    name: String,
    #[field(label = "UID")]
    uid: i64,
    #[field(label = "最后登录", timeline = "login")]
    last_login: Option<Timestamp>,
}

fn base(run: &Run) -> Result<()> {
    let view = run.require_view()?;
    if let Some(passwd) = view.open("etc/passwd")? {
        for (index, line) in passwd.read_lines(1 << 20)?.iter().enumerate() {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() < 7 {
                run.warn_file(&passwd, "MALFORMED_LINE", format!("第 {} 行字段不足", index + 1))?;
                continue;
            }
            run.record(&User { name: fields[0].to_owned(), uid: fields[2].parse().unwrap_or(-1), last_login: None })
                .from(&passwd)
                .note(format!("第 {} 行", index + 1))
                .emit()?;
        }
    }
    if let Some(hostname) = view.read_text("etc/hostname", 256)? {
        run.provide("hostname", hostname.trim())?;
    }
    Ok(())
}

fn plugin() -> Plugin {
    Plugin::new("org.example.demo").record::<User>().analyzer("base", base)
}

export_plugin!(plugin);
```

- `#[record(id, label, title?, category?)]`；`#[field(label, key, hidden, timeline = "…", kind = "…", choices = "…", rename = "…", skip)]`；字段类型由 Rust 类型推断（`String`、整数、浮点、`bool`、`Timestamp`、`Vec<String>`、`Json`，`Option<_>` 表示可缺失），`kind` 可在同一表示下选更具体的类型（`size`、`duration`、`path`、`hash`、`ip`、`enum`）；枚举取值写成 `kind = "enum", choices = "success=成功,failure=失败"`。只在运行时才知道列的解析器使用 `DynamicRecord` 与 `Plugin::schema`。
- `Run`：`view` / `require_view`、`system`、`inputs`、`options`（`list`、`wants`、`flag`、`text`）、`profile`（`timezone`、`hostname`）、`record` / `emit` / `emit_from`、`provide`、`warn` / `warn_file`、`progress`、`set_summary`、`check_cancelled`、`log`。记录在 SDK 中按结构校验后成批发送。
- `View`：`stat`、`exists`、`is_file`、`is_dir`、`list`（自动分页）、`open`、`read` / `read_text` / `read_lines`、`find`（最多 4096 条结果）、`source`。
- `FileRef`：`read_at`、`read_all`、`read_text`、`read_lines`、`readv`、`reader`（`Read + Seek`）、`materialize`、`source` / `source_at`。
- `Timestamp`：`from_unix`、`from_unix_millis`、`from_unix_nanos`、`with_offset`、`local`（时区未知的本地时间）、`parse`。
- 文件系统：实现 `FileSystemParser` 与 `Volume`，以 `Plugin::filesystem` 注册。
- 测试：feature `testing` 的 `Harness` 通过真实的 C ABI 入口扮演宿主，在本地目录上运行分析器（`Fixture::system`、`Fixture::evidence`、`Fixture::files`），并按结构校验每条记录。

插件 crate 是 `cdylib`，`[lib] name` 与清单的 `library` 一致。插件代码在宿主工作线程上运行，可以阻塞；宿主取消运行后，所有宿主调用返回 `CANCELLED`，纯计算循环应轮询 `check_cancelled()`。

## 应用接口

| 命令 | CLI | 用途 |
| --- | --- | --- |
| `plugin_list` | `plugins list` | 插件包、分析器、记录类型、语言包、加载失败的包与安装目录 |
| `plugin_install` / `plugin_uninstall` | `plugins create` / `plugins delete` | 安装 `.lcpkg`、卸载已安装的插件包 |
| `plugin_restart` | `plugins restart` | 重启文件系统插件的共享实例 |
| `sched_task_functions` | `plugins functions list` | 可调度的分析器：作用域、平台、选项与画像事实 |
| `sched_systems_list` | `plugins systems list` | 检材上识别出的系统及其挂载 |
| `sched_task_create` / `cancel` / `start` / `restart` / `remove` | `plugins tasks …` | 任务生命周期 |
| `task_list` / `task_status` / `task_cancel` | `tasks list` / `get` / `cancel` | 任务中心：插件任务与内置任务的统一列表 |
| `analysis_result_tree`、`analysis_records_page`、`analysis_record_detail`、`analysis_run_get` | `analysis tree get`、`analysis records list` / `get`、`analysis runs get` | 结果树、按列排序与检索的分页记录、记录详情、运行审计 |

前端经 `src/lib/ipc/plugins.ts`、`tasks.ts`、`taskRuntime.ts`、`analysisResults.ts` 调用。界面：「设置 → 插件」安装、卸载并查看插件包；任务中心的「快速分析」按所选系统列出系统级与检材级分析器；文件列表右键「插件分析任务…」运行文件级分析器（有选项时先填写）；「分析结果」按分析器运行列出记录类型，表格列与排序来自记录结构，运行信息显示审计。

## 旧版本数据

- LPP v1 插件包（`protocol_min` / `[[capabilities]]` / `[task]` / `[library]` 表）不再加载，需用 SDK 0.2 重新构建。
- 旧案件中 v1 插件写进检材 `analysis.db` 的结果在结果树中以“旧版”标记只读展示：没有字段类型、排序只按文本、来源取自旧的 `source.*` 属性，也没有运行审计。新运行一律写入 `records.db`，旧结果不迁移、不改写。
- 旧任务行中的平台与子项参数在读取时忽略；旧结果的 `artifactCount` 读作记录数。

## 工作区插件

`plugins/` 是独立的 Cargo 工作区（Git 子模块）：`sdk`、`sdk-derive`、`host`（`lc-plugin-host`）、`linuxinfo`（随安装包附带的 Linux 系统分析，见 [LINUXINFO.md](LINUXINFO.md)）与 `vehicle`（汽车取证，以 `.lcpkg` 安装）。

- `just build-plugins [--release]`：构建工作区，把各插件库复制到其 `plugin.toml` 旁，并写出 `schema.json`；`--plugin <目录>` 只构建一个插件库（沿用已构建的加载器）。
- `just pack-plugins`：为每个工作区插件生成可安装的 `.lcpkg`（含说明、许可证、构建信息与 SHA-256 校验），输出到 `dist-pack/`。
- `just test-plugins`、`just clippy-plugins`：测试与检查工作区。
- `just run-example probe_plugin <镜像路径> <插件 ID> <分析器> <输出 JSON> [--system <系统 ID> | --file <卷键> <路径>]`：在临时案件中走完整的任务流程（加载器、视图、输入、`records.db`），输出已发布的记录与运行审计。
- `just run-example probe_evidence_path <镜像路径> <卷键> <路径>...`：以插件看到的检材视图读取路径。

可选的真实镜像测试：设置 `LOVELYCARVE_TEST_LINUX_IMAGE` 指向本地 Linux 镜像并运行 `just build-plugins` 后，执行 `just test linuxinfo_analyzes_every_linux_system_of_a_real_image -- --ignored --nocapture`；设置 `LOVELYCARVE_TEST_LCPKG` 指向 `just pack-plugins` 生成的插件包后，执行 `just test built_package_installs_and_starts` 检查它能否安装与启动。私有检材与生成的结果不进入版本库。
