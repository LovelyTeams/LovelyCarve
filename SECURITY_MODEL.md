# 安全模型

本文记录 LovelyCarve 与 [LovelyCarve-Plugins](https://github.com/LovelyTeams/LovelyCarve-Plugins) 的资产、信任边界和风险判断依据。漏洞报告渠道与支持范围见 [安全策略](SECURITY.md)；插件接口与限制见 [LPP 协议](docs/PLUGIN_PROTOCOL.md)。本文描述安全目标和当前边界，不代表所有输入路径已通过安全验证。

## 系统与保护目标

LovelyCarve 是本地取证工具：Vue / TypeScript 界面运行在 Tauri v2 WebView 中，由 Rust 后端读取检材、解析存储结构、预览内容、维护案件和索引、搜索、雕刻及导出。独立 CLI 提供 shell 与标准输入输出 JSONL 会话；[Python 读取库](python/README.md) 只暴露镜像、文件系统和归档的只读接口，不初始化桌面或 CLI 运行时；`integrations/dsh-lovelycarve/` 将自动化请求接入 CLI。LPP 原生插件由独立的 `lc-plugin-host` 进程加载。

需要保护的资产包括：

- **检材完整性与结果准确性**：源检材不被修改，缺失或短读不能静默充当真实内容，恢复、索引、插件记录与来源关联保持正确。
- **案件机密性与完整性**：原始与解密内容、路径、文件名、账户信息、案件数据库、记录、索引、审计、缓存、临时预览和导出。
- **凭据**：解锁密码、解密密钥，以及检材或分析环境中可接触的其他凭据。
- **分析工作站**：分析人员权限下的文件、进程、执行环境与可用性。

Windows 是当前记录的发布平台，Unix 分支也属于代码的安全评估范围。现有仓库没有建立生产网络服务或基础设施的部署边界；不能据此推断外部部署已具备额外隔离。

## 组件与代码范围

下列英文名称作为安全问题的稳定组件标识，跨主仓库与插件仓库复用。命令包装层的问题按根因所属子系统归类。表内主仓库的 `core/`、`commands/`、`app/`、`host/`、`cli/` 和 `bin/` 路径相对 `src-tauri/src/`；其他路径相对对应仓库根目录。

| 组件标识 | 代码与边界 |
| --- | --- |
| Desktop and CLI | `src/app/`、共享组件与 composable、`src/lib/ipc/`、其他功能界面，以及 `src-tauri/src/commands/`、`app/`、`host/`、`cli/`、`bin/`、状态和任务管理；Tauri 配置、`integrations/` 与 CLI 技能。负责操作人员、WebView、自动化请求到本地后端的边界。专用预览、案件、导出和插件命令归相应组件。 |
| Evidence engine | `src-tauri/src/core/evidence/`、`core/filesystem/`（导出除外）、`core/carving/`、`src-tauri/vendor/` 与 `python/`；外部镜像、归档、备份、磁盘、阵列、加密容器和文件系统元数据进入原生读取器。 |
| Preview | `src/features/preview/`、`src/views/`、`src/utils/safeHtml.ts`、`src-tauri/src/core/preview*`、`commands/preview/`、`state/preview.rs`、`host/preview_window.rs`；检材内容进入原生预览解析器、临时资源、WebView 和文档渲染器，也包含可选 OCR / QR 路径。 |
| Case and analysis | `src-tauri/src/core/casework*`、`core/catalog/`、`core/analysis/`、`core/search/`、`core/timeline/` 与 `commands/casework/`；案件、索引、记录、搜索、审计和时间线。 |
| Export and mount | `core/filesystem/evidence_session/export*`、`core/filesystem/recovery/export.rs`、`core/filesystem/tree/fs_tree_export*`、`app/workflows/evidence/export.rs`、`commands/filesystem/filesystem_export.rs`、`core/platform/`（均在 `src-tauri/src/` 下）及 `src/utils/exportFormat.ts`；检材数据进入主机输出路径、CSV、恢复文件与只读挂载。 |
| Plugin runtime | 主仓库 `src-tauri/src/core/extensions/`、`commands/extensions*`、插件设置界面，以及插件仓库 `host/`、`sdk/`、`sdk-derive/`；包发现与安装、动态库 ABI、LPP、反向 RPC、句柄归属、取消和进程生命周期。 |
| Linux analyzer | 插件仓库 `linuxinfo/`；Linux 检材中的用户、登录、命令历史、服务、计划任务与网络等文件进入类型化取证记录。 |
| Build and release | 两仓库的依赖清单、锁文件、构建脚本、justfile，主仓库 `.github/` 与打包配置；依赖和构建输入进入应用、CLI、Python wheel、插件宿主和插件产物。 |

主仓库 `plugins/` 固定插件仓库的提交；单独评估插件仓库默认分支时，不能假定它与应用打包的版本相同。

## 输入、攻击者与信任边界

### 检材提供者与受控源系统

镜像、分区表、文件系统元数据、目录名、链接、压缩流、文档、数据库和日志文本均可能被攻击者控制。打开、浏览、预览、校验、搜索、导出和插件分析等正常操作会到达这些输入。分析人员主动选择检材，不表示信任其中的内容。

主要跨界路径是：

1. 外部字节进入 Rust 读取器、解压器与解析器。
2. 提取内容进入预览渲染器、HTML / DOM、临时文件与资源协议。
3. 检材中的路径和取值成为主机路径、SQL、CSV、日志或插件记录。
4. 插件经 LPP 请求检材读取，结果回到宿主案件和界面。

越界读取、任意主机写入、非预期代码执行和凭据泄露属于安全问题。失控的资源消耗与静默损坏取证结果也需要按实际可达性和影响评估。

### 操作人员、案件与自动化

本地操作人员可以选择案件、检材、导出目录并发起 CLI / IPC 请求；这些操作以应用的操作系统身份执行。WebView 中的检材内容不应因此获得操作人员权限；从渲染内容到后端命令或主机执行的利用链必须单独确认。

案件文件、索引和自动化请求也是输入。是否交换外部案件、是否在共享工作站运行，以及是否以管理员身份运行，取决于实际部署；未明确这些条件时，不把磁盘上的输入或所有本地用户统一视为可信。

### 已发现或安装的原生插件

[安全策略](SECURITY.md#执行边界)与[插件协议](docs/PLUGIN_PROTOCOL.md)明确采用“发现即信任”：合法插件包被当作可信原生代码执行。独立进程用于承载、取消、资源与故障隔离，**不是操作系统权限沙箱**。`network` 权限声明联网意图，不是防火墙。

因此，只有“可信插件主动使用当前用户本就拥有的文件或网络权限”这一性质，属于已记录的信任假设。不能把它扩大为以下风险的接受：

- 外部检材攻破原本可信的插件或解析器。
- 安装包解压越界、非预期动态库选择或替换，以及绕过正常可信安装的执行路径。
- 跨运行句柄混用、读取范围失效或插件结果污染其他案件。

插件包、宿主和依赖的分发来源需要可信；仓库并未定义一个可安全执行任意恶意原生插件的环境。

## 当前控制与验证边界

下列控制来自实现与项目文档，不等同于所有调用路径均已正确应用：

- `src-tauri/tauri.conf.json` 与 `src-tauri/capabilities/default.json` 设置 CSP、WebView 能力和初始为空的资源协议范围；预览资源授权、HTML 处理与 IPC 可达性仍需沿具体路径验证。
- 检材按只读访问；边界检查、解压和分配预算、短读处理及取消由各读取器和任务负责。只读不代表没有主机写入：案件、索引、缓存、临时预览和导出各有输出目录。
- `core/extensions/plugin/package_install.rs` 在暂存目录解包，限制条目数、单条目和总展开大小，拒绝越界路径和链接，校验后替换安装目录。
- `core/extensions/plugin/client/` 清理继承环境并应用允许列表，管理帧 / RPC 上限、超时及 kill-on-drop；`core/extensions/plugin/run_context.rs`、`core/extensions/plugin/source_registry.rs` 等维护运行内的文件、源与视图句柄。句柄归属、失效与范围检查需与具体方法一并核对。
- `core/extensions/plugin/process_guard.rs` 在 Windows 使用 Job Object 限制单进程、内存并随关闭终止；其他平台没有同等的原生进程树和内存限制。这些限制均不构成权限沙箱。
- `core/casework/case_journal.rs` 的哈希链提供内部一致性校验；截断末尾需要在案件外保存最新哈希才能发现，不能把它当作独立真实性证明。
- 前后端共享 CSV 转义约定；动态 HTML 使用 `src/utils/safeHtml.ts`。新增输出路径需验证实际经过相应处理，不能只因存在辅助函数就认定安全。

上面的 `core/` 路径均相对 `src-tauri/src/`。架构与格式能力的详细边界分别见 [架构说明](docs/ARCHITECTURE.md) 和[格式文档](docs/formats/README.md)。

## 风险判断与待确认条件

从发现问题时确认的攻击前提、可利用性与影响开始判断优先级。仅当本模型已明确的资产价值、攻击者或接受风险改变这些前提时才调整；代码阅读中推测的部署情况不能当作已确认事实。高价值资产不自动使所有问题变成高危；困难或尚未确认的利用链也不能仅凭潜在后果提高优先级。

未排除任何一般攻击面或攻击者类型；未提供基础设施代码只意味着部署模型不完整，不意味着接受相关风险。仍需结合实际环境确认运行权限、操作系统隔离、敌对本地用户、外部案件交换、发布凭据与签名管理、组件所属团队。

问题可按上述组件、适用 CWE、已确认适用的 CVE 与所属团队归类。没有团队映射时标为 `Unassigned`，不根据个人提交者名称推定团队；不存在对应 CVE 时不填造编号。系统边界、插件信任策略或部署假设改变时，同步维护本文及相应安全策略。
