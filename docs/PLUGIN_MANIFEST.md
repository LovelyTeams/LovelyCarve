# 插件配置参考：plugin.toml

本文按当前宿主的清单解析器说明 `plugin.toml` 的字段含义、类型、默认值和校验范围。插件调用协议、SDK 与资源限制见 [LPP 协议](PLUGIN_PROTOCOL.md)，运行中手动检查目录的流程见 [插件发现方案](PLUGIN_DISCOVERY.md)。

实现依据：[清单解析与校验](../src-tauri/src/core/extensions/plugin/manifest.rs)、[任务功能注册表](../src-tauri/src/core/extensions/task_scheduler/functions.rs)、[快捷分析筛选](../src/features/tasks/composables/useQuickAnalysis.ts)。下文区分“解析器允许的值”和“建议使用的值”；推荐名称不等于固定枚举。

## 1. 最小可用清单

以下示例声明一个检材级分析器；它的动态库还需要通过 SDK 实现并注册 `example.baseinfo`，导出 `lovelycarve_plugin_v1`。

```toml
id = "org.example.baseinfo"
name = "示例基本信息"
version = "1.0.0"
protocol_min = 1
protocol_max = 1
permissions = ["emit_artifacts"]

[library]
path = "example_baseinfo"

[[capabilities]]
type = "evidence_analyzer"
analyzer = "example.baseinfo"
display = "示例基本信息"
```

Windows 部署目录为：

```text
plugins/
└── example-baseinfo/
    ├── plugin.toml
    └── example_baseinfo.dll
```

插件目录名不必等于 `id`、`analyzer` 或动态库名。插件身份由 `id` 决定，动态库位置由 `library.path` 决定。清单本身不会编译源码，也不会生成插件功能实现。

## 2. 通用类型与命名规则

| 规则 | 实际允许范围 | 示例 |
| --- | --- | --- |
| 插件 ID | 非空，仅 ASCII 大小写字母、数字、`.`、`-`、`_`；可表示为 `^[A-Za-z0-9._-]+$` | `org.example.baseinfo` |
| 小写标识符 | 非空，仅 ASCII 小写字母、数字、`.`、`-`、`_`；`^[a-z0-9._-]+$` | `example.baseinfo`、`baseinfo`、`linux` |
| 扩展名 | 非空，仅 ASCII 小写字母、数字、`-`、`_`；`^[a-z0-9_-]+$`，不含点 | `db`、`sqlite` |
| function 名 | `<插件 ID>/<analyzer>`；左侧遵守插件 ID 规则，右侧遵守小写标识符规则 | `org.example.baseinfo/example.baseinfo` |
| 布尔值 | TOML 的 `true` 或 `false`，不是字符串或整数 | `single_instance = true` |
| 字符串数组 | TOML 字符串数组；每项再按所属字段校验 | `permissions = ["emit_artifacts"]` |

当前没有统一的字符串长度、能力数量或数组长度上限；不要将此理解为适合提交任意大的清单。推荐采用短且稳定的名称，插件 ID 使用反向域名形式，analyzer 在同一插件内保持唯一。

可选字段通过**省略字段**使用默认值；TOML 没有 `null`。`""` 不等于省略，大多标识符字段不接受空字符串。

## 3. 顶层字段

顶层字段必须写在 `[library]`、`[task]` 等表之前；写在表后会属于该表。

| 字段 | 类型 | 必填 / 默认 | 含义与实际校验范围 |
| --- | --- | --- | --- |
| `id` | 字符串 | 必填 | 插件唯一身份，遵守插件 ID 规则；宿主加载时检查它与动态库握手返回的 ID 一致 |
| `name` | 字符串 | 必填 | 展示名称，可用中文；去除首尾空白后不能为空 |
| `version` | 字符串 | 必填 | 展示与分析结果记录的版本；去除首尾空白后不能为空；目前不强制语义化版本，不据此自动下载、升级或选择同 ID 插件 |
| `protocol_min` | 整数 `u16` | 必填 | `1..65535`，必须不大于 `protocol_max`，且不大于宿主当前 LPP 版本 |
| `protocol_max` | 整数 `u16` | 必填 | `1..65535`，必须不小于 `protocol_min`，且不小于宿主当前 LPP 版本 |
| `permissions` | 字符串数组 | `[]` | 仅接受第 5 节中的三种权限；同一个权限不能重复 |
| `capabilities` | 表数组 | `[]` | 用 `[[capabilities]]` 声明一个或多个能力；类型见第 6 节。空数组可以通过清单校验，但不提供业务能力 |
| `library` | 表 | 必填 | 动态库位置和隔离方式，见第 4 节 |
| `task` | 表 | 可省略 | 插件级调度与展示元数据，见第 7 节 |

当前宿主 LPP 版本为 **1**，所以有效范围必须包含 `1`；一般写 `protocol_min = 1`、`protocol_max = 1`。清单中的这个范围是 LPP 协议范围，不能代替 C ABI、操作系统或 CPU 架构兼容性检查。库必须适配实际运行平台和架构，握手时仍会检查 ABI 与协议。

推荐 `version = "1.0.0"`，并与 SDK 中的插件版本保持一致。当前宿主的清单一致性检查针对插件 ID 和能力类别，不会严格比较清单与动态库返回的名称、版本，也不会在发现阶段验证所有 analyzer 的实现。

## 4. `[library]`

| 字段 | 类型 | 必填 / 默认 | 含义与实际校验范围 |
| --- | --- | --- | --- |
| `path` | 字符串 | 必填 | 插件目录内的相对路径；不能是空白、绝对路径，不能包含父目录跳转等非正常路径组件；推荐使用 `/` 分隔子目录 |
| `resources` | 字符串数组 | `[]` | 附带分发的依赖库或数据文件，路径相对插件目录，不能为空或跳出目录；构建资源使用 `/` 分隔，逐文件声明，不支持目录或通配符 |
| `isolation` | 字符串枚举 | `"process"` | 仅 `"process"`、`"in_process"`；注意清单使用下划线，前端 IPC 展示值可能为 `inProcess` |

`path` 的最后一段若带 `.dll`、`.so`、`.dylib` 扩展名（识别扩展名时不区分大小写），则使用写出的文件名。否则补当前平台的动态库前缀、后缀：

| 写法 | Windows | Linux | macOS |
| --- | --- | --- | --- |
| `path = "example"` | `example.dll` | `libexample.so` | `libexample.dylib` |
| `path = "bin/example"` | `bin/example.dll` | `bin/libexample.so` | `bin/libexample.dylib` |
| `path = "custom.dll"` | 原样使用 `custom.dll` | 原样使用 `custom.dll` | 原样使用 `custom.dll` |

指定扩展名不会转换动态库的平台格式。第三行只是路径解析规则，不表示 Windows DLL 能在 Linux 或 macOS 加载。

`process` 由主程序旁的 `lc-plugin-host` 加载动态库；插件故障不会直接终止主程序，任务取消可以终止对应 worker。`in_process` 在主程序内加载，崩溃影响主程序，取消依赖插件配合，库加载后不会卸载。详细资源限制见 [LPP 的隔离与资源限制](PLUGIN_PROTOCOL.md#process-boundary-and-resource-limits)。

例如 `resources = ["helper.dll", "data/map.json"]` 会把这些文件保留相对位置放入插件包；不会把源码和整个目录默认打包。该字段声明分发文件，不会自动加载依赖库或数据。

运行时路径解析支持子目录和显式文件名，但当前仓库的 `build-plugins` 构建复制脚本要求 `library.path` 为**不带路径和动态库扩展名的 crate 库名**，例如 `example_baseinfo`。在插件 workspace 中开发时应采用这个较窄的规则，并确保 Cargo `[lib] name` 与之匹配。源码包还必须属于插件 workspace，并构建为 `cdylib`。

## 5. 权限 `permissions`

| 值 | 含义 | 不声明时的行为 |
| --- | --- | --- |
| `"file_materialize"` | 允许逻辑文件通过 `file.materialize` 生成宿主管理的临时文件，供需要原生路径的库读取 | 物化请求被拒绝；逻辑只读文件访问不需要这项权限 |
| `"emit_artifacts"` | 允许分析结果返回 artifacts、timeline、findings | 返回上述非空结果时校验失败；仅 summary 的结果不需要此权限 |
| `"network"` | 声明网络使用意图，显示在设置页，并传给进程宿主 | 当前不是 OS 网络防火墙；不声明不等于 OS 保证插件无法联网 |

插件发现即信任，没有额外批准步骤或信任库。权限是宿主 API 契约；进程隔离也不是完整的操作系统权限沙箱。临时文件的数量、大小和生命周期限制由宿主统一管理，不能在清单中任意放宽。

## 6. `[[capabilities]]`

每一个 `[[capabilities]]` 是独立表，必须包含 `type`。当前解析器接受 `filesystem`、`file_analyzer`、`evidence_analyzer`、`test`，其他值不接受。

### 6.1 文件系统：`type = "filesystem"`

| 字段 | 类型 | 必填 / 默认 | 含义与允许范围 |
| --- | --- | --- | --- |
| `type` | 枚举 | 必填 | 固定为 `"filesystem"` |
| `filesystem` | 字符串 | 必填 | 文件系统名称，遵守小写标识符规则；须与插件探测结果、SDK 实现一致 |
| `priority` | 整数 `i32` | `0` | `-2147483648..2147483647`；文件系统探测先比较置信度，再比较优先级，数值越大越优先 |

```toml
[[capabilities]]
type = "filesystem"
filesystem = "vendorfs"
priority = 100
```

文件系统能力走按需探测、打开和浏览流程，**不会**因 `[task]` 声明变成可调度分析任务。

### 6.2 文件分析：`type = "file_analyzer"`

| 字段 | 类型 | 必填 / 默认 | 含义与允许范围 |
| --- | --- | --- | --- |
| `type` | 枚举 | 必填 | 固定为 `"file_analyzer"` |
| `analyzer` | 字符串 | 必填 | 分析器 ID，遵守小写标识符规则；须在动态库中实现并注册 |
| `extensions` | 字符串数组 | `[]` | 支持的扩展名元数据，每项遵守扩展名规则；写 `"db"`，不能写 `".db"`、`"DB"`、`"tar.gz"` 或 `"*"` |
| `mime_types` | 字符串数组 | `[]` | MIME 元数据；当前仅检查 ASCII、包含 `/`、不含空白；推荐标准的 `type/subtype`，如 `"application/vnd.sqlite3"` |
| `priority` | 整数 `i32` | `0` | `-2147483648..2147483647`；传入功能注册表的优先级元数据，当前可调度功能清单按 function 名排序 |

```toml
[[capabilities]]
type = "file_analyzer"
analyzer = "browser.sqlite"
extensions = ["db", "sqlite"]
mime_types = ["application/vnd.sqlite3"]
priority = 10
```

function 名为 `<id>/<analyzer>`。文件分析需要目标文件；快捷分析面板当前只列检材级能力，不列文件级能力。显示名固定由 `<插件名称> · <analyzer>` 生成；文件级能力当前没有 `display` 字段。

`extensions` 和 `mime_types` 是声明元数据，不能视为所有调用路径都会强制执行的文件类型限制。显式调用分析器时，宿主会核对 analyzer 声明，但当前不会依据这些数组拒绝不同类型的文件；插件实现需要校验实际输入。

### 6.3 检材分析：`type = "evidence_analyzer"`

| 字段 | 类型 | 必填 / 默认 | 含义与允许范围 |
| --- | --- | --- | --- |
| `type` | 枚举 | 必填 | 固定为 `"evidence_analyzer"` |
| `analyzer` | 字符串 | 必填 | 分析器 ID，遵守小写标识符规则；须在动态库中实现并注册 |
| `display` | 字符串 | 省略时生成 | 功能显示名；省略时为 `<插件名称> · <analyzer>`；当前不额外校验非空，建议提供非空中文名称 |
| `priority` | 整数 `i32` | `0` | `-2147483648..2147483647`；功能注册表元数据，当前不是任务执行先后顺序控制 |

```toml
[[capabilities]]
type = "evidence_analyzer"
analyzer = "linuxbaseinfo"
display = "Linux基本信息"
```

function 名同样为 `<id>/<analyzer>`。这种能力接收整个检材的只读逻辑句柄，不需要单个目标文件，也不接收原始检材宿主路径。检材级能力没有 `extensions`、`mime_types` 字段。

### 6.4 测试类别：`type = "test"`

当前解析器为协议测试保留此值，没有业务字段，不注册文件系统或分析任务。普通插件不要使用它代替实际能力；当前常规 SDK 不会自动通告 `test` 能力。

同一个插件可以声明多个能力。建议每个文件系统名称、analyzer 名称唯一，尤其不要让文件级和检材级 analyzer 共用相同 ID，因为两者会产生相同 function 名。当前清单解析器不检查能力名称重复。

## 7. `[task]`：插件级任务元数据

这张表作用于插件内**全部文件级、检材级分析能力**。当前没有逐 capability 的任务配置覆盖，也不支持在同一清单内给不同 analyzer 分别设置依赖、平台或分类。

| 字段 | 类型 | 必填 / 默认 | 含义与实际校验范围 |
| --- | --- | --- | --- |
| `dependence` | 字符串 | 省略：无前驱 | 单个前驱 function 名，遵守 `<插件 ID>/<analyzer>` 规则；不能用插件 ID 代替 function，不能写空字符串或数组 |
| `trigger` | 字符串枚举 | `"manually"` | 仅 `"manually"`、`"auto"`；手动创建任务始终记录手动触发，自动根任务在证据上线时创建 |
| `internal` | 布尔 | `false` | 内部任务标记；快捷分析隐藏这类功能，任务列表查询默认可排除它；不是禁止执行或访问控制 |
| `single_instance` | 布尔 | `true` | 同一 `(function, evidence)` 只保留一个未终结任务，重复创建返回已有任务；不是整个插件全局只能运行一次 |
| `can_pause` | 布尔 | `false` | 是否允许暂停；恢复会重新执行，不表示插件自动支持断点续跑 |
| `type` | 字符串 | `"fileanalyzer"` | 任务分类，遵守小写标识符规则；不是固定枚举；文件级与检材级当前均使用此默认值 |
| `platform` | 字符串 | 省略：全平台 | 检材平台元数据，遵守小写标识符规则；不是运行动态库的宿主 OS 限制；前端也将 `"all"`、`"any"` 当成全平台 |
| `support` | 字符串数组 | `[]` | 功能子项名称，每项去除首尾空白后不能为空；可用中文；当前没有去重或自动生成实现 |
| `evidence_types` | 字符串数组 | `[]`：全部 | 适用检材类型元数据，每项去除首尾空白后不能为空；不限定枚举，也不会自动转成小写；前端按原字符串精确匹配 |

### 7.1 触发、依赖和暂停

`trigger = "auto"` 且没有 `dependence` 才是自动依赖链根任务。前驱成功后，调度器为依赖它的 function 创建后继任务；**当前后继匹配不再按清单的 `trigger` 过滤**，所以声明 `dependence` 的 function 即使写 `"manually"`，也可能因前驱成功而自动创建。前驱失败、取消不会创建后继。

`dependence` 只能声明一个前驱。解析器仅校验名称格式，不在加载清单时验证前驱是否存在、依赖是否成环；开发者应避免自依赖和循环。后继会继承前驱的目标参数，链中能力应能处理对应目标。

手动创建可以直接指定已注册 function；`dependence` 不是“手动执行前必须成功”的权限门槛。自动触发分析任务与自动发现插件是两件事，`trigger` 不控制扫描插件目录。

`single_instance` 约束针对任务身份，不区分不同目标文件或参数。暂停、离线任务仍未终结，可能阻止创建同 function、同检材的新任务；任务成功、失败、取消后可以再次创建。`can_pause = true` 的插件应能安全重新运行，清单不能替实现提供检查点恢复。

### 7.2 分类 `task.type`

以下名称有现成中文标签，其他合法小写标识符也可用，会按原名展示：

| 建议值 | 中文展示 |
| --- | --- |
| `baseinfo` | 基本信息 |
| `system` | 系统记录 |
| `browserinfo` | 浏览器 |
| `application` | 应用程序 |
| `loginfo` | 日志 |
| `databaseinfo` | 数据库 |
| `fileanalyzer` | 文件分析 |
| `filesystem` | 文件系统 |
| `analysis` | 数据分析 |
| `longtime` | 耗时任务（快捷分析分类放在最后） |

这里的 `filesystem` 只是任务分类标签，不会注册文件系统能力。检材级基本信息插件建议显式填写 `type = "baseinfo"`，避免使用默认的“文件分析”分类。

### 7.3 平台 `task.platform`

快捷分析已有标签和顺序的值是 `android`、`harmony`、`ios`、`windows`、`macos`、`linux`、`iot`。检材识别还可能返回 `huaweihmos`、`othermobile`。其他合法标识符可用，会按原名称增加平台项；省略、`all`、`any` 表示全平台。

此字段指**被分析检材的平台**。例如，Windows 上运行的 `linuxinfo.dll` 可声明 `platform = "linux"`，表示它分析 Linux 检材，而非只能在 Linux 宿主上加载。

### 7.4 检材类型 `task.evidence_types`

值需要与检材识别回写的 `evidence.type` 一致，不能直接填写 `.e01`、`.zip` 这类源文件扩展名。

| 常见值 | 对应类型 |
| --- | --- |
| `disk` | 磁盘 / 手机镜像识别路径 |
| `compressfile` | 未识别为特定备份的归档 / 目录检材 |
| `backup` | 通用备份 |
| `iosbackup` | iOS 本地备份 |
| `huaweiotgbackuptotar` | 华为 OTG 备份 |
| `samsungmachinegtool` | 三星 Smart Switch 备份 |
| `wechatbackupgtool`、`wechatbackup2gtool` | 微信手机助手备份包 |

这不是封闭枚举。当前类型来源见 [备份识别](../src-tauri/src/core/analysis/identify/autodetect/backup.rs) 和 [检材类型生成](../src-tauri/src/core/analysis/identify/autodetect.rs)；新增识别类型时以代码中的回写字符串为准。

平台与检材类型目前主要用于快捷分析筛选。检材类型尚未识别时，快捷分析不按 `evidence_types` 过滤。后端手动建任务及自动依赖链不把这些声明作为强制执行限制，插件仍应检查输入是否适用。

### 7.5 子项 `task.support`

`support = ["系统信息", "用户列表"]` 声明功能范围。它不会自动生成分析代码，也不是输出结果表的列配置。当前快捷分析按整个插件提交，发送空的 `subItems`，约定表示全部子项；定向执行由任务参数及插件实现共同处理。结果树、列和工件字段通过分析响应声明，见 [LPP 分析结果契约](PLUGIN_PROTOCOL.md#file-analyzer-methods)。

## 8. 完整检材级示例

```toml
id = "org.example.linuxinfo"
name = "示例 Linux 信息"
version = "1.0.0"
protocol_min = 1
protocol_max = 1
permissions = ["emit_artifacts"]

[library]
path = "example_linuxinfo"
isolation = "process"

[task]
trigger = "manually"
internal = false
single_instance = true
can_pause = false
type = "baseinfo"
platform = "linux"
support = ["系统信息", "用户列表"]
evidence_types = ["disk"]

[[capabilities]]
type = "evidence_analyzer"
analyzer = "linux.baseinfo"
display = "Linux 基本信息"
priority = 0
```

这个示例没有前驱；需要依赖另一个分析器时，在 `[task]` 添加例如 `dependence = "org.example.prepare/prepare.root"`，并保证前驱存在、目标参数兼容。

实际内置插件可参照 [linuxinfo/plugin.toml](../plugins/linuxinfo/plugin.toml)。运行、构建及测试使用根目录 `just` 配方，见 [开发指南](DEVELOPMENT.md#插件与资源)。

## 9. 发现、错误与当前限制

当前应用启动时按顺序扫描：用户应用数据目录的 `plugins/`、可执行程序旁的 `plugins/`、`LOVELYCARVE_PLUGIN_DIR` 指定的额外目录；开发版再扫描当前目录下的 `plugins/` 和仓库 `plugins/`。目录不存在时跳过。

扫描通常只找根目录的直接子目录中的 `plugin.toml`，不递归搜索所有层级。如果指定的根目录自身就含 `plugin.toml`，当前扫描器把它作为一个插件，不再扫描其子目录。同 ID 插件按扫描顺序保留第一个合法清单，后续重复项写日志后忽略；同一根目录内候选路径先排序。不会按 `version` 自动选最新版。

发现阶段解析清单、检查动态库文件是否存在且非空，并检查文件元数据在读取前后是否变化；不执行动态库。能否加载、ABI 是否匹配仍在首次使用时确定。`configured` 状态不代表动态库已验证。非法清单、不可读目录和缺库通过检查报告展示；运行中手动检查及界面诊断见 [插件发现流程](PLUGIN_DISCOVERY.md)。

| 问题 | 当前含义 / 处理方式 |
| --- | --- |
| 缺少必填字段、TOML 结构错误 | 清单无法解析；检查字段所属表，布尔和整数不要写成字符串 |
| 未知字段、未知权限、未知 `isolation`、未知 capability `type` | 清单解析失败；使用本文列出的字段和值 |
| `protocol_min > protocol_max` 或不包含宿主版本 | 清单校验失败 |
| 非法 ID、analyzer、扩展名、依赖名 | 清单校验失败；按第 2 节命名规则修正 |
| `library.path` 指向外部路径 | 清单校验失败；改为插件目录内相对路径 |
| 动态库缺失或空文件 | 发现时报告；新插件不发布，已有定义保留并显示诊断 |
| 平台不匹配、依赖 DLL 缺失 | 首次加载时报错；部署对应平台的库及其依赖 |
| 同 ID 插件出现在多个目录 | 仅第一个合法清单生效；检查扫描路径及日志 |
| 没有业务能力 | 清单可被发现，但不会产生分析功能或文件系统入口 |
| 库更新后点击“重置” | 只重建实例，不保证替换已加载库代码；更新库应退出应用后替换并重启 |

清单、`[library]`、`[task]` 和能力表均拒绝未知字段，拼错字段不会静默回退。文件级能力不接受 `display`，检材级能力不接受 `extensions` 或 `mime_types`。旧 `[process] command = "python"` 脚本插件格式不受支持，必须使用当前 `[library]` 动态库格式。

插件进程超时、内存限额、文件读取大小、租约限额、任务并发等不是这里的可配置字段；见 [LPP 协议](PLUGIN_PROTOCOL.md) 的宿主限制和调度器配置。
