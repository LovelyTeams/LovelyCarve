# LovelyCarve 架构说明

LovelyCarve 是一个 Tauri v2 桌面取证工具，另有共用同一后端的无窗口 CLI。前端使用 Vue 3、TypeScript、Vite 和 Composition API；后端使用 Rust、Tauri command、异步任务和纯 Rust 的镜像、文件系统与加密容器读取器。

主流程是：创建或打开案件，添加镜像文件、文件夹证据、物理磁盘或 RAID 成员盘，检测分区和文件系统，浏览文件列表与原类型预览，执行哈希、导出、解锁、卷快照与索引，再运行全局搜索、时间线、文件雕刻和插件分析。

原镜像缺失时，`image_load` 可恢复当前案件已发布的文件系统索引：校验索引格式、检材与卷身份、构建时间和指纹后，沿用案件检材 ID 建立 `index_only` 会话。目录分页、过滤与元数据查询仍由只读 filedb 提供；原镜像的分区几何未知，且所有字节读取都明确失败，不触发自动快照、哈希或平台识别。文件预览默认显示索引属性，独立窗口同步这一状态。恢复原路径的镜像后重新打开检材，会替换离线会话并恢复正常读取；没有有效当前索引时仍报告打开失败。

离线会话的分析来源定位通过已验证的当前卷索引解析卷入口、祖先链与卷内路径；LVM 逻辑卷可位于分区索引的子树中，无需原盘分区几何。解析保留符号链接跳转限制与删除项唯一性要求，目录未被索引时提示恢复原镜像，定位成功后仍只提供索引属性。

使用与构建入口见 [文档导航](README.md) 和 [开发指南](DEVELOPMENT.md)。本文先按代码结构、证据读取、任务与插件、案件存储与索引、分析功能组织后端实现，再记录主窗口的界面与交互约定；各格式的实现范围与读取器维护要求见 [格式与读取器](formats/README.md)。

主要章节：[前端结构](#前端结构)、[后端结构](#后端结构)、[镜像与证据源](#镜像与证据源)、[任务与事件](#任务与事件)、[插件任务调度](#插件任务调度)、[插件运行时](#插件运行时)、[案件数据库](#案件数据库)、[元数据索引](#元数据索引层filedb)、[时间线](#时间线)、[文件雕刻](#文件雕刻)、[界面与交互](#界面与交互)、[功能与 IPC 对照](#功能与-ipc-对照)、[扩展流程](#扩展流程)。

```text
┌────────────────────────────────────────────────────────────┐
│                    Tauri v2 Desktop App                     │
│                                                            │
│  Vue 3 + TypeScript + Vite        Rust backend              │
│  src/App.vue                      src-tauri/src/lib.rs      │
│  src/AppShell.vue          IPC    commands::*               │
│  components/composables ────────► core::*                   │
│  src/lib/ipc/*             events state / tasks / cases     │
│                                                            │
│  WebView2 / WKWebView / WebKitGTK                           │
│                                                            │
│ raw/dd/img/iso/e01/ad1/vmdk/qcow2/vdi/vhd/vhdx/dmg/ldbk/tar/tar.gz/zip/7z/folder │
└────────────────────────────────────────────────────────────┘
```

## 前端结构

前端采用应用装配、按功能归属的业务模块、共享 UI/能力和类型化 IPC。当前公开接口与资源生命周期见 [前端模块与生命周期](FRONTEND_STRUCTURE.md)。

```text
src/
├── main.ts                 Vue 入口
├── App.vue                 主窗口根组件：主壳 + Toast/确认框宿主
├── AppShell.vue            唯一主壳装配点：标题栏、侧栏、标签页及面板连接
├── app/                    bootstrap、主壳组件、workspace 与跨功能 workflows
├── features/               cases/evidence/files/preview/search/carving/timeline/analysis/bcp/tasks/raid/settings
│   └── <功能>/             components、composables、model 与就近测试；按需提供 api.ts
├── components/             ui/feedback/layout/display 共享组件
├── composables/            通用通知、确认、主题、动效与持久化；不引用业务状态
├── views/                  独立 WebView 窗口（文件预览弹窗）
├── lib/ipc/                唯一 Tauri IPC 边界
├── lib/models/             文件定位等跨功能纯契约
├── utils/                  格式化、安全 HTML、CSV 等共享纯函数
├── i18n/                   中英文词典与纯翻译函数；语言偏好由 composables/useLocale.ts 持有
├── styles/themes.css       light/dark/pink 三套主题 token
├── styles/ui.css           全局共享 UI 基元：按钮、输入框、分段/标签切换、徽章、卡片、弹窗壳、菜单、空态、面板标题
└── styles/motion.css       动效层：共享关键帧、基元入场、ui-stagger 依次入场、主题圆形展开
```

重要约定：

- 组件和 composable 不直接 import `@tauri-apps/api/core` 或 `@tauri-apps/api/event`。
- 命令调用走 `src/lib/ipc/client.ts` 的 `invokeCmd`，事件走 `src/lib/ipc/events.ts` 的 `onEvent` / `emitEvent`。
- 新增前后端交互时，同步更新 Rust command、`src-tauri/src/lib.rs` 注册、`src/lib/ipc/*` 类型封装和调用方。
- UI 文案中文优先；样式使用 `src/styles/themes.css` 的 token。控件统一用 `src/styles/ui.css` 的 `ui-*` 基元（`ui-btn`、`ui-input`、`ui-badge`、`ui-modal-*` 等），组件 scoped 样式只写布局和组件特有的视觉；基元均为单类选择器、状态与修饰写在 `:where()` 中，scoped 规则可直接覆盖。结构化文件视图（PE / ELF / Mach-O / ar / GGUF）共用 `report-viewer.css`，SQLite / Realm 数据库视图共用 `db-browser.css`，均通过 `<style scoped src>` 引用。
- 前端规范（设计原则、token 与对比度、字号与间距、基元目录、交互状态与反馈、表格、弹窗与层级、图标、动效、文案、可访问性与自查清单）见 [FRONTEND.md](FRONTEND.md)。
- 原生数据表格统一添加 `data-table` 类，并通过 `<style scoped src="../../../styles/data-table.css">` 引用文件列表基准样式（路径按实际层级调整；12px 字号、36px 常规行、吸顶表头、统一悬停/选中态）。行内 `ui-btn link` 操作（定位、恢复、详情）平时为次要色，悬停、选中或聚焦该行时才显示主题色。列宽、排序和取证状态留在各组件；数据库字段类型和键值字节预览允许按内容增高。分析结果采用固定列布局（序号 64px、名称 240px、动态字段 200px），多字段时横向滚动，不随记录长度或详情开合挤压列宽。文件列表、搜索与时间线的虚拟列表行高需与公共样式同步，操作按钮限制在 24px，占位行不加边框和内边距。
- 文件列表面板的内部 id 是 `files`。
- 「rich」原类型视图（`features/preview/components/RichPreviewViewer.vue`）用 `@file-viewer/vue3-full`（Flyfish File
  Viewer，当前 3.1.2），按 `name` prop 的扩展名懒加载渲染器；worker / WASM / 字体等重资产由
  `@file-viewer/vite-plugin`（`vite.config.ts` 中 `copyAssets: { mode: 'build' }, inject: false`）
  复制到 `dist/file-viewer/`（随 frontendDist 进 Tauri 包）。开发环境通过
  `scripts/copy-file-viewer-assets.mjs` 按包版本同步到 `public/file-viewer/`，运行时相对
  `{base}/file-viewer/` 取用，不依赖外网。升级使用 `just update-file-viewer <version>`，
  同步固定 Vue 组件和 Vite 插件版本、更新锁文件并刷新开发资源；升级后运行
  `just typecheck`、`just test-fe` 和 `just build-fe`。

IPC 边界由脚本检查：

```bash
just ipc-check
```

等价要求：`rg "@tauri-apps/api/core|@tauri-apps/api/event" src --glob "!src/lib/ipc/**"` 必须为空。

应用层 `bindWorkspaceServices.ts` 持有案件、任务、分析、证据的注册回调和生产 disposer。任务/分析通过可替换的只读上下文订阅当前案件，避免直接引用案件单例造成循环。案件切换先关闭标签和视图，再等待标签防抖及已发出的派生数据写入，然后调用后端；请求代际防止旧读取、水合、选择器或订阅回填。迟到句柄即时释放，dispose 幂等；切页保留后台任务，不等于取消或卸载证据。

功能内部就近引用实现；功能之间只使用声明的 `api.ts`、组件入口或纯契约。共享层不引用业务，纯模型无响应式/宿主 I/O。`just frontend-structure-check` 使用 TS AST 和 Vue SFC 解析器检查两类 script、静态/动态/type-only import、别名、失效 import/style 路径、允许的跨功能边及运行时依赖环。允许边与所有权表见 [前端模块与生命周期](FRONTEND_STRUCTURE.md)，具体规则在 `scripts/lib/frontend-structure.mjs`。

主窗口与独立预览保持两个动态根；`app/workspace/panels.ts` 的七个面板仍懒加载，加载提示延迟 120ms。五类 KeepAlive 面板保持原 id 和证据复用规则；标签的五份 fileSession LRU 不是全部面板的状态快照。重格式 viewer 继续在详情内懒加载，公开状态 API 不导出 viewer。

## 后端结构

`src-tauri/` 保留单个应用 Cargo crate，按入口、应用工作流、命令、共享状态与核心域组织。基础证据读者不引用文件系统或插件实现，文件系统不引用 catalog 或插件宿主，持久化调度器不直接读写分析结果和时间线。案件发布、索引和路径模型仍按业务协作；十二个域不要求形成完全独立的 crate 或 DAG。

```text
src-tauri/src/
├── lib.rs                  Tauri builder、状态注册、invoke_handler!
├── main.rs                 桌面入口
├── bin/                    lovelycarve-cli 入口
├── cli/                    输入、运行、shell、工作流与生成的命令目录
├── app.rs + app/           setup、lifecycle、access、error、module_catalog
│   └── workflows/          证据加载/卸载、案件、快照、识别、Docker、挂载、外部卷和插件任务编排
├── state.rs + state/       AppState；预览、快照、遍历与运行状态辅助类型
├── task_manager.rs + task_manager/  即时任务登记、取消、资源、模型和测试
├── commands.rs + commands/ Tauri 传输层，按十个业务域组织
└── core/mod.rs             只声明十二个职责域
    ├── evidence.rs + evidence/     镜像、读者、加密、分区、RAID、物理盘
    ├── filesystem.rs + filesystem/ 卷适配、文件树、读写投影、Docker
    ├── catalog.rs + catalog/       filedb、目录查询、快照和容器索引
    ├── casework.rs + casework/     案件台账、审计、锁和检材存储
    ├── analysis.rs + analysis/     识别、结果查询和 BCP
    ├── carving.rs + carving/       雕刻引擎、扫描、格式和会话
    ├── search.rs + search/         全局搜索、关键词/正则和搜索存储
    ├── timeline.rs + timeline/     时间线投影、存储、USN 和 mactime
    ├── preview.rs + preview/       二进制、SQLite、表格和其他原类型预览
    ├── extensions.rs + extensions/ 插件宿主和持久化任务调度
    ├── platform.rs + platform/     FFI、Dokan、Windows 共享和资源
    └── common.rs + common/         哈希、自然排序和文本解码
```

`commands` 的十个域是 `evidence`、`casework`、`filesystem`、`preview`、`analysis`、`carving`、`search`、`timeline`、`extensions`、`system`。跨域流程在 `app/workflows` 中复用；应用层不调用命令层，`core` 不依赖 app、commands、state、cli 或 Tauri。应用错误定义在 `app/error.rs`，`commands/app_error.rs` 保留传输层导出。

`lib.rs` 保留 builder 和唯一的命令注册表。启动接线在 `app/setup.rs`，桌面退出和 CLI 共用 `app/lifecycle.rs` 的媒体、插件清理步骤，各入口保持原有收尾顺序。CLI 生成器沿模块声明解析嵌套命令路径，命令名、参数和帮助分组保持稳定。

CLI JSONL 调度位于 `cli/session.rs`。`session --jobs N` 使用有界输入队列和查询工作线程，主协调器统一发布按请求 ID 关联的 JSON 响应；stdin 等待不会阻塞完成结果输出。并发查询复制派发时的 Shell 上下文，案件／检材／目录切换及其他未列入并发清单的命令先等待已有查询完成。结构化 `$ref` 等待先前请求结果，EOF 收齐请求后再等待后台任务。新增命令默认是屏障；确认其生命周期与并发安全后，才加入查询清单并通过命令目录的 `parallel` 字段公开。

`src-tauri/examples/` 按用途分类，所有 example 目标在 Cargo 中显式声明，通过 `just run-example <名称>` 调用；`just check-examples` 检查这些目标，用法见 [后端探针](../src-tauri/examples/README.md)。新增模块使用 `<name>.rs` 加 `<name>/`，不新增 `mod.rs`。

`core::evidence::image::DiskImage` 负责镜像源、分区布局、字节读取、解密和定位缓存。`AppState.images` 登记的对象是 `Arc<core::filesystem::evidence_session::EvidenceSession>`：它持有同一个字节读者，并拥有宿主 Docker、NTFS 删除记录投影及外部卷注册表。字节扫描、RAID、哈希和源字节导出借用 `disk_image()`；文件树、预览和插件证据浏览借用 session。原证据 ID、路径、序列化字段和分区寻址保持不变。

`EvidenceSession::raw_export_reader` 提供独立的 RAW 导出投影。`image_export_raw` 默认 `mode: source`，桌面镜像头部使用 `unlockedPartitions`，导出开始时固定已解锁的 LUKS / VeraCrypt / BitLocker 读者。明文放在原分区起始位置，容器开销对应的分区尾部补零；整盘大小、分区表、未解锁分区和分区间隙保留原字节。范围重叠、溢出、越界、明文过长及短读明确失败，关闭解密会话不回退到密文。该投影不影响普通读取、源哈希和证据身份；尾部补零是导出转换规则，源坏区仍沿用读故障审计。导出目标以 `create_new` 创建。

外部卷通过 `filesystem::external_volume::ExternalVolume` 提供只读 Volume 和关闭契约；插件 `RemoteVolume` 实现这个接口。未知分区的插件探测、源登记和卷安装由 `app/workflows/filesystem.rs` 编排。卸载先撤销 AppState 登记、取消任务和停止挂载，再 drain 外部卷并逐项关闭、移除源和其他缓存；持有调用句柄的线程释放前，provider backing 继续存活。字节读者本身不持有插件连接。

`core/extensions/task_scheduler` 保留台账、状态机、派发、恢复、进度、看门狗和 post-action 契约。生产 `PluginTaskRunner` 位于 `app/workflows/scheduler.rs`，负责实例存活、执行、analysis.db 落库、时间线投影和结果清理；清理失败仍按原语义记录告警并完成任务删除。插件 SDK 保留 ABI/LPP 和 wire 结果，`core/analysis/results/models.rs` 保存经过宿主校验的 JSON/IPC 投影；插件解析器做转换和限额校验，存储不依赖插件宿主。

LVM 标签/CRC/活动副本位于 `evidence/partitions/lvm2/header.rs`，有界文本 grammar 与 VG/PV/LV/segment/stripe 模型位于 `text.rs`，完整线性 LV 字节映射与 read/seek 位于 `layout.rs`；文件系统与 RAID 共用。活动 ring 回绕、远端备份和成员 UUID 按同一读取规则处理，不以历史文本扫描替代损坏的当前副本。原生 ext/XFS/Btrfs/NTFS 读者支持单 PV 碎片段，来源 runs 映射回原始镜像；多 PV 线性 LV 仍通过 RAID 候选流程消费，缺失成员不缩短为可读卷。文件系统层构造导航、原始 LV 文件与未使用 PE 投影，RAID 不反向调用导航实现。完整 libvslvm API 对照与限制见 [LVM2](formats/LVM.md)。

无元数据 RAID 的数据偏移探测保留小偏移候选，并以 512 KiB 步长扫描各成员前 256 MiB；只有命中 MBR、NTFS 或 ext4 签名的偏移才进入重组搜索，偏移候选总数最多 32 个，随后仍须通过文件系统结构和目录／MFT 验证。ext4 验证最多检查 256 个 inode 和 24 个目录，继续进入共享目录的子树并核对 `.`／`..`，不要求根下存在三个目录；备份超级块及已知文件格式的正文签名优先用于区分早期目录同样可读的布局，文件扩展名与签名不一致只是不贡献分数。此路径覆盖 mdadm 头被清空而数据区仍在大预留区之后的 NAS 检材。`just run-example probe_raid --member-signatures <image>...` 可只读检查成员头尾和该范围内的 ext4 签名；真实 NAS 回归可设置 `LOVELYCARVE_NAS_FIXTURE_DIR` 后运行 `just test real_nas_erased_mdadm -- --ignored`。

读者入口保留公共模型、打开方式和源身份；具体实现按职责放在入口同名目录。Btrfs 分为 open/directory/data/tree/codec，NTFS 分为 backing/directory/deleted/scan/read/metadata 与 logical_stream/data_stream/wof/reparse/forensic_metadata/security/mft_metadata，ZFS 分为 directory/read；`fs_dispatch` 保留 Volume/FsAdapter 契约和原注册顺序，各特殊卷适配在 ntfs/ext4/btrfs/zfs 子模块。RAID 的 manual/detect/members/probe 与 heuristic 的 search/parity/validation/probe/output 分开，原探测优先级、搜索预算和取消检查不变。

插件 client 保留请求句柄和实例存活状态，process/connection/router/host_requests 分别负责启动、握手、消息路由和反向 RPC。Dokan 入口保留挂载会话和引用计数，filesystem/callbacks/paths/read 负责上下文、只读回调、路径和读取缓存。ReFS 通过本地 `refs-reader` crate 解析只读卷，filesystem 的 refs/backing/walk 分别负责节点适配、证据范围读取和可取消遍历；目录缓存随卷释放。普通文件、ADS 与快照版本按同一 extent 映射随机读取；platform/ffi 的旧实现只在显式 `refs-dll-oracle` 验证功能下编译。XLSX 预览分为 workbook/format/style，案件状态将检材登记和卷发布分别放在 case/evidence.rs、case/volumes.rs。相关原有测试与实现同域。

`commands.rs` 同时存放各命令文件共享的辅助函数。后端 CSV 导出（时间线、全局搜索）的数据单元格都经
`commands::quote_csv_field` 转义：以 `=`、`+`、`-`、`@`、制表符或回车开头的值前置 `'`（`-12`、`-3.5` 等纯负数除外），
防止检材取值在电子表格中作为公式执行；前端分析结果导出的 `utils/exportFormat.ts` `csvEscape` 使用同一规则。
新增 CSV 导出须复用二者之一。

`src-tauri/src/lib.rs` 是后端命令注册的唯一入口。当前已注册的主要命令域包括：

- 镜像：`image_load`、`image_info`、`image_unload`、`image_list`、`image_export_raw`
- RAID/物理磁盘：`raid_auto_detect`、`raid_manual_assemble`、`physical_disks_list`
- 文件雕刻：`carve_formats`、`carve_start`、`carve_scan_status`、`carve_scan_pause`、`carve_scan_resume`、`carve_scan_list`、`carve_scan_remove`、`carve_results_page`、`carve_preview`、`carve_recover`
- 文件系统：`fs_children`、`fs_flat_page`、`fs_flat_ext_counts`、`fs_read`、`fs_read_large`、`fs_media_temp`、`fs_preview*`、`fs_export*`、`fs_digest`、`fs_identify_type`
- 原类型预览：SQLite、Realm、二进制、GGUF、ZIP、CSV、Vim swap 等（后端解析）；文档/办公/邮件/压缩包/CAD/3D/图表等
  「rich」类别由前端 Flyfish File Viewer 渲染，字节经 `fs_read_large`（≤16 MiB，可取消且受任务并发限制的 ArrayBuffer）或
  `fs_media_temp` + asset 协议供给。PDF 支持 Range，单文件≤4 GiB；非流式富预览≤64 MiB，超限提示导出。
  `previewRequest` 为每个查看器分配 UUID，切换/卸载时取消读取并调用 `fs_media_release` 释放缓存引用，迟到响应同样释放。
  同一 `(image,node,ext)` 的物化互斥，完成前使用唯一临时文件，单写入通道约束瞬时磁盘占用。
  `MediaTempCache` 的 4 GiB / 6 文件预算只淘汰无人引用的最久未使用文件；独立窗口销毁由后端兜底释放引用。
  新缓存位于 `lovelycarve-preview-cache/<uuid>`，持有目录外的进程锁；启动只回收能取得锁的旧目录，退出清理当前目录。
- 卷/挂载：`fs_mount_readonly`、`fs_unmount_readonly`、`fs_mounts`、挂载点建议和创建
- BitLocker：状态、密码/恢复密钥/VMK 解锁、内存扫描、导出解密镜像
- 案件和快照：案件创建/打开/关闭、证据列表/移除、卷快照构建/列表/删除
- 插件与任务调度：`plugin_list`/`plugin_restart`/`plugin_analyze_file`/
  `plugin_analysis_list`；`sched_task_functions`、`sched_task_list`、`sched_task_create`、
  `sched_task_cancel`/`pause`/`start`/`restart`/`remove`、`sched_evidence_post_actions_set`
- 系统辅助：主题、任务取消、模块摘要、路径显示、二维码、OCR、OS 检测、Windows 盘符识别

若前端调用未注册命令，必须按功能清单处理为恢复后端、隐藏入口，或显示明确不可用说明。

## 核心数据流

1. `AppShell.vue` 通过案件管理器确保当前有活动案件。
2. 用户选择镜像文件、文件夹证据、物理磁盘或 RAID 成员。
3. 前端调用 `image_load`，后端把证据注册到内存状态和活动案件。

4. 侧栏显示镜像信息与文件列表，文件树按需调用 `fs_children` 或快照索引。
5. 文件详情通过 `fs_preview`、`fs_preview_text`、原类型预览命令读取内容；「rich」类别由前端
   `RichPreviewViewer`（Flyfish File Viewer）自取字节（`fs_read_large` / `fs_media_temp`）渲染。
6. 长任务通过 Tauri event 推送进度，例如镜像加载、哈希、导出、BitLocker、快照构建。

## 镜像与证据源

支持的主要源类型：

| 类型 | 后端路径 | 说明 |
| --- | --- | --- |
| RAW / DD / IMG / ISO | `core::evidence::image` + `vendor/smraw` | 单文件 mmap，分段 RAW 纯 Rust positioned I/O 与所有段稳定性检查；ISO 由 CDFS 探针识别，边界见 [SMRAW](formats/SMRAW.md) |
| E01 / Ex01 | `core::evidence::readers::ewf_image` → vendored `ewf` (SecurityRonin/ewf-forensic) | 纯 Rust、多分卷、随机读取、v1 延迟块索引；无需 libewf DLL |
| VMDK | `core::evidence::readers::vmdk_image` + `vendor/vmdk` | vmdk-forensic 的纯 Rust flat / sparse / streamOptimized 读取；分卷与父盘源变化检测 |
| QCOW / QCOW2 | vendored `qcow2` | v1/2/3 稀疏/DEFLATE 虚拟盘、RAW/QCOW backing chains；legacy AES 通过 image_load 可选凭据打开，能力与边界见 [QCOW](formats/QCOW.md) |
| VDI | vendored `vdi` | VirtualBox VDI 1.1 动态盘与固定盘，只读稀疏随机访问；差分盘明确拒绝 |
| Parallels PHDI | vendored `phdi` | XML / .hdd bundle、v2 Plain / Sparse / Split / 差分父链；直接逻辑随机读、所有父链文件稳定性与指纹，见 [PHDI](formats/PHDI.md) |
| BitLocker | `core::evidence::crypto::bde_image` + `vendor/bitlocker` | 纯 Rust 密码/恢复密码/明文密钥/BEK/VMK/FVEK；六种 AES、Vista、EOW；直接读取分区，无 DLL 或临时整卷转储；验证边界见 [BITLOCKER](formats/BITLOCKER.md) |
| VHD / VHDX | `core::evidence::readers::vhd_image` → `vendor/vhd` / `vendor/vhdx` | 纯 Rust 固定/动态盘随机读取；VHDX 日志仅在内存重放，差分盘无父盘参数时明确拒绝 |
| DMG | vendored `udif` + `hfsplus` | UDIF 容器与 HFS+ |
| Xiaomi HDR1 firmware | `core::evidence::partitions::xiaomi_firmware` | 按字节偏移解析固件段，将其中可识别的文件系统作为卷浏览；原始容器字节保持不变 |
| LDBK | `core::evidence::readers::ldplayer_image` | 解包后按多分区盘处理 |
| TAR / TAR.GZ / ZIP / 7Z / folder | 逻辑证据源 | 无分区表，暴露为合成只读文件树 |
| PhysicalDrive | `core::evidence::physical_disk` | Windows 物理磁盘，通常需要管理员权限 |

分区识别不能仅凭首扇区末尾的 `0x55AA` 判断 MBR：NTFS 等卷引导扇区也含此标记。
MBR 解析先校验四个分区项的启动标志（`0x00` / `0x80`），拒绝将引导代码或提示文字解释为分区表；
未匹配分区表的镜像继续通过整卷文件系统探针识别，因此直接从 NTFS 卷起始位置采集的 E01 也可浏览。

GPT 由 `evidence/partitions/gpt` 的统一 Rust 路径读取主备头和完整条目数组。头结构与两项 CRC
有效的副本优先；损坏恢复保留告警，原始类型 GUID、属性、槽位与名称不因文件系统细化而丢失。
磁盘 GUID 与分区来自同一次选表；完整范围必须在源和声明可用区内，u8 分区 ID 不回绕。
没有权威几何的源可根据有效 GPT 推断 512/1024/2048/4096 字节扇区，已知容器/物理盘几何保持不变。
公开读取接口、恢复边界和完整 libvsgpt 对照见 [GPT](formats/GPT.md)。

Docker / OCI 归档解析同时支持 descriptor 直接指向 image manifest，以及多架构导出中递归嵌套的
image index（最多 8 层）。当归档包含多个候选时，优先选择 blob 实际存在、非 attestation 且匹配当前
CPU 架构的 manifest；同时存在 `index.json` 与 `manifest.json` 的混合归档在 OCI 图不完整时可回退旧式
manifest。`.tar` 加载先建立一次普通 TAR 成员索引，确认容器标记后把同一个 mmap 和成员位置交给 Docker
合层器；各层建索引通过 seek 跳过文件体，只读取 tar header，避免重复扫描外层归档或读穿层内文件内容。

`.tar.gz` / `.tgz`、Docker / OCI 的 GZIP 层与小米 `.bak` 中的 GZIP payload
使用 vendored `gzipf` 完整消费拼接成员并校验所有 CRC / ISIZE。TAR / 备份
解压到自动清理的临时文件，压缩层保留 8 GiB 解压上限；校验成功后才发布
文件树或开始备份提取。原证据路径保留，损坏 tar.gz 沿用原始字节 RAW 探测
回退。宿主接入、限额和验证边界见 [GZIP](formats/GZIP.md)。

普通磁盘镜像中的 Docker Engine 数据由 `core::filesystem::docker::host_docker` 惰性探测：只检查 Ext4 分区与 LVM 内的
Ext4 逻辑卷是否存在 `/var/lib/docker`，把发现的容器作为磁盘根下的“Docker 容器”合成节点。首次展开
某个容器时，`core::filesystem::docker::host_docker_overlay` 才解析 overlay2 的 `lower` 链，并按从底层到 init/可写层的
顺序组合目录；字符设备或扩展属性 whiteout 会删除低层路径，`overlay.opaque=y` 会遮蔽低层目录内容。
合层节点保留实际 Ext4 inode 作为内容来源，因此现有读取、预览、哈希、导出和数据段定位无需新增 IPC。
探测与每个容器的合层索引在 `EvidenceSession` 内缓存，源镜像始终只读。文件列表通过 `fs_docker_prepare`
先探测可复用状态；首次分析只准备点击的容器。`core::filesystem::docker::mapping_cache` 负责版本化、带检材指纹的来源映射，
仍写入案件 `evidence/<uid>/cache/docker/*.zst`；文件树只返回自己的节点、父节点和路径。
`core::catalog::host_docker_index` 将这些节点编码为目录快照，并构建该容器的 SQLite `filedb` 索引，经 staging 发布到
`evidence/<uid>/volumes/<卷名>/`。来源映射的格式、版本、指纹校验和原子替换保持原契约。
物理卷的 `vol_id` 保持不变，容器根排在物理卷之后；枚举容器元数据不触发其他容器合层。
映射与快照采用同目录临时文件、刷盘后原子替换，完整索引及快照引用发布后任务才成功。相同容器的准备
串行执行，失败可重试；无案件时仅保留会话结果。重新打开案件后，状态查询和内容读取惰性恢复映射，
目录浏览直接查询已落盘索引；指纹或格式版本不匹配、文件缺失/不可读、索引不完整时重新准备。
Dokan 挂载层直接解析
`host-docker:` 合成根和 `host-docker:<container-id>:` 容器根，避免为定位挂载节点先遍历物理分区；
两种视图均标记为大小写敏感的只读 `DOCKER` 文件系统，可由文件列表右键挂载为盘符或 NTFS 文件夹。
前端只在用户首次进入或展开 `host-docker:<container-id>:` 时触发合层，并在目录树显示不伪造完成比例的
紧凑加载条；子项计数的后台预取会跳过尚未合层的容器根。容器根属于导航与挂载入口，不参与“隐藏
空目录”判定；进入容器后，普通目录仍沿用已有空目录过滤规则。

### EWF 读取器

`core/evidence/readers/ewf_image.rs` 把 ewf-forensic 仓库的纯 Rust `ewf` 读取器适配为
`ImageSource::Ewf`，不加载 libewf。`EwfAcquiryInfo` 提供采集元数据，分卷路径、大小及
时间戳用于长任务的证据稳定性检查。v1 使用延迟块表和有界缓存，v2 使用
上游的完整块表。缺失分卷在打开时拒绝，解码 / 校验故障交由 tolerant read
层报告。上游版本及本地补丁见 `src-tauri/vendor/ewf/LOVELYCARVE.md`。

### 加密容器的会话视图

`core/evidence/crypto/encrypted_image.rs` 接入 vendored `luks-core` 与 `veracrypt-core`。
`EvidenceReader` 对源 `DiskImage` 持有弱引用，限制在用户指定的分区或字节区域内；
读取拒绝有损坏区域记录的密文。解锁后通过现有 `SectorSource` / `from_assembled`
建立按需解密的镜像，`source_format=decrypted`、路径 `decrypted://<uuid>`，
不生成整卷明文临时文件。`DiskImage.derived` 记录父镜像以支持递归卸载。

`commands/evidence/encrypted_volume.rs` 提供 `encrypted_volume_unlock`，使用 CPU 资源调度和
`encryptedVolumeUnlock` 任务生命周期事件；注册结果前再次确认父镜像仍在当前会话。
`image_load` 对解密路径只执行已注册会话查找，不写入案件检材清单。
解密视图与卷影副本统一记为 `DiskImage.derived`（`DerivedImage::Decrypted` / `ShadowCopy`），
卸载父镜像时一并卸载。
LUKS2 在 KDF 前通过 `core/evidence/crypto/luks_metadata.rs` 校验头部大小、校验和、数据段和 keyslot
范围及支持的布局；上游 reader 的密文读取改为严格读取，越界/截断均报错。
解锁参数不实现 `Debug` / `Serialize`，结果不包含凭据或主密钥。
LUKS 扇区密码套件由 vendored `sector_cipher` 统一分派，密码槽与数据段使用同一只读路径；
支持 AES CBC/ECB/XTS、Blowfish/Serpent CBC/ECB 和 ARC4 ECB，LUKS2 的 area key 与卷主密钥独立。
`masterKeyHex` 通过同一命令/任务建立会话，验证摘要后才暴露明文；凭据和恢复的主密钥缓冲区在释放时清零。
完整算法、IV、36 个 libluksde API 对照及边界见 [LUKS](formats/LUKS.md)。

### 卷影副本（VSS）的会话视图

`core/evidence/vss_image.rs` 接入 vendored `vsc-core`（`src-tauri/vendor/vsc`，来自 SecurityRonin/vsc-forensic）。
`list` 逐个检查 NTFS 分区及已解锁的 BitLocker 卷，读取卷内 0x1E00 处的 VSS 头与 catalog，
按创建时间（最早在前）列出卷影副本的存储区 ID、卷影副本 ID、创建时间、属性和计算机名；
有 VSS 头而无副本的卷也会列出（未创建或已被删除）。`open` 为所选副本构建该存储区及所有更新
存储区的块映射（`vsc::SnapshotLayout`），以 `SectorSource` 按需重建快照时刻的卷：
当前存储区没有的块依次向更新的存储区查找，再回落到当前卷，规则与 libvshadow 一致。
重建结果通过 `from_assembled` 注册为会话镜像，路径为
`vss://<父镜像 ID>/<分区>/<存储区 ID>/<标签>`，`source_format=shadow_copy`，
`ImageInfo.shadow_copy` 携带来源；镜像只在内存中按需重建，不生成临时文件。

读取父镜像时遇到坏区记录或短读即报错，不把补零的字节当作快照内容；父镜像关闭后快照读取失败。
存储区元数据损坏（越界、记录类型不符、链重复）时拒绝打开该副本。`commands/evidence/shadow_copy.rs` 提供
`shadow_copy_list` 与 `shadow_copy_open`；后者登记 `shadowCopyOpen` 任务、采用随机 I/O
资源调度，注册前确认父镜像仍在会话中，同一副本已打开时复用原会话。`image_load` 对 `vss://`
路径只查找已注册会话，不写入案件检材清单；RAID 检测不把卷影副本当作候选成员。
前端由 `useShadowCopies` 读取列表，镜像信息页在对应分区行显示副本数（有 VSS 区而无副本时显示「无卷影副本」），
展开后由 `ShadowCopyList.vue` 列出副本，打开后在新标签页浏览；解锁 BitLocker 后重新读取列表。
CLI 注册表提供 `shadow-copy-list` / `shadow-copy-open`，在同一会话中保留父镜像与副本。
读取器公开 `store_blocks` / `store_ranges` 原始枚举和 `Snapshot: Read + Seek`；完整 API 对照与
验证边界见 [VSS](formats/VSS.md)。catalog 损坏或循环时报错而不是返回部分列表，存储头整块读取并校验。
原始 catalog 尺寸保留用于调查，布局尺寸独立求值；异常时要求主备 NTFS 几何一致。

### AD1 逻辑镜像

`core/evidence/readers/ad1_image.rs` 接入 vendored `ad1-core`（`src-tauri/vendor/ad1`，来自 SecurityRonin/ad1-forensic）。
AD1 是 FTK Imager 的逻辑镜像（「Custom Content Image」），不是扇区镜像：一棵项目树、每项的元数据
（时间戳、MD5/SHA1）和按块 zlib 压缩的文件数据，可分成 `.ad1`、`.ad2`… 多个分段。打开 `.ad1` 时解析
整棵项目树，并要求头部声明的分段都在；直接打开后续分段会提示改选第一段，加密的 ADCRYPT 镜像明确拒绝，
扩展名是 `.ad1` 但内容不是 AD1 的文件按 RAW 打开。

`volumes/ad1.rs` 的节点 ID 是 `ad1:`（合成根）和 `ad1:<序号>`（深度优先序号；AD1 允许同名的兄弟节点，
所以不用路径）。每个 zlib 块可以单独解压，`FsSession::read_range_direct` 因此支持随机读取，只解压范围覆盖
的块；整文件流式读取在解出的字节少于声明大小时报错，不会产出看似完整的哈希或导出。AD1 的时间戳不带时区，
与 FAT 一样按 UTC 显示：修改、访问时间照常映射，0x09 键按 AD1-tools 的解释作为元数据变更时间
（`mft_changed`），不当作创建时间。

各分段按顺序映射成连续字节，十六进制视图和镜像哈希读取的是原始分段数据；打开时记录每个分段的大小与
时间，出具哈希或导出前据此检查源文件是否变化。读取器改为定位读取，`Ad1Reader` 是 `Send + Sync`，多个线程
共用一个实例；目录树遍历用两个 8 KiB 预读窗口，代替逐字段读取。

## 取证读故障审计

E01、VMDK、VHD/VHDX、PMFX 等容错后端遇到源读取错误时，会按扇区粒度补零并把绝对源盘区间记录为
`BadRegion`。`image_bad_regions` 返回当前镜像与活动案件中持久化的合并报告；源盘哈希、RAW 镜像导出、
单文件导出、目录树导出和 ZIP 目录导出都会在成功结果中返回 `bad_regions`、`synthetic_bytes` 和
`read_faults_detected`。文件/目录导出使用一次导出操作内的读故障收集作用域，报告的是该操作实际通过
`DiskImage::read_range` 命中的源盘坏区；它用于审计导出结果是否包含合成零字节，不表示逐个输出文件的
逻辑偏移到物理坏区映射。

## 任务与事件

长任务应返回任务 id 或明确结果，进度通过事件发送，取消走 `task_cancel`；任务状态可通过
`task_status(task_id)` 查询单个活动任务，或通过 `task_list()` 列出当前全部活动任务。
所有接入 `TaskManager` 的后台任务还会发出统一 `task-lifecycle` 事件，payload 包含
`phase`（`queued` / `running` / `cancellationRequested` / `finished`）和完整任务快照。
其中有细粒度进度的任务还会额外发出统一 `task-progress` 事件，payload 包含同一份任务快照、
`unit`（`bytes` / `entries` / `attempts`）、`done`、可选 `total` 和可选 `message`。旧的领域
进度事件仍保留给现有 UI，`task-progress` 用于跨任务聚合展示、诊断和未来通用任务面板。

后端长任务统一注册到 `TaskManager`，注册守卫负责在成功、错误、取消和 worker panic 后
自动清理。任务记录包含任务类型、可选的镜像归属、状态和已运行时间；状态为 `queued`、
`running` 或 `cancellationRequested`。`image_unload` 会先取消该镜像的全部关联任务，避免已捕获
`Arc<DiskImage>` 的后台工作在卸载后继续消耗 I/O。

`TaskManager` 还负责后台资源预算：哈希、镜像/目录导出、BitLocker、二维码、媒体物化和
快照构建会先取得全局后台任务 permit；需要顺序读取镜像的任务还会取得按镜像的
semaphore，默认同一镜像最多一个顺序 I/O 后台任务。任务快照的 `resourceClasses` 会标明
`globalBackground`、`imageSequentialIo`、`imageRandomIo`、`cpu` 或 `gpu` 资源类别；
媒体物化和快照构建归入随机 I/O，二维码归入随机 I/O + CPU，OCR 在 CUDA 构建下归入随机 I/O +
GPU，否则归入随机 I/O + CPU。资源获取按全局、镜像 I/O、CPU/GPU 的固定顺序执行，避免排队任务
提前占住同镜像 I/O permit。`fs_children`、`fs_flat_page`、`fs_stat` 等交互式读取不进入该后台队列；
自动识别平台（`autoDetect`）是有界的短探测，同样不取任何 permit，登记后立即运行，不排在快照构建之后。

## 插件任务调度

`TaskManager` 管的是进程内的即时后台操作；插件分析任务另走一套持久化调度器
`core/task_scheduler/`，以案件台账 `case.db` 的 `task` / `task_followup` 表为持久化来源
（见「案件数据库」）。插件、分析器、触发方式、父任务与失败原因各是一列，只有 `params` /
`result` 是 JSON；线上形态 `SchedTaskRecord` 仍把前几项收在 `info` 里。插件实例只在进程内
登记，不落库。

```text
core/extensions/task_scheduler.rs      模块入口
├── config.rs               SchedulerConfig（LOVELYCARVE_TASK_* 环境变量）与 SchedError
├── model.rs                状态机 created/queued/running/success/failed/cancel/pause/offline、
│                           任务行、EvidenceTaskStatus 即时统计、SchedEvent 广播
├── store.rs                台账 task 表（调度器自己的 WAL 连接）；store_evidence.rs 负责
│                           检材主键映射、任务状态统计与 task_followup 收尾动作
├── functions.rs            Function 注册表：每个 file_analyzer 能力 = 一个 function
│                           `<plugin id>/<analyzer>`，任务元数据取自清单 [task] 段
├── scheduler.rs            绑定/崩溃恢复、调度循环、created→queued 派发
├── lifecycle.rs            queued→running→commit、OnTaskFinish、离线分流、进度回流
├── control.rs              创建、取消、暂停、继续、重启、删除与证据上下线钩子
├── executor.rs             TaskRunner trait + PluginTaskRunner（worker 角色：每次执行一个插件实例）
├── dependence.rs           CheckDependenceTask / CreateNextTask / evidence firsttask
├── post_action.rs          PostActionWatcher：防抖、幂等、initFromDB 恢复
└── watchdog.rs             离线 / 进度超时巡检与恢复分流
```

生命周期：`sched_task_create`（或依赖链、证据首次上线）写入 `created` 行 → 派发循环每 500 ms
按创建顺序派发 `created` 任务（`queued`），同时处于 queued/running 的任务不超过
`LOVELYCARVE_TASK_MAX_RUNNING`（默认 4，最大 32，不区分插件）→ 执行前置 `running`、写开始时间
→ `PluginManager::start_task_worker` 为这次执行单独起一个插件实例（独立进程插件是
一个新的 lc-plugin-host 进程，各有 512 MiB 内存上限；进程内插件另建一个库实例），在其上执行
`analyzer.analyze` / `analyze_evidence`，同一插件的多条任务因此并行 → 该实例的 `host.progress`
只回流到这次执行：回报只更新内存登记并立即刷新看门狗
计时，派发循环把最新百分比落库（百分比变化即写、否则 1 s 节流），插件线程从不等待台账 → 结果
先持久化到检材的 `analysis.db` 再 commit `success`/`failed`（写结束时间，耗时由开始/结束时间结算），
实例随后退出 → OnTaskFinish：PostActionWatcher 计数、成功时按
`dependence == 本 function` 建下一级任务（同检材、继承目标文件参数、`parentTid` 链、
`trigger = auto`），失败/取消阻塞后续。每次状态或进度变化都广播 `sched-task-event`。
插件共享实例（文件系统浏览、`plugin_analyze_file` 直接分析）
与任务实例互不影响。

SQLite 写入：调度器对台账的读写全部在阻塞线程池上进行，台账忙时不占用异步运行时；状态迁移在一个
`BEGIN IMMEDIATE` 事务里先校验当前状态再写（`TaskStore::transition` / `complete`），并行任务的提交、
控制面与看门狗同时改写同一行时以先写者为准，迟到的一方不覆盖终态；进度只更新仍为 `running` 的行。
检材的任务状态统计（PostActionWatcher 的「全部完成」判定）由任务行即时计算，不另存聚合。任务行的写入与其
`sched-task-event` 广播在同一把顺序锁内完成，广播顺序等于写入顺序，前端不会在终态之后又收到旧的
"运行中"进度。`analysis.db` 结果落库与删除、时间线增量投影（`project_run`：已建且不在重建中的检查、
按 tid 替换、事件源水位推进）各是检材自己库上的一个 IMMEDIATE 事务：同一检材的任务同时收尾时在
该库写锁上排队，不同检材互不等待；水位取已有值与本 run 完成时间的较大者，钩子乱序提交也只前进。

控制面：取消允许 `created/queued/running/pause/offline`，运行中的执行撤销 token 并终止这次执行的
插件实例：独立进程插件结束其 lc-plugin-host 进程，进程内插件对
运行中的调用发协作式取消，同一插件的其他任务不受影响；迟到的旧 commit 被丢弃；暂停只对清单 `can_pause = true` 的
function 开放，LPP 尚无协作式暂停，因此暂停即终止执行，`start` 后从头重跑；重启终态任务会新建
一条 `parentTid` 指向原任务的任务，重启离线任务直接重排队；删除只接受终态。单实例约束
（`single_instance`，默认开）下同一 `(function, 检材)` 同时最多一条未终结任务，命中时返回已有
任务（`created: false`）；台账的部分唯一索引 `task_single_active` 兜底。移除检材时先取消它的
未终结任务（`withdraw_evidence`），删除检材行后任务随外键级联删除，调度器再逐条广播
`taskRemoved`（`forget_evidence`）。

看门狗按 `LOVELYCARVE_TASK_CHECK_INTERVAL`（默认 15 s）巡检运行中任务：超过
`LOVELYCARVE_TASK_UPDATE_TIMEOUT`（900 s）没有进度 → `failed` + `taskTimeout` 通知；该任务的插件
实例失效且超过 `LOVELYCARVE_TASK_OFFLINE_TIMEOUT`（60 s）→ `offline` + `taskOffline` 通知，再由离线处理器
分流：`auto` 任务重排队 `created`，手动任务判 `failed`。案件打开时把上一会话遗留的
`queued`/`running` 行置 `offline` 并走同一分流；案件关闭/应用退出时运行中任务置 `offline`。

PostActionWatcher 按检材登记创建/完成计数（案件打开时从台账恢复），某检材全部任务终结后启动
5 s 防抖，到期仍全部完成则广播 `allTasksFinished`，并执行 `task_followup.actions` JSON 里的
动作，全部成功后写 `done_at` 保证幂等；失败只记日志并广播 `postActionFailed`，下轮全部完成时重试。

前端：`src/lib/ipc/tasks.ts` 包装 `sched_*` 命令与 `sched-task-event`；`useTaskCenter` 是模块级
单例，按 id 合并事件、把通知转成 toast；右上角「任务中心」的任务页（`TaskCenterPanel.vue`）展示
分段任务、进度/剩余时间、错误与结果摘要、取消/暂停/继续/重新分析/删除操作与最近通知；
文件列表右键「插件分析任务…」（`PluginTaskDialog.vue`）按光标行文件选择 function 提交任务。

**快速分析。** 右上角「任务中心」的「快速分析」页签承载
`QuickAnalysisPanel.vue`，界面见上方「任务中心」章节。平台由 function 清单推导
（清单 `platform` 缺省、为 `all` 或 `any` 的功能克隆到每个平台；预选检材自动识别平台，并按检材记忆上次选择），
功能按插件归并，插件按收藏 / 各 Type 分组 / 耗时任务分类。确定时为所选插件的每个功能调用
`sched_task_create`（`params.platform` 为所选平台；不传子项，`subItems` 为空即插件执行全部子项；
逐个创建时不弹单条通知，结束后汇总一次）；勾选的插件已有结束的任务时先红字确认，
再逐个 `sched_task_remove` 删旧重建；「重新分析全部任务」确认后对检材任务逐个 restart/start。
组装逻辑（`preprocessFunctions`/`buildPlatforms`/`groupByPlugin`/`buildPluginGroups`）与勾选状态
（`useQuickAnalysisSelection`，按插件 id）在 `useQuickAnalysis.ts`；收藏按插件 id 记（旧版按功能名记的条目仍算其插件），
与平台记忆一起经 `usePersistedRef` 落 localStorage。

**检材级 function（evidence_analyzer）。** LPP v1 的 `evidence_analyzer` 分析整个检材：
任务不带目标文件 `nodeId`，执行器改走
`PluginManager::analyze_evidence` → 插件的 `analyzer.analyze_evidence`（携带任务
params JSON，30 分钟时限），期间插件经宿主 `evidence.roots/stat/list/read/readv`
以 POSIX 路径只读访问该检材的全部可浏览卷（owner-scoped、单次读 4 MiB、符号链接
跟随上限 16 跳；注册表在 `core/extensions/plugin/evidence_registry.rs`）。注册表为每个证据句柄维护
有界目录 LRU（最多 4096 个目录、65536 个节点，过大目录不缓存），路径解析与目录枚举共用；
证据移除、镜像卸载、插件退出时随句柄释放，不跨插件或证据共享。function 元数据
（`platform`/`support`/`evidence_types`/检材级标记）经清单 `[task]` 段与能力声明进入
`SchedFunction`，快速分析对话框据此渲染。`plugins/linuxinfo/`（Rust 动态库，随安装包分发）是首个此类插件：
`linuxbaseinfo`（Linux 基本信息，type=baseinfo、platform=linux、15 个子项）定位
全部 Linux 根卷（含容器合并卷）后解析 os-release/passwd/shadow/group/wtmp/btmp（utmp 384B 记录）、
systemd/init.d 服务与启动项、dpkg status/rpm rpmdb.sqlite、crontab、NetworkManager/
OpenWrt UCI/RedHat ifcfg 网络配置、var/log USB 行与逐条 Bash/Zsh 历史、时区、用户组及 Hosts 映射，产物与时间线落
检材的 `analysis.db` 并自动投影时间线；普通解析错误按子项容错，短读、资源上限和取消使任务失败，
不静默截断结果。无时区的 Syslog 时钟只保留本地时间，不猜测 UTC。
仓库仅内置 `plugins/linuxinfo`；有界完整文件读取位于该插件的 `evidence.rs`，
工件来源与验证方法见 [Linux 基本信息插件](LINUXINFO.md)。

**分析结果页。** 插件结果中的
`nodes` 声明（`{type, name, columns}`）
随 run 落 `plugin_result_nodes` 表；run 行带上 `display_name`/`type_name`（功能级
标签）。侧边栏「分析结果」面板（`AnalysisResultsPanel.vue`）三段式呈现：

- 左侧结果树（`analysis_result_tree` → `core/analysis_query`）：检材 → 分类
  （Type）→ 功能（analyzer）→ 结果项（artifact_type），每个 (检材, 功能) 只展示
  最新 run，数字徽标显示记录数；节点名/列定义全部来自插件提交，
  前端零硬编码，有记录但未声明列的类型按类型名合成节点。
- 中部记录表格（`analysis_records_page`）：按结果项服务端分页（每页 100），
  关键词过滤命中内容/标题，导出 CSV 复用启动时的过滤条件，逐页读取全部记录，不截断到 10 万条。
  常驻通知显示已读取数量与进度并提供取消，切页后仍可操作；结果数量或所属 run 变化、提前读到空页时停止并提示重试。
- 右侧详情（`analysis_record_detail`，带记录所属检材的 `imageId`）：点行展示记录值与全部属性键值。
- 任务完成只把树标脏（`useAnalysisResults` 监听 `useTaskCenter` 的新 runId），
  面板激活（KeepAlive `onActivated`）时整树重拉。

任务删除（`sched_task_remove`，快速分析去重流程依赖）现在级联删除该任务落库的
runs（外键级联清产物/节点/事件/线索）并按 tid 清时间线投影；移除检材时整个检材目录
（含 `analysis.db`）随之删除。

执行边界：worker 是插件实例（默认在 lc-plugin-host 子进程中，`in_process` 插件在主进程内）；可调度 function 有
文件分析与检材级两类（文件系统能力仍走懒探测）；证据只在首次登记时触发自动首任务
（分析结果已持久化，重新上线不重跑）；文件级 analyzer 的
`trigger = "auto"` 根任务缺少目标文件会立即判失败，`auto` 应用于依赖链上的下游
function（检材级 function 无此限制）；结果树一次整树返回（体量小），未按 pid 逐层
懒加载；任务重启产生的旧 run 保留到任务删除为止（树始终只显示最新 run）。

## 插件运行时

插件运行时位于 `core::extensions::plugin`。插件是导出 C ABI（`lovelycarve_plugin_v1`）的原生动态库，
清单 `[library]` 指定库文件与隔离方式：默认 `process` 由随应用分发的 `lc-plugin-host` 子进程加载，
宿主经 4 字节大端长度前缀的 MessagePack stdio 帧与之通信，独立 reader/writer loop 支持双向嵌套 RPC
（`client.rs`）；`in_process` 由主进程直接加载（`in_process.rs`）。两种方式由 `PluginConnection`
统一，承载相同的 LPP 方法与载荷。插件 SDK、C ABI 定义、线协议类型与 `lc-plugin-host` 在
`plugins/` Rust 工作区（`plugins/sdk` 同时是宿主依赖）。插件只获得 `SourceRegistry` 中的只读
逻辑源句柄，不能获得或直接打开证据路径。运行时提供握手、受限读取、分页、取消、进度与超时。
Unknown 分区在首次展开时通过插件管理器执行 `fs.probe` / `fs.open`，命中后安装 `RemoteVolume`；
其 children、stat、范围读取和流式读取继续复用原有 `Volume` / `FsSession` dispatch。

插件**发现即信任**：清单合法的插件初始即为可用（`Configured`），可直接探测、派发与分析；
没有批准步骤与禁用开关，插件目录 SHA-256 指纹仅用于设置页展示与版本标识（已加载的库不会卸载，
插件更新后重启应用即生效）。
`file_analyzer` capability 通过
owner-scoped `FileHandle` 对逻辑文件执行 `file.info` / `file.read` / `file.readv`，只在声明
`file_materialize` 权限时创建 host-owned 临时租约。插件返回的 artifact、timeline、finding 会经过稳定
LPP v1 DTO、大小/深度/类型校验和 `emit_artifacts` 权限检查；活动案件中由 `core::analysis::results::plugin_result_store`
事务写入 SQLite。插件列表、重启、文件分析和历史查询通过 `commands::extensions::plugin` 与
`src/lib/ipc/plugins.ts` 形成完整 IPC 链。协议、安全边界与限制见 `docs/PLUGIN_PROTOCOL.md`。

## 自动识别平台

`core/autodetect/` 负责判定检材所属平台、证据类型与备份特征，再回写 evidence。
证据首次上线时由宿主建立内部首任务，手动重新识别通过 `autodetect_evidence` 命令执行。两条路径都登记为
`TaskManager` 的 `autoDetect` 任务（id `autodetect:<image id>`，同时进行的两次运行共用一份登记与取消令牌）：
识别在分区、逻辑卷与归档目录之间检查取消，镜像卸载或 `task_cancel` 后不再回写；登记释放之前广播
`autodetect-finished`（报告）或 `autodetect-failed`（`{image_id, error}`，`error` 与命令错误同形，取消为
`cancelled` 码），任务中心据此判定终态。

```text
core/analysis/identify/autodetect.rs          数据模型（EvidencePlatform 词表 android/ios/windows/macos/linux/harmony/
│                           huaweihmos/othermobile、DetectRoute、PartitionPlatform、AutoDetectReport）、
│                           三路分发器、分区/LVM 逻辑卷遍历、归档条目走查、聚合
├── fstype.rs               磁盘镜像路：文件系统类型门限、标志文件组全匹配与
│                           四平台复核（macOS→Windows→Linux→Android 短路）
└── backup.rs               归档/备份文件路：detectTar/detectZip 特征文件表、跳过扩展名表、厂商/大类
                            关键词、adb `ANDROID BACKUP` 魔数（含小米 .bak 前缀）、huaweicheck.json
                            品牌白名单、备份类型到平台与厂商的映射、显示名称
app/workflows/identify.rs      `autodetect_evidence` 命令 + 证据首次上线的后台首任务 + `autodetect-finished` /
                            `autodetect-failed`
```

三路分发：磁盘镜像路逐分区（LVM 展开到逻辑卷，锁定的 BitLocker 单列）先按文件系统类型
选择标志文件组：NTFS → `/$MFT`、`/Windows/System32`、`/Users`、
`/Program Files`；HFS/APFS → `/bin /sbin /etc /var /usr /Applications
/System/Library/CoreServices/SystemVersion.plist /private/var/db/dslocal/nodes/Default/users`；
ext 系、UFS、XFS 与 Btrfs → `/boot /bin /etc/issue /home /usr /var`，
**全部存在**才算系统分区；再做平台复核：macOS 按类型门限检查两标志任一；
Windows 对 ReFS 直接命中、NTFS 看 `Windows/WINDOWS/Program Files/Program Files (x86)`
任一；Linux 无门限看 `etc/issue`/`boot/vmlinuz` 任一；Android 看 `data/com.android.camera` 快捷标志或
`system/data/misc/app/local` 全部存在。主平台优先取标志文件组与平台复核一致的系统分区，`platforms` 收全部
命中。归档/备份路（tar/zip/7z/AD1/docker/目录检材）对合成根做无类型门限的平台检查，并按包内条目做特征文件
（`huaweicheck.json`、`thunderforensic.json`、`kunlunmobile.json`、`backupinfo.ini`、`backupHistoryInfo.xml`、
`SmartSwitchBackupInf.bk`、`com.tencent.mm.*`、`weixin_resource.zip`、`com.tencent.mobileqq.*`、`info.xml`、
iTunes 清单）与关键词匹配；raw 打开的 `.ab`/小米 `.bak` 由文件头魔数直接判备份，加密未预填密码时报
"未预填密码或密码错误"。手机镜像路（小米固件容器、雷电备份）分区扫描后兜底 Android。

回写：有活动案件且检材已登记时，`evidence.platform` ← 平台词、`evidence.type` ← `disk` /
`compressfile` / 备份枚举词、`evidence.extra_info` ← 厂商 / 显示名 / 备份特征 / 告警 JSON、
`evidence.partitions` ← 逐分区判定 JSON，
并写 `evidence/autodetect` 审计日志；平台未命中时 `platform` 保持原值，
任务照常结束。`EvidenceRef` DTO 带出 `platform` / `evidence_type` 供案件管理器展示。

前端：`src/lib/ipc/autodetect.ts` 包装命令与事件；`useAutoDetect` 模块级缓存按镜像 id 存报告、接完成
事件并刷新活动案件 DTO，失败事件记为该检材的识别错误（取消不算失败）；镜像信息页头部的
`AutoDetectBadge.vue` 只显示结论（平台 / 类型 / 厂商 / 备份）并支持手动重新识别，逐分区判定、
标志文件明细、备份特征与告警不在界面展开。
每次识别在右上角「任务中心」的任务页显示为「自动识别平台」（见「右上角任务中心面板」）。

识别限制：macOS 两标志均缺失时不按卷名猜测；部分厂商备份归入 `backup` 大类，
iOS 整盘镜像没有独立判定路径。复核结论时查看案件回写的 `evidence.partitions`（逐分区判定与标志文件）
与 `evidence.extra_info`（备份特征与告警）。

## 案件数据库

一个案件就是一个目录，布局只在 `core::casework::case_paths` 定义。旧版 `.carve` 案件不迁移、不兼容：打开时
报错要求新建案件，建案时目录里已有旧案件也拒绝。

```text
<案件目录>/
  case.db                    案件台账（core::casework::case_ledger）：档案、检材、卷发布、任务、标签、盘符、
                             读故障、设置与哈希链审计
  case.lock                  进程独占锁（core::casework::case_lock，std::fs::File::try_lock）
  staging/                   构建中的快照与索引；打开案件时清空
  evidence/<uid>/            一检材一目录，uid = 检材 image id；移除检材即删除该目录
    facts.db                 文件哈希与类型识别（core::casework::facts_store）
    analysis.db              插件分析结果（core::analysis::results::plugin_result_store）
    timeline.db              时间线投影（core::timeline::timeline_store）
    search.db                全局搜索任务与命中（core::search::global_search_store）
    cache/docker/            可随时删除的缓存（Docker 容器来源映射）
    volumes/<卷名>/          一卷一目录（part:1 → part-1，过长的卷键截断后接哈希）
      tree-<token>.dirz      卷快照（目录树交换格式）
      index-<token>.db       由快照导入的文件系统索引，发布后不再修改
```

**按写者拆库。** SQLite 在 WAL 下读不等写，冲突只发生在同一个库文件的两个写者之间，所以库按「谁在
写、写多久」划分，而不是按业务领域：

- 台账只放与检材数量或人工操作次数同阶的行，每个写事务都是毫秒级的（登记检材、发布一个卷、改任务
  状态、打标签）；读-改-写一律 `BEGIN IMMEDIATE` 开局取写锁，事务里不做文件 I/O。台账写者之间只有
  短暂排队，忙等上限 10 s 足够。
- 与文件数量同阶的数据落在检材目录下，每个库只有一类写者：`facts.db` 由哈希/类型识别命令批量
  upsert，`analysis.db` 由插件任务提交（每次运行一个 IMMEDIATE 事务），`timeline.db` 由重建任务与
  插件收尾投影写，`search.db` 只由运行中的搜索写。不同检材的写入互不排队。
- 最长的写入——把百万行快照导入文件系统索引——只写 `staging/` 里一个还没有人读的私有文件，写完
  整体发布，发布后只读（`immutable=1` 打开）。导入再久也不占任何共享库的写锁，导入期间浏览、搜索、
  时间线照常读旧索引。
- 读路径（列表、分页、状态查询、导出）从不建库、不跑 DDL、不做无条件写：库不存在就按空结果返回；
  孤儿 `running` 行的修复只短等写锁，等不到就只在本次结果里按中断呈现。

**台账（`core::casework::case_ledger`）。** `application_id = 0x4C434344`（"LCCD"）标识这是 LovelyCarve 台账，
`user_version` 是结构版本，不是当前版本的台账拒绝打开。表：`case_profile`（唯一一行：案件 uid、编号、
名称、检验人、描述、逻辑锁、创建/更新时间）、`setting`（案件级 KV）、`evidence`（`uid` 唯一、
`path_key` 唯一——同一来源在案件里只有一条记录；自动识别的平台、类型与完整报告 JSON）、`volume`
（每个 `(检材, 卷)` 当前生效的快照与索引文件名、来源快照时间、源卷大小、证据指纹、条目数、截断）、
`tag`、`drive_letter`、`read_fault`、`task` / `task_followup`（见「插件任务调度」）与 `journal`。
外键一律 `ON DELETE CASCADE` 到检材：删除检材行即带走它的卷记录、标签、盘符、读故障与任务。
时间戳是 RFC 3339 UTC（毫秒、`Z` 结尾），字典序即时间序。

**审计（`core::casework::case_journal`）。** 建案、开关、改名、锁定、检材增删都与对应的审计记录在同一个台账
事务里提交。每条记录的 `hash = sha256(前一条 hash ‖ 长度前缀的各字段)`，链首是案件 uid；
`case_journal::verify` 重算整条链，篡改或删除中间记录都会断链。

**生命周期（`core::casework::case` / `commands::casework::case`）。** `case_create` 建目录、取进程锁、建台账（文件已存在
时拒绝，绝不覆盖）；默认编号 `LC-YYYYMMDD-xxxxxx`，检验人默认取系统用户名。`case_open` 取进程锁、
校验台账、载入检材与卷记录并自愈（`case_heal.rs`：清空 `staging/`、删除不属于任何检材的检材目录、
快照文件缺失的卷记录删除、索引文件缺失的卷下线索引、回收卷目录里记录之外的旧代文件与没有记录的卷
目录），再记 `case.open`。`case_close` 记审计、checkpoint 台账 WAL、释放连接与锁，再卸载运行态镜像。
`case_lock`/`case_unlock` 翻转逻辑锁（锁定后拒绝改名、移除检材与重建快照）；`case_remove` 只删除案件自己的文件
（台账、锁、`evidence/`、`staging/`），目录里的其它内容原样保留，目录空了才删除目录本身；
`case_list` 读应用数据目录下的本地登记簿（`core::casework::case_registry`）并附台账档案；
`case_option_query`/`case_option_save` 读写 `setting` 表。`CaseState` 是台账的读缓存（检材与卷记录）
加上索引不可用时回退用的内存快照体，台账本身是唯一事实源。

**检材。** `image_load` 与案件打开时把已加载镜像登记为检材（以 image id 幂等；来源已是另一个检材时
拒绝加载）。首次登记触发自动识别首任务与 `trigger = auto` 的依赖链根任务。移除检材先取消它的全部
未终结任务，再在一个事务里删除检材行（级联）并记 `evidence.remove`，最后在锁外删除整个检材目录；
Windows 上仍被打开的文件删不掉时短暂重试，残留留给下次打开时的自愈。

**卷发布（`core::casework::case_ledger_volumes`，两段）。** 每个卷的走树在 `staging/` 写出 `.dirz`，发布时移入卷
目录并登记 `volume` 行（`publish_snapshot`）——昂贵的走树结果立刻持久，后续导入失败或案件关闭都
不会丢。随后在 `staging/` 由该快照导入索引文件，再移入卷目录登记（`publish_index`），登记以「索引
来自的快照仍是当前快照」为条件，过期的构建不会覆盖新结果；更旧的快照也不会覆盖更新的
（`Superseded`）。快照内容（源大小、证据指纹、卷号）未变的重建保留旧索引继续服务，直到新索引发布；
内容变了则旧索引立即下线。被替换的文件在发布事务提交后删除，删不掉的留给自愈回收。复用：卷记录与
已加载镜像一致且快照文件在时直接复用；只有索引缺失时由现有快照重新导入，不重走文件系统。索引导入
失败时把快照体载入内存作为浏览回退。宿主 Docker 容器卷（`core::catalog::host_docker_index`）走同一条发布
路径，来源映射缓存在检材的 `cache/docker/`。

**检材库（`core::casework::evidence_db`）。** 四个检材库共用一套连接策略：写者（`open_writer`）必要时建检材
目录与库文件，新库在一个 `BEGIN IMMEDIATE` 事务里一次建全结构并盖上各自的 `application_id`
（facts "LCFX"、analysis "LCAN"、timeline "LCTL"、search "LCSR"）与 `user_version`；读者
（`open_reader`）库不存在时返回 `None`，从不建库；只改已有数据的写者（删除、孤儿修复）用
`open_existing`，忙等上限由调用方给出。版本不符的库拒绝打开，提示移除检材后重新加入以重建。
哈希、类型识别与插件结果只为案件里的检材落库；未在案件中的镜像结果只留在会话里。

**全局搜索。** 每次搜索是检材 `search.db` 里的一行任务（`global_search_history`）加逐条命中
（`global_search_hits`，`core::search::global_search_store`）。任务行在搜索启动时即以 `running` 落库——任务即
历史，删除任务 = 删除整次搜索（含命中）；命中带递增 `hit_seq`，表格经 `global_search_hits_page` 服务端
分页（关键词过滤命中内容/文件名/路径/模式的小写列 LIKE；按文件 `node_id` / 按 `sha256` 窗口函数去重；
`after_seq` 支持增量轮询）。引擎 `core::search::global_search` 从台账读出检材各卷当前的索引文件，按范围（分区/
标签/树节点或卷内路径）解析可扫对象——树节点在所在卷的索引里定位，目录解析成 `full_path_low` 半开
前缀子树——可选已删除文件（`is_delete=1`）、未分配空间（`$未分配簇`/`diskgap:` 虚拟节点直读镜像字节）
与命中源 MD5/SHA256 回填；关键词走 Aho-Corasick（`core::search::keyword::keyword_search`，编码页可选），正则走
`core::search::keyword::keyword_regex`（ASCII/UTF-8 字节直扫，GBK/UTF-16 解码文本匹配后映射回字节偏移——GBK 按解码器
实际消耗的字节数逐字符对齐（`core::search::keyword::gbk_units`：单字节 0x80 的「€」、GB18030 四字节字符与各类错误都
不是两字节）；同一编码的全部正则每块只解码一次，并先用从语法树推出的必含字面量在原始字节上预筛，
不可能命中的块跳过解码）。扫描工作池在 `core::search::global_search_scan`：worker 按原始行序从原子游标领取
单元，命中上下文直接从扫描窗口截取（carry 窗口至少留「前文 + 最长模式」，后文跨块时由下一块补齐），
不再按命中回读文件；可随机读的源（NTFS、ext4 与连续镜像区间）≥128 MiB 时切 64 MiB 段，各段经
`FsSession::stream_file_range` 只读本段窗口，不可随机读的源整段扫描（分段后每段都要从文件头跳读，
总读量随段数平方增长）；每文件上限按文件序保留最靠前的命中，整源与分段扫描选出同一批命中。宿主
Docker 容器文件在范围解析时一次性映射到底层卷上的文件节点，worker 按底层节点读取、命中仍记在容器
节点上；CalcHash 多 worker 并行，回填在单连接上分批提交。单个文件读不出来计入 `read_errors`
（界面显示「N 个对象读取失败已跳过」）并记 warn 日志；没有数据流的文件（`fs_dispatch::MissingDataStream`：
`$Secure` 等只有命名流或索引的 NTFS 元文件，以及镜像摄于系统更新中途、只剩 `$FILE_NAME` 的记录）按空
文件处理，不算读取失败，只记 debug 日志。每个检材最多保留 20 个任务，滚动删除；扫描失败同样落 `failed` 终态。重启或收尾落库失败残留的 `running` 行由任务列表查询改判 `failed`
（启动先登记在跑再落行、列表先读行再取存活集，刚启动的搜索不会被误判）。收尾时任务行已被删除
（删除运行中的搜索只等 10 s）的命中一并清掉。命中与哈希回填失败的批次留在缓冲里随下一批整批重试
（`INSERT OR REPLACE` / 按源 UPDATE，幂等），积压超限或收尾仍写不进去时整次搜索以该错误失败，不
静默丢数据。任务面板运行期间每 2 s 轮询任务列表。

**时间线。** 时间线以物化投影存储在检材的 `timeline.db`（`core::timeline::timeline_store`：`timeline` 表含
`oid`/`tid`/`delete`/`tl_time`/`from_app`/`type`/`tl_flag`/`content`/`extra` 与按应用类型、删除标记、
tid、时间的索引，外加构建元数据单行 `timeline_build`）。投影规则由代码显式定义：`file` datatype 投影
各卷索引里的文件行（time 角色字段 = Mtime，Ctime/Atime 不进时间线，`tl_flag` = 修改，`from_app` =
卷键），`plugin_event` 投影检材 `analysis.db` 的插件时间线事件（`tid` = run_id，`from_app` = 插件 id，
`tl_flag` = event_type），`ntfs_usn` 投影 NTFS 变更日志；缺时间戳的源记录跳过。两条投影路径为：
`timeline_start` 创建重建任务（`TaskKind::TimelineBuild`）全量重投影（先清空，幂等），插件任务收尾
钩子（`task_scheduler::executor`）按 run_id 级联删 + 重投影该 run——写路径自动投影。`timeline_status`
状态机 `buildRequired`/`running`/`stale`/`ready` 以源水位（投影规则版本 + 各已索引卷的「卷键@来源快照
时间」、插件 run `max(completed_at)`）判定 staleness，水位在重建开始前取，重建期间新到的数据让结果判
为过期而不是被悄悄漏掉；查询面 `timeline_count`（strftime 动态时间桶 + 时区参数化 + 桶分页）/
`timeline_datas`（`tl_time desc` 明细分页）/`timeline_types`（`group by from_app,type` 两级维度）/
`timeline_range`/`timeline_export` 都带 `imageId`，共享参数族（start/end/time_zone/time_unit/
from_apps/data_types/pairs/text/show_delete），两级勾选走 `(from_app, type)` 行值 IN 精确过滤。前端
`TimelinePanel.vue`：状态机确认创建/更新、手绘柱状图点击钻取（范围收窄 + 粒度降档，压栈撤销）、虚拟
明细表（序号/时间/类型/内容/时间类型）、双击文件行经 `fs_node_locate` 跳回文件列表、配置弹窗（当前
检材的应用树两级勾选）与 CSV 导出。mactime body-file 导出模块为 `core::timeline::mactime`（命令
`fs_timeline_export`）。

## 元数据索引层（filedb）

统一文件目录分为索引文件格式 `core::catalog::filedb::filedb_file`、构建引擎 `core::catalog::filedb::filedb_store`、查询路径
`core::catalog::filedb::filedb_query` / `core::catalog::filedb::filedb_flat`、寻址模型 `core::filesystem::meta_addr` 与文件系统数组 `core::filesystem::fs_all`。
每个卷一个索引文件（`application_id = 0x4C434958`，"LCIX"），由快照一次建成后发布，之后只读；
`user_version` 是结构版本（`filedb_file::SCHEMA_VERSION`），读者拒绝其它版本的文件。

**表结构。** `filedb` 一行对应一个目录项，`ordinal` 是 rowid：`nid`/`pid` 为节点与父节点 id；
`name`/`name_low` 与 `full_path`/`full_path_low` 成对保存原文与小写副本，大小写不敏感的名称/路径查询
只走小写副本列的索引，从不在读时 `LOWER()`；`kind`（datatype）、`extension`、`size`、`mtime`/`atime`/
`ctime`/`crtime`、`is_delete`（**2 = 正常，1 = 已删除**）、`meta_addr`/`meta_addr_high` 均为独立列，完整
`FsNode` 仍存 `node_json` 以保持 IPC 结构兼容。一个文件只装一个卷，所以没有镜像 id、卷号列，也没有
它们的索引前缀。二级索引包括 `pid`（覆盖 `(pid, name COLLATE natsort, nid)`，目录浏览、自然序排序
与 keyset 游标全部由它服务）、`name_low`、`full_path`、`full_path_low`、`datatype`、`extension`、
`time`、`pid_time`、`is_delete`、`meta_addr`；文件全显另有按起点定位子树的 `nid`、两条排序覆盖索引
`flat_default`（默认序 `kind, name natsort, nid`，尾带 `extension`/`name_low`/`full_path_low` 供全显
过滤在索引内求值）与 `flat_name`（名称序，也服务其它按名称自然序的扫描），以及只收已删除目录、按
`full_path_low` 排序的部分索引 `deleted_dir_path`（已删除目录探测专用）。`natsort` 是 `core::common::natsort` 用
rusqlite `create_collation` 注册的数字感知比较器，构建与读取连接都注册，`file2` 排在 `file10` 之前；
它按 UTF-8 字节零分配比较，建一条自然序索引要调用上千万次，结果必须与已建索引时的字符级定义逐一
一致（测试以随机对拍固定）。`filedb_walked_dirs` 记录已遍历目录，区分"已覆盖但为空"与"未建索引"；
`filedb_child_counts` 是构建时预计算的计数缓存，含 `child_count`、`child_delete_count` 与
`child_count_inexact`（截断快照标记近似）；`filedb_meta` 单行记录卷身份（检材、卷键、卷号、来源
快照时间、证据指纹、条目数、截断、导入时间）。

**寻址模型。** 读取以 `(image, volId, metaAddr, metaAddrHigh)` 四元组寻址（`meta_addr::FileQuery`）。
`fs_all::FsAll` 按分区表顺序枚举文件系统，LVM 容器后紧跟其逻辑卷，无分区表的归档/文件夹证据得到
单个 `root` 条目；`vol_id` 就是数组下标，`part:N` / `lvm:N:M` 卷键与之双向映射，台账的卷记录同时保存
两者。`meta_addr::classify` 把适配器节点 id 归类：NTFS MFT 记录号、ext4/btrfs inode 是元数据地址
（已删除文件的地址仍然有效）；NTFS 命名流用 `meta_addr_high` 携带流名哈希，btrfs 用它携带子卷 id；
未分配簇、盘隙、FAT 系统结构与 `deleted:run:` 走块级 `(offset, length)` 寻址而不经过元数据地址；
FAT/ReFS/APFS 等路径型文件系统 `meta_addr = 0`，靠 `full_path` 判别。`filedb_resolve_meta_addr` 按
`vol_id` 选出卷索引，由 `filedb_query::node_id_for_meta_addr` 以索引为权威把四元组解析回节点 id，再交给
`fs_read*` / `fs_stat` 读内容。

**构建。** `filedb_store::build_index` 把一份 `.dirz` 导入一个必须不存在的新文件：构建连接
`journal_mode = OFF`、`synchronous = OFF`、`locking_mode = EXCLUSIVE`、16 KiB 页；生产者线程解码快照并
计算小写副本与 MetaAddr 列，消费者经有界通道逐批 4096 条插入并流式累计目录覆盖与子项计数
（`DirectoryTally`），全部行写完后一次写入计数缓存与卷身份行，再建全部二级索引，最后关闭并 fsync。
没有回滚日志也不需要：取消或失败时删除半成品（`PartialIndex`），上一次发布的索引原样继续服务。消费者
中途取消或失败时先释放有界通道接收端再 join 解码线程，避免生产者堵在 `send`；SQLite progress handler
同时检查任务的 CancellationToken，使二级索引建立阶段也响应取消。读者以 `file:…?mode=ro&immutable=1`
URI 打开（Windows 长路径前缀与 UNC 路径都转成合法 URI），不再需要共享内存与锁。

**查询。** 目录浏览为 `WHERE pid=?`，`ORDER BY name COLLATE natsort, nid` 加 keyset 游标分页；
`query_files` 支持名称、路径、扩展名与大小等查询：前缀用 `_low` 列的范围比较
（`>= prefix AND < prefix+U+10FFFF`，直接命中索引，`LIKE … ESCAPE` 会让 SQLite 放弃索引优化）、子串用
`LIKE`、扩展名/种类 `IN`、大小区间、`is_delete`，`LIMIT ? OFFSET ?` 分页并按自然序排序；`child_counts`
点查计数缓存并回传 `inexact` 标志；`category_summary` 按 `kind` 聚合。未覆盖目录返回 `None`，命令层回退
live 遍历。前端通过 `src/lib/ipc/filedb.ts` 的 `filedbVolumes` / `filedbQueryFiles` /
`filedbResolveMetaAddr` 调用。

浏览命令的数据源（`commands::filesystem::filesystem_snapshot_index`）：开着案件时是台账里该检材源卷大小与已加载
镜像一致的卷索引，没有案件时是会话索引（无案件时快照导入进程临时目录里的同格式索引文件，随会话快照
释放而删除；完整 body 仅作为索引失败时的回退）。`fs_children_page`、完整 `fs_children`、
`fs_child_counts`、`fs_subtree_empty_dirs` 与 `fs_flat_page` 优先读索引，miss 后再回退到内存快照体或
live listing。`fs_flat_page` 只使用非截断快照的索引，仍会对索引 / 快照未下探的 deleted 目录拼接 live
恢复遍历。删除单卷快照（`snapshot_drop`）删除卷记录与整个卷目录。

**文件全显（deepsearch / allowAllFiles）。** 列表请求与逐级浏览同构，只多
一个布尔——前端叫 `deepsearch`，后端叫 `allowAllFiles`；置位后起点 `parentId` 从「列一层」变成「以该
节点为根递归平铺全部文件（含符号链接，`datatype=file` 口径，不含目录）」。命令 `fs_flat_page`
（`commands/filesystem/filesystem_flat.rs`）接受 `keyword`（命中 `name_low` 或 `full_path_low`）、`sort` / `sortdesc`、
`skip` / `limit`（默认 100，上限 5000）与表达式过滤 `filter`（`core::catalog::flat_query::FlatFileFilter`：
名称通配、相对起点的锚定/子串路径、扩展名、类型标签、链接指向、元数据地址、已删除、大小区间与整除、
三个时间列的显示口径区间、类别扩展名集合），一次返回 `{items, total, truncated}`。`core::catalog::filedb::filedb_flat`
两段式取页：内层只取排序键与主键切出 `LIMIT ? OFFSET ?` 页，外层以 `CROSS JOIN` 按主键回表取
`node_json`——索引文件没有 ANALYZE 统计，普通 JOIN 会被规划成沿索引扫整卷、对每行重跑页子查询，
`CROSS JOIN` 把页子查询固定为只跑一次的外层循环。总数是同一 WHERE 的独立 COUNT，页没取满时直接由
行数推出；关键词、名称通配、路径子串、扩展名与类别条过滤由 `flat_default` 覆盖索引在索引内求值，
取页与 COUNT 都不回表；无过滤的整卷 COUNT 锁定最窄的 `datatype`。带 `full_path_low` 区间（目录起点或
锚定路径过滤）的取页、COUNT 与直方图锁定 `full_path_low` 区间计划，耗时随子树大小——否则规划器会改为
整段扫描 `flat_default`，小目录反而变慢。已删除目录探测（每取一页都跑）锁定部分索引
`deleted_dir_path`：整卷起点扫它，目录起点按前缀区间定位，开销只与范围内的已删除目录数有关；SQL 必须把
已删除标记写成常量才能匹配部分索引。不锁定时规划器会走 `is_delete` 索引，逐个回表整卷的已删除行，已删除
行多的卷上连只有几百个文件的目录也要数百毫秒。探测结果在 Rust 侧排序。这些计划由 `filedb_flat` 的
`EXPLAIN QUERY PLAN` 测试固定：没有统计时计划与数据量无关，种子库就能复现。
起点为整卷时不加范围约束，目录起点经 `nid` 索引取 `full_path` 后用 `full_path_low` 前缀区间圈定子树。
三条数据源（卷索引 → 内存卷快照 → 实时遍历）共用 `flat_query` 的谓词与比较器，SQL WHERE / ORDER
表达式是它们的逐条镜像并由测试对拍；多卷起点（镜像根）与已删除目录的实时拼接行按「每个来源取前
`skip + limit` 行」的窗口经 `FlatPageAssembler` 合并切页。实时遍历与拼接结果按 `(image, parent)` 缓存
在 `AppState::flat_walk_cache`（FIFO 2 条），前端「刷新」传 `refresh` 作废。`fs_flat_ext_counts` 给出
起点子树的扩展名直方图供类别条计数。全显请求不进入后台任务队列（SQL 路径以整卷为起点时在百万行卷上
为亚秒级，以目录为起点时随子树规模增长；实时遍历只在无索引无快照时发生并被缓存）。前端 `DiskFileList` 以 infinite 模型消费：`useVirtualList` 的 `virtualCount`
按服务端总数撑开滚动条，可视区间越过已加载末尾即按 `skip = 已加载行数` 补页；关键词 / 漏斗 / 排序变化
按请求签名去抖后从第一页重载；搜索框输入即自动进入全显、清空即退出，切走面板或切换镜像自动复位。

## 文件系统会话复用

递归、批量或同一 command 内包含多次文件系统操作的路径应优先使用 `core::filesystem::tree::fs_tree::FsSession`，而不是在
循环里反复调用 `children_for_image` / `file_stat` / `read_file_limited` / `stream_file_chunks`。
`FsSession` 在一次请求内按卷缓存 `fs_dispatch::Volume` 句柄，目录导出、ZIP 导出、目录统计、时间线/
快照遍历、媒体物化、`fs_read`、`fs_preview`、`fs_digest` 和多种原类型预览因此可以复用
同一个 Ext4、Btrfs、ReFS、XFS、UFS、SquashFS、exFAT、APFS 等卷实例。Dokan 只读挂载的路径解析
和目录枚举使用 mount 专用 session listing profile，保持不展开 deleted/unallocated/orphan 取证增强节点，
同时让单次深路径解析复用已打开卷。`SnapshotWalker` 目前保留为
`FsSession` 的兼容别名；新增整树遍历、批量 stream 或 stat+read 组合路径应直接命名为 session，避免把
这个机制误认为只服务快照构建。交互式 `fs_children` 仍保持按请求打开，后续若需要跨请求热卷缓存，应先
增加明确的内存与生命周期预算。

## 分页 IPC

大规模目录、Journal 和案件派生数据不应新增全量返回接口。新增分页接口统一使用
`Page<T>` 形态：`items`、`nextCursor`、可选 `total` 和 `truncated`。游标应基于稳定排序键，而不是裸
offset。当前普通目录已提供 `fs_children_page`，案件模式下会优先使用 SQLite 快照索引返回单页，
索引未覆盖时沿用 `fs_children` 的案件快照 / session 快照优先路径；
文件列表通过 `fs_children_page` 的可选 `query` 参数发送元数据过滤和排序，采用目录优先、排序键、节点 ID 的稳定游标；SQLite 索引只取单页，实时来源在后端过滤排序。
未传 `query` 的兼容调用仍按 `(name,id)` 分页。哈希/标签等派生列、路径和隐藏空目录筛选通过有界分页取得完整目录后沿用客户端语义；普通滚动按需补页，定位目标会补齐到目标页。分类条在分页统计不完整时保留所有类别入口并隐藏计数，避免把已加载页误报为整目录。
目录计数只对可见行查询，未索引目录通过 `pageSize:1` 获取总数，不再传输其完整子列表。无快照的实时来源仍可能需要枚举整目录以计数和排序。NTFS USN Journal
提供 `fs_usn_journal_page`，ext4 jbd2 历史 block revisions 提供 `fs_ext4_journal_blocks_page`。旧
`fs_children`、`fs_usn_journal` 和 `fs_ext4_journal` 仅作为兼容入口保留，新的 UI 或
批量调用应优先使用分页接口。案件派生数据虽然仍保留 `fs_digest_load`、`fs_ftype_load`、`fs_tags_load`
全量兼容入口，但新调用方应优先使用 `*_load_nodes` 按当前节点集合水合，避免一次目录切换拉取整镜像
摘要、类型识别或大型标签表；标签编辑应优先使用 `fs_tags_apply_changes` 增量写入，不应为了清除一个
标签而提交不完整的整镜像标签 map。

常见事件：

- `task-lifecycle`
- `task-progress`
- `image-load-progress`
- `image-hash-progress`
- `image-export-progress`
- `bitlocker-unlock-progress`
- `bitlocker-scan-progress`
- `snapshot-build-progress`
- `carve-scan-progress`
- `fs-flat-list-batch` / `fs-flat-list-progress`

事件订阅必须通过 `src/lib/ipc/events.ts` 或领域 wrapper。

## 时间线

- 重建同时投影 filedb 文件修改时间、插件事件，以及已加载镜像各 NTFS 卷的 `$Extend\$UsnJrnl:$J`；USN 以独立类型 `3`（NTFS 变更日志）参与筛选、热力图、分页与导出。日志读取不依赖文件目录索引，因此目录索引未完成也可生成日志时间线。旧源水位升级为 `usn-v1:` 前缀，已有成功结果会提示重建一次。
- `volumes/ntfs_journal.rs` 复用现有 VMDK/NTFS 只读数据源，按数据运行列表跳过稀疏前缀；连接属性走原 NTFS 流式读取路径。固定大小缓冲区处理分块和页间零填充，时间线不再受日志面板的 64 MiB 上限限制。取消在每个数据块检查，已投影批次保留并标为取消；读取失败向任务中心传播。
- `ntfs-core`（Apache-2.0）负责 V2/V3 二进制记录解析，本地适配层验证记录边界、保留 FILETIME 原始精度并拒绝缺失时间。V4 仅含区间信息、没有时间戳，不伪造时间线事件。原始解析结果保存在时间线 `extra` JSON 中，行时间仍为 Unix 秒。
- USN 行使用卷键与 USN 唯一标识历史事件，保留事件文件名、MFT 与父目录编号；不把可能复用的 MFT 编号直接跳转为现存文件，不声称重建了完整历史路径。删除日志是有效事件，默认“排除已删除文件”筛选不会隐藏该事件；该筛选继续作用于 filedb 删除文件行。

- 默认显示当前检材的事件，顶部保留内容搜索、检材/类型筛选、已删除开关与重置；未创建时提供创建入口，新数据通过内联提示更新。
- 年度热力图使用 `timelineCount` 按显示时区、所选年份的完整起止区间聚合（最多 366 个日桶），与明细分页无关；缺失日期补零，强度相对该年单日峰值分为四档。默认年份取当前检材最新事件年份，支持切换年份。
- 热力图遵循检材、类型、内容和删除状态筛选，保留全年背景；点击一天替换当前时间选择，明细及导出限定到当天。图表年份切换仅改变概览，明细的有效时间范围始终显示在列表上方。
- 趋势视图保留年到秒的粒度与逐层返回，钻取区间与已有选择相交，避免切换到粗粒度后意外放大范围；可收起概览以增加列表空间。
- 事件列表以单行显示完整时间、事件类型、内容、来源与定位入口；单击在右侧详情栏展开完整摘要和来源标识，双击文件或点击定位回到文件列表。所有导出沿用已应用的查询条件。

## 文件雕刻

全局搜索、时间线、文件雕刻和分析结果的初始页共用 `WorkspaceStartState.vue`：居中显示图标、标题、简短说明与主按钮，历史记录或刷新放在左上角插槽。加载期间禁用主按钮；时间线创建前隐藏筛选、空表格和计数，读取失败可在初始页重试，开始构建后进入工作区。

全局搜索和文件雕刻通过 `analysis-workspace.css` 共用记录侧栏、工具栏、筛选条、状态条、表格与新建弹窗样式；选中记录的详情由 `DetailDock` 从右侧展开（正文使用共享 `FileDetailLayout` 模板，与时间线事件详情共用）。初次读取已有记录或出现新记录时自动展开侧栏，普通进度刷新保留用户手动收起状态。雕刻页通过 `carve_scan_list` 列出当前检材在本进程中保留的会话（沿用最近三个会话的回收策略，活动扫描不会回收），切换记录时隔离迟到的结果请求；`carve_scan_remove` 仅删除当前检材已结束扫描的内存记录，不删除已恢复文件，也不清除任务中心的执行历史。重启应用后不会保留这些内存记录。

“新建扫描”弹窗先选择按类别分组的文件类型，扫描范围与高级上限默认折叠；Esc / 取消保留原有结果。运行时显示进度、暂停/继续和取消，结果沿用文件列表表格样式。筛选仅作用于已加载结果，底部显示已加载与总数；未勾选时“恢复全部”恢复整个扫描会话，勾选后按钮变为“恢复所选”，行内及右侧详情可恢复单个文件。

`core::carving::carver` 以 4 MiB 为读取窗口、以 512 字节为扫描块，仅在块首检查注册格式的
签名。签名命中后由 `carver_format_*` 模块中的对应结构解析器计算文件长度；解析失败的候选只增加签名
计数，不进入结果集。当前注册顺序为 MP3、WAV、AU、WMA、JPEG、PNG、BMP、GIF、PCX、
TIFF、ZIP、RAR、PDF、SQLite。扫描器按签名首字节索引候选格式。
识别出的文件只在本窗口内跳过其占用的块，下一个窗口总是从窗口边界重新扫描：头部声明的长度（BMP、
WAV、SQLite 等）即使有误，也最多让扫描漏看当前窗口剩余部分，不会跳过整段证据。

窗口彼此独立，`core::carving::carver_scan` 因此把窗口按顺序分给工作线程（默认最多 8 个），在调用线程按窗口
顺序合并：序号、进度、签名计数与结果上限的截断位置都与单线程扫描逐字节一致。工作线程最多领先合并
点几十个窗口，观察者阻塞（暂停）时随之停下；某窗口读取失败要等合并到该窗口才上报，之前窗口的结果照常
输出。EWF / VMDK / VHD(X) / PMFX 的解码器对每次读取加锁，工作线程经
`DiskImage::concurrent_reader` 各自重开一份句柄。

解析器通过 `carver_reader::CarveReader` 读取：候选位置到当前窗口末尾的字节直接取自扫描缓冲区；跨出
窗口后由每个工作线程复用的预读缓冲区供数，顺序读取时预读量从 64 KiB 倍增到 1 MiB，跳到别处（ZIP
成员头、PDF 交叉引用偏移）则从 4 KiB 重新开始。标记搜索用 `memchr::memmem`，PNG 校验用
`flate2::Crc`，JPEG 熵编码数据按 `0xFF` 批量跳过。结构解析的关键行为如下：

- PDF：结束标记须是前面紧跟 `startxref <偏移>` 的 `%%EOF`（跳过流或注释中的裸 `%%EOF`，无
  `startxref` 的畸形文件退回第一个标记）；大小为最后一个结束标记加其换行的绝对位置。
  只有标记后（跳过补白与注释行）紧接对象头或 `xref` 时才继续寻找增量更新或
  线性化文件主体，且新标记的 `startxref` 必须指向本文件内的交叉引用段，不再盲目向后搜索 16 MiB 并把
  相邻文件并进来。
- ZIP：从第一个中央目录记录逐条走到结束记录，支持 ZIP64 结束记录与定位符、APK 签名块和
  `PK00` 分卷标记。数据描述符优先按记录的
  压缩长度校验（支持 ZIP64 与无签名描述符），搜索止于数据之后的第一个记录，不会越过成员扫到单文件
  上限。中央目录条目多于已走过的本地条目时，说明起点是更大归档内部的某个成员，不作为结果。按成员识别
  DOCX / XLSX / PPTX、ODT / ODS / ODP / ODG、EPUB、APK、JAR、WHL。
- TIFF：长度为各 IFD、离线标签值、条带 / 瓦片和内嵌 JPEG 所及的最远字节，跟随 IFD 链与
  EXIF / GPS / 子 IFD。IFD 表必须完整且字段类型合法；越过区域末尾或
  超过图像尺寸所能容纳的引用（损坏或恶意文件）被忽略而不是采信。
- WAV：支持 RIFF / RIFX / RF64 与 16 字节以上的 `fmt` 块；`data` 之后的块能走到容器声明的长度时取
  容器长度（含奇数长度的填充字节与 LIST 等尾随块）。
- 读取计数精确到当前位置，回退读取位置时同步减少计数，避免 RAR 和跨块 ZIP 结束记录的长度误差。
- 任何格式报告的长度都不超过区域末尾与单文件上限。

扫描运行在统一后台任务和顺序 I/O 资源通道中。`CarveScanSession` 保存签名数、有效结果、稳定序号与
暂停状态；前端通过 `carve-scan-progress` 增量同步，以 `sequence` 游标分页。恢复按
`f{绝对块号}.{扩展名}` 命名，流式计算 SHA-256 并输出 DFXML。代码版权与许可证见
[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。

`probe_carve` 示例可导出全部结果（`--dump`）并指定线程数（`--threads`）；镜像带有完好的 ext4 文件系统
时，`probe_carve_truth` 列出每个文件的首个物理偏移与大小，二者按偏移对照即可统计各格式的召回率与长度
准确率。

## SQLite 预览与删除记录恢复

SQLite 原类型预览的 schema/sample 与分页请求均通过 `core::preview::sqlite::sqlite_preview_budget` 限制 SQL
执行：每次请求共享 5 秒 / 5,000 万条虚拟机指令预算，每 1,000 条指令检查一次。超限中断并
明确报错，计数失败不会伪装为空表。请求以 `Arc<SqliteSession>` 保持 VFS 文件有效，SQL 执行
移出 `AppState::sqlite_session` 缓存锁；切换数据库或卸载镜像释放缓存引用后，最后一个读取者
结束时清理对应 VFS 文件。

预览把库导入进程内存 VFS（`core/preview/sqlite/sqlite_vfs.rs`）。该 VFS 没有共享内存方法，连接以排他锁模式
打开，wal-index 放在堆内存（VFS 的锁是空操作，预览连接之间不会互相阻塞），并关闭「关闭时检查点」。
同目录的 `-wal` 一并导入，由 SQLite 自己校验并回放已提交的帧，加密库的 WAL 同样适用；表头为 WAL
模式而没有 `-wal` 的库也能打开。`-shm` 用不到；`-journal` 不导入，因为热日志会让只读打开失败，
删除记录视图会单独读取它。sidecar 在前端传入的父目录节点下查找，所以 `ntfs:`、`ext4:`、
`folder:` 这类不含路径的节点 ID 也能找到。



`core/preview/sqlite/sqlite_recovery.rs` 接入 vendored `sqlite-core`（`src-tauri/vendor/sqlite-core`，来自
SecurityRonin/sqlite-forensic），为 SQLite 浏览器的「已删除记录」提供 `fs_sqlite_deleted_records`。
与表格预览不同，它不经过 SQLite，而是直接解析库文件的字节：

- 雕刻空闲链表页、页内空闲区、空闲块残留（单元头被覆盖时按列结构重建，rowid 随之丢失）与溢出链。
- 同目录有 `-wal` 时叠加已提交的帧，并雕刻未检查点的帧；没有 `-wal` 而有 `-journal` 时，用回滚
  日志里的事务前页面与当前库比对，得到最后一个事务删除的行和修改前的行。
- 从空闲区恢复被删除或替换的 `sqlite_master` 行，即已删除的表结构。

每条记录带来源、页与偏移、rowid、置信度和表归属：页仍属于某张表的 B 树为「确定」；在空闲页上、
行结构只与一张现存表吻合为「推断」；与多张吻合为「多表匹配」；其余为未归属。现存的行不会出现
在结果里。加密库用浏览器解锁时的同一密钥按页重新解密（自动识别 SQLCipher 1–4 的默认参数，也可
使用表单里的兼容参数），因为逻辑导出会丢掉存放证据的空闲页；加密库的 `-wal` / `-journal` 暂不
参与，结果里附说明。库超过 512 MiB 时不扫描；结果最多返回 20,000 条，其余计入 `omitted`。
前端 `SqliteDeletedRecords.vue` 按表和来源筛选并本地分页，选中现存表时按其列名显示；回滚日志中
修改前的版本会标出与当前值不同的单元格。

## 命令行与桌面只读挂载

完整无窗口命令行入口为 `src-tauri/src/bin/lovelycarve-cli.rs`，交互用法与脚本协议见
[`CLI.md`](CLI.md)。`cli/` 管理交互目录上下文、参数解析、JSONL 结果引用和退出清理。
`build/cli_catalog.rs` 在构建时读取本文件所述的 `lib.rs` Tauri 注册列表和真实函数签名，
生成全部命令的直接 Rust 调用与参数元数据；不经 WebView、HTTP 服务或运行中的桌面程序。
CLI 复用 `application_builder()` 的状态/插件初始化，但清空窗口配置。唯一依赖 WebView
参数的媒体租约命令通过显式 CLI owner 适配，保持 GUI 的窗口存活检查。可执行文件旁的
`plugins/` 同样参与插件发现，`lc-plugin-host` 也从可执行文件旁查找。`just build-cli-full` 构建完整功能，`just build-cli` 与 `just run-cli` 是无 QR/OCR 的
开发入口；`just test-cli` 验证真正的无窗口进程与脚本工作流。

桌面只读挂载仍由 `app/workflows/mount.rs` 编排，`commands/system/mount.rs` 提供 IPC，
`core/platform/dokan_mount/` 负责 Windows Dokan 投影。挂载使用 evidence session，
范围读取复用文件系统适配器；共享、句柄和挂载点在卸载证据及关闭应用时清理。
CLI 可通过同一命令目录调用相关 `fs_mount_*` 命令，保持无窗口会话的退出清理。

## 界面与交互

本章记录主窗口各页面的交互约定与组件归属；视觉与交互规范见 [前端规范](FRONTEND.md)，状态归属与资源收尾见 [前端模块与生命周期](FRONTEND_STRUCTURE.md)。

### 文件列表与可拆分预览

- 文件列表默认展示名称、类型、大小、修改时间；删除状态直接标记在名称旁。完整路径、哈希及其他取证字段通过「显示列」开启，已有列偏好保留。
- 顶部仅保留路径与单行工具栏；分类筛选通过「分类」按需展开，收起时按钮保留已选类别数量。选中/勾选后的导出操作位于列表底部，快照与结果完整性提示放在底部状态栏。
- 右侧预览默认关闭，单击文件或点击「右侧预览」按钮时打开；目录树可收起并保留展开状态。搜索范围为当前目录及子目录，勾选后可直接批量导出文件或目录；其他取证操作保留在右键菜单。
- 选中目录后主动打开「右侧预览」会显示该目录并允许拆分；预览已经打开或拆分时，点击目录行同步目录属性。预览关闭时，单击目录仍只选中，不自动展开预览。
- 打开右侧预览时主窗口保持当前大小，预览在窗口内分配宽度（见「右侧详情栏与固定窗口大小」）。预览左边缘可拖动调整宽度（支持方向键）并记住宽度，拖动只在窗口内与列表分配宽度。窄窗口以右侧浮层展示，关闭即可继续浏览。
- 拖动预览标题栏超过 48px 后松开，或点击拆分按钮，打开独立 Tauri 窗口；只有创建成功才收起停靠栏。独立窗口接管该列表的预览，随后点击文件、从目录树打开文件或从搜索结果定位时都在同一窗口切换，主界面不再展开停靠栏；切换检材后明确打开的新文件也会同步。工具栏按钮在拆分后用于聚焦独立窗口。
- `lib/ipc/previewDock.ts` 的选择事件携带 owner / preview 标识、递增 revision，以及文件节点、检材、完整路径、识别结果、哈希和字节定位目标。窗口先订阅再请求最新选择，避免开窗期间的切换丢失；接收端忽略迟到的旧 revision。不同 owner / preview 的窗口互不跟随。
- 独立窗口「停靠到右侧」通过 `lib/ipc/previewDock.ts` 请求原列表，携带 owner / preview / request 标识。原列表保存最新同步的节点与完整路径，确认当前检材仍匹配后恢复预览并切回文件面板；窗口收到确认后才关闭。原列表关闭、切换检材但尚未选择新文件或通信超时时保留独立窗口并提示原因。直接关闭独立窗口会结束拆分模式，主预览保持关闭，下一次打开文件恢复内嵌预览。
- `usePreviewDock` 管理窗口归属、拆分状态和订阅生命周期；识别类型、哈希与主题继续使用原有跨窗口同步。拆分与收回会重新挂载内容，视图标签、滚动位置和搜索条件不随窗口迁移。

### 右侧详情栏与固定窗口大小

文件列表预览、全局搜索命中、时间线事件、文件雕刻结果与分析结果记录的详情都在右侧展开，由 `<UiTransition kind="dock">` 包裹。搜索、时间线、雕刻、分析与 BCP 记录共用 `DetailDock.vue`（标题栏、可拖动宽度，共用 `lovelycarve-detail-width`）。文件列表的详细信息页（含独立窗口）及上述五类详情统一使用 `FileDetailLayout.vue`（完整名称、状态、操作区）、`FileDetailSection.vue`（分组与可选折叠）和 `FileDetailFields.vue`（标签/值、长内容换行、复制反馈）。常用信息默认展开，技术标识默认折叠；可用正文宽度达到 640px 时分组可并列，哈希、上下文等宽分组仍占整行。`analysis-detail.css` 仅承载上下文与复核提示样式。

- 主窗口大小和位置由用户控制。切换页面（含 KeepAlive 激活/停用）、打开/关闭详情、卸载详情组件都不调用原生窗口缩放或移动接口。
- 详情在当前窗口内以横向过渡展开/收起；足够宽时与表格并列，窄区域以右侧浮层展示。拖动详情分隔条只调整面板宽度并记住选择。
- 首次挂载或切回缓存页面时保留详情的选中记录与面板宽度。各页面详情开合状态不同，也不改变主窗口尺寸；动画关闭时立即完成布局切换。
- `lib/ipc/windows.ts` 只负责独立预览窗口的创建与主窗口聚焦；详情生命周期不管理主窗口几何尺寸。

### 界面动画

标题栏的「设置中心」默认进入「界面」页，另有「插件」「日志」「关于」；界面页集中提供浅色／深色／樱花主题、动画与挂载偏好。
「设置 → 界面 → 界面动画」提供跟随系统（默认）／开启／关闭三档，
均通过 `useUiPreferences` 的 `motionPreference` 持久化（沿用 `lovelycarve-ui-animations` 键，旧版布尔值
`false` 读作关闭、`true` 读作跟随系统）。快捷开关按最少强制原则切换：打开时只有在系统要求减少动态效果时才写入「开启」。
`main.ts` 在所有窗口挂载前初始化 `useUiMotion`，把偏好与系统 `prefers-reduced-motion` 合并为根元素
`data-motion="full" | "reduced"`，并监听系统变化和同源窗口的 storage 事件。`data-motion` 是唯一开关：
CSS 不再直接查询 `prefers-reduced-motion`，因此「开启」可以覆盖系统设置。

- 关闭（`reduced`）：立即结束正在执行的交互动画并恢复最终布局，`base.css` 把其余过渡和动画统一缩短为瞬时；
  `.spin` 加载反馈继续以半速旋转，不定进度条改为静态满条。
- 开启（`full`）：`motion.css` 中的装饰性动效全部挂在 `:root[data-motion='full']` 下，关闭时根本不运行。
  关键帧只声明起始状态、使用独立的 `translate` / `scale` / `rotate` 属性和 `backwards` 填充，
  与组件自身的 `transform` 叠加且结束后不残留。时长与曲线来自 `--motion-fast|normal|slow` 与
  `--ease-out|in|spring` token（入场用较长时长和减速曲线，退场用较短时长和加速曲线）。

动效分布：弹窗背景淡入、对话框自下方弹性放大，关闭更快；`ui-menu` 菜单与气泡由锚点弹出；
通知从右侧弹入，关闭时固定原位淡出滑走，其余通知平滑归位；工作区与任务面板标签切换对已有容器做上浮淡入，
不重新创建 KeepAlive 面板，文件预览随选中项切换只做轻微透明度过渡；按钮按下轻缩，选项卡片悬停上浮；
空态图标弹出、提示条下滑；欢迎页依次入场并保留插图的轻微循环动画；镜像信息卡片依次入场、磁盘布局逐段生长；
时间线柱状图从左到右生长；子类栏展开与收起使用横向宽度过渡，标签页入场、任务数变化时跳动。
`ui-stagger` 只用于短小、非虚拟化的列表与卡片网格，不逐行动画文件列表、搜索结果等虚拟列表。
切换主题时，`setTheme(theme, from)` 在 view transition 中把新主题从所点控件处圆形展开（不支持时直接切换）。

`UiTransition` 负责条件面板的纵向展开、横向展开、淡入淡出与气泡弹出（`pop`），覆盖搜索筛选、目录栏、结果详情和时间线图表等区域。
`v-disclosure-motion` 统一处理原生 details 的展开/收起，保留 summary 键盘操作与嵌套折叠；`v-content-motion` 默认做轻量淡入，
`.rise` 修饰符用于页面与标签切换，均保留已挂载组件状态。
两者使用 `animateUi` 管理 Web Animations，按 token 取时长与曲线，连续切换、组件卸载或禁用动画时清理临时尺寸和溢出样式。
绝对定位的 details 菜单使用淡入而不裁切其内容。

### 全局搜索界面

- 搜索记录默认收起，点击工具栏「搜索记录」展开；列表保留运行状态、命中数量和时间，窄窗口以侧面浮层显示。无记录时从空状态直接新建搜索。
- 新建搜索默认只展开关键词输入；范围（分区、标签、目录）与高级选项（正则、编码、删除文件、未分配空间、哈希、大小写及命中上限）通过折叠区配置，收起时仍显示当前范围和关键选项摘要。
- 结果表采用文件、路径/来源、命中内容和定位四列，移除每行重复的关键词、编码与普通文件来源标签；未分配空间等特殊来源仍就地标注。单击查看底部完整上下文、模式、来源、偏移、编码、大小与可用哈希，双击或点击「定位」进入文件列表。
- 内容筛选与去重设置默认收起，点击「筛选」展开；已应用条件在收起后仍显示摘要。应用按钮与 Enter 使用相同的服务端筛选逻辑，清除按钮恢复未过滤结果（保留去重方式）。导出继续沿用已应用的筛选与去重条件；运行状态下仍显示进度、取消入口和不完整/读取失败提示。

### 案件管理窗口

`CaseManagerModal.vue` 使用紧凑单列布局，顶部显示案件名称和编号，中间优先展示检材；
`CaseEvidenceRow.vue` 统一普通镜像和 RAID 检材行，直接提供打开操作，路径、添加时间和移除操作按需展开。
RAID 成员、当前检材卷快照和案件详情默认折叠，切换与锁定案件固定在底部。
添加检材复用主壳已有的添加流程，分析统一从右上角「任务中心」面板进入；案件锁定时禁用添加、移除和快照构建，保留只读打开。

### 分析结果目录与记录表

`AppShell.vue` 将 `AnalysisResultsNav.vue` 放入 `SidebarNav` 的中间栏插槽；显示分析大类时替代静态子页面菜单，包括收起状态下的悬停预览。
`SidebarNav` 使用左侧大类栏与中间子类栏，选中大类由主壳的当前页面推导；镜像、工具、分析分别组织页面，BCP 按检材额外追加。
主窗口没有全局底部状态栏；主题切换集中在设置中心的「界面」，当前会话日志在「日志」中搜索、筛选、复制和清空，软件版本与本地构建时间位于「关于」。通用版本读取和控制台捕获由 `src/composables/useAppVersion.ts` 与 `useConsoleCapture.ts` 提供；日志在启动时捕获，即使设置未打开也持续收集，最多保留 5,000 条。
分析中间栏直接显示插件执行结果，不重复大类标题或「分析结果」菜单；目录保留自身搜索框，大类栏底部只有子类栏展开 / 收起，不提供过滤或添加证据入口。收起子类栏后点击分析大类或展开按钮即可重新展开。目录仅显示当前检材，省略重复的检材与分类层级，
按分析功能分组直接列出结果项及记录数，并支持搜索结果项。
悬停或聚焦大类会显示对应子类，展开时在原中间栏预览，收起时浮出；离开导航或按 Esc 恢复当前页面的大类，并关闭收起状态的浮层。
悬停不切换当前页面，点击子页面后通过主壳导航并结束预览。
结果目录独立初始化当前检材与结果树，因此从其他页面悬停分析大类也能查看插件结果，点击结果再激活分析页。
`AnalysisResultsPanel.vue` 使用主工作区显示表格；默认选中当前检材首个有数据的结果项，搜索与 CSV 导出共用同一过滤条件。
点击记录在当前窗口内打开右侧详情，保持窗口大小与位置；区域较窄时，详情浮在表格右侧；字段显示完整值，技术标识折叠。
两者共享 `useAnalysisResults`，使用请求序号隔离快速切换产生的过期响应；案件切换清空旧状态，记录加载失败支持原地重试。
后台完成订阅由独立 effect scope 持有，不依赖目录组件的挂载周期。

### 搜索与时间线回源定位

结果查询同时返回宿主持久化的 `imageId` 与 `sourceNodeId`（文件级任务输入节点；检材级任务为空），插件未声明来源属性时仍可回到任务输入文件。分页与详情采用同一查询口径，检材归属以运行记录为准。

分析结果行内「定位源文件」与详情源文件列表同样通过 `useResultNavigation` 激活记录所属检材。`source.root` + `source.path`（多来源使用 `source.paths`）优先于 `source.node`，兼容旧 Linuxinfo 的 `linux.root` / 路径型 `linux.source`。`fs_source_locate` 在镜像目录中按组件解析并返回导航帧，无需 filedb；严格保留卷和名称大小写，支持分区、LVM 和 Docker 容器视图，跟随中间目录链接，保留末级链接作为定位目标。越过卷根、循环链接、缺失卷和文件明确报错；缺少来源地址的记录禁用入口。源文件定位不强制打开字节偏移，详情提供多个源文件的独立入口。

全局搜索发出包含检材 ID、节点 ID、命中偏移和长度的 `FileLocationTarget`；时间线文件事件使用记录自身的 `eid` 和 `oid`，不强制打开偏移 0。`useResultNavigation` 先激活来源检材，再等待相同检材的文件列表挂载；案件、检材或页面改变后放弃在途跳转，避免定位到另一份检材。
文件列表通过 `fs_node_locate` 解析祖先目录：普通节点从 `filedb` 读取父链和目标卷标识，分区/根节点不要求存在于 `filedb`，到达该边界后合成导航帧；目标行缺失时明确报错。随后退出全显并清除原筛选，等待目录装载完成后选中、滚动并打开目标。源文件缺失或索引过期时显示可操作的失败提示。插件时间线事件仍展开详情，不把插件事件 ID 当作文件节点 ID。
搜索定位使用 `FileRangePreview`，通过现有 `fs_read_range` 每次读取至多 4 KiB，显示绝对偏移并高亮命中字节；可前后分段浏览并回到命中位置，不受普通预览的 1 MiB 上限限制。独立预览及停靠返回保留原命中位置。越界、短读和过期响应不会被显示为定位成功。

### 右上角任务中心面板

主窗口 `AppShell.vue` 通过 `Titlebar` 的插槽提供「任务中心」入口，活动任务数持续显示在入口上；面板本身是 `TaskHub.vue`。
面板固定在右上角，「任务」与「快速分析」以页签切换（左右方向键可切换），始终跟随顶部当前检材标签，检材名称以只读标签显示在页签行右侧。
页内不提供检材选择器或“全部检材”选项；没有当前检材时显示打开提示，不自动回退到案件第一份检材。
切换顶部标签时任务列表、通知、活动数量和快速分析目标一起更新，其他检材已启动的后台任务继续运行。
任务中心不占用侧边栏或主工作区，案件管理的检材行也不重复放置分析入口。
面板收起时保留组件和选择状态，后台任务继续运行；切换案件时卸载面板并重置目标。

任务页（`TaskCenterPanel.vue`）按状态分为「进行中」（运行、排队、待派发、暂停）、「需要处理」（失败、离线）和「已结束」（成功、取消）三张卡片，沿用镜像信息页的卡片样式：`ui-card` 标题行为主色图标、标题与 `ui-count` 计数，右侧按钮可折叠该段；卡片内每个任务一行，行间细分隔线，状态由所在分组与行内状态徽标表达。「已结束」超过 6 条时其余折叠为「显示其余 N 项」，搜索时显示全部匹配；调度器最近通知收在末尾一张默认折叠的卡片中。分组、排序与状态色（`TASK_STATUS_TONE`）在 `lib/taskCenterView.ts`。
任务行第一行是名称和状态徽标（`ui-badge` 按状态色着色，运行中显示进度百分比，成功不显示徽标），第二行是摘要和已用时间（插件提供时加剩余时间）、创建时间或开始时间；只有运行和暂停显示进度条，不定进度显示「处理中」；失败任务的摘要显示错误。
每行右侧只放一个带文字的直接操作（`ui-btn sm`）：运行中暂停、已暂停或离线继续、排队或运行中取消、失败或取消后重新分析；成功后改为结果跳转：插件任务在产生分析结果后「查看分析结果」（打开分析结果页并定位该功能的结果，见 `focusAnalysisResult`），全局搜索、时间线、文件雕刻打开各自页面，Docker、文件系统加载与索引建立打开文件列表。内置任务在详情中始终提供前往所在页面的入口。
点击任务行展开详情：完整错误（`ui-alert`）、对象或文件、开始（未开始则创建）时间、耗时、未完成时保留的进度、分析产物（字段行与采集记录同样式），以及靠右的取消、重新分析、前往结果和靠左的删除；技术标识再单独折叠。成功不显示进度，失败和取消保留最后进度，不伪装成已完成。重新分析和删除继续使用原确认流程。
内部任务默认隐藏，有内部任务时工具栏显示「内部 N」开关；搜索匹配当前检材的任务名与文件名。
调度器以 UTC 写入不带时区的 `YYYY-MM-DD HH:MM:SS`，内置任务使用 ISO 时间；`parseTaskTime` 统一按 UTC 解析，列表时间按本地时区显示（当天只显示时分），插件任务与内置任务按同一时钟排序。
快速分析（`QuickAnalysisPanel.vue`）按插件勾选：同一插件的全部检材级功能合为一个格子，勾选即运行它们（不提供子项勾选）；文件级功能需要目标文件，只能从文件列表右键提交，这里不列出。工具栏放平台选择（单一平台时只读显示）与搜索（插件名、插件 id 或任一功能名），其下一行是覆盖全部列出插件（搜索时为匹配项）的三态「全选」框与「全部展开 / 收起」。插件按收藏、类型（基本信息、系统记录、浏览器、应用程序、日志、数据库、文件分析……）、耗时任务分类，分类标题行带自己的三态全选框、已选 / 总数并可折叠，滚动时停在顶部；插件以格子平铺，面板宽 440px 时每行两个，选中格子为浅底加主题色文字，收藏星标悬停时出现。格子右侧的图标是该插件在当前检材上最近任务的汇总状态（执行中、排队、暂停、离线、失败、取消、完成），悬停提示列出插件包含的功能和各自最近任务；任一功能的任务尚未结束时整块不能勾选，已勾选的插件因其他入口开始任务时自动取消勾选。底栏显示已选插件数和将清理旧结果后重新分析的插件数，「更多操作」中可重新分析全部任务；提交成功后自动切到任务页。
提交捕获发起时的检材 ID，批量部分失败时保留未完整提交的插件；切换检材会卸载旧组件并停止继续发起批量操作，已经启动的任务仍在原检材下执行。
插件状态与调度仍由 `useTaskCenter` 和现有 `sched_*` IPC / `sched-task-event` 驱动；`useUnifiedTaskCenter` 将插件与内置任务汇总给任务列表及标题栏计数，快速分析保持使用插件调度接口。

`useBuiltinTasks` 在全局搜索、时间线重建、文件雕刻扫描和文件列表 Docker 容器合并的调用入口登记任务，通过原有搜索/时间线进度事件更新进度，以命令真实返回结果判定成功、取消或失败。跟踪独立于发起页面，关闭面板或切换页面不会丢失完成状态；切换案件清空会话状态并忽略旧案件异步结果。任务标识带来源前缀，避免与插件数字 ID 冲突。

LUKS / VeraCrypt 解锁由 `useEncryptedVolumeUnlocks` 持有会话任务，提交后立即关闭凭据弹窗。
任务中心显示「加密卷解锁」、源分区、实际排队/运行状态与不定进度，使用原生任务 ID 取消，
以解锁命令真实结果判定成功、取消或失败。桌面使用 `sourcePartition` 目标，`EvidenceSession`
持有按原分区 ID 绑定的原生解密文件系统视图；源 `DiskImage` 的字节、几何、哈希与 RAW 导出不变。
目录、文件、范围读取、挂载和快照按节点分区路由到解密视图，节点 ID 与索引仍归属原检材。
成功后后台自动补建该分区的案件索引或无案件会话索引。主窗口仅更新原镜像元数据，
不打开新标签；停用的标签恢复时使用最新元数据。卸载原镜像撤销解密读取；切案后忽略迟到的 UI 结果。
从分区行进入时不提供分区选择，凭据弹窗不显示说明文字。密码只传入 IPC，不进入任务记录。
任务列表也接受当前已加载的会话镜像，未打开案件或镜像没有案件台账条目时仍可查看、取消后台任务；快速分析仍要求选择案件检材。

雕刻扫描启动命令仅返回会话 ID，不能据此标记成功；通过 `carve-scan-progress` 与状态轮询同步排队、运行、暂停、成功、取消和失败，保存最后扫描进度与发现文件数。任务中心暂停/继续调用 `carve_scan_pause` / `carve_scan_resume`，取消使用快照中的真实 `task_id`。同一进程内通过 `carve_latest_scan` 恢复当前扫描，已知活动会话单独轮询；雕刻会话仅存于后端内存，不承诺重启后恢复，切换案件后忽略旧会话返回。
搜索和时间线通过 `task_cancel` 取消，取消请求期间保留运行状态直到命令结束；不提供原生引擎尚不支持的暂停或重启按钮。Docker 按检材和容器去重，首次展开、进入容器或路径查询触发合并及索引落盘；目录树和列表并发读取等待同一次准备，进度未知时使用不定进度，完成后显示已保存的条目数，跨重启的缓存读取也不新增任务。
文件系统目录快照通过 `snapshot-build-progress` 接入任务中心，按检材与卷拆成“文件系统加载”和“索引建立”两个展示阶段，共享实际后台任务的取消 ID。`task-lifecycle` 与 `task_list` 补齐排队、已有活动任务及异常退出；原生任务从注册表移除本身不能视为成功，必须收到领域完成事件。`started/walking` 表示读取目录，`indexing` 表示目录读取完成并开始索引，`done` 才表示索引安装成功；错误与取消不会被迟到的普通进度或完成事件覆盖，新的排队/启动才重置记录。任务中心收起或离开文件列表不影响跟踪，普通文件夹展开不单独制造任务。每个展示阶段使用不定进度与实际条目数，不估算未知总量；执行记录在当前案件会话内保留。
索引安装收尾集中在 `app::workflows::snapshot::snapshot_install`：成功安装、索引错误和取消各自发出真实终态，目录快照内存回退不代表索引成功。案件自动构建、缓存索引复用/重建和无案件会话都发出明确的索引阶段；缓存复用说明会显示在结果摘要。
自动识别平台以 `autoDetect` 后台任务接入（`lib/autodetectTasks.ts`），每份检材一条记录，保留最近一次识别：`task-lifecycle` 与 `task_list` 打开记录，`autodetect-finished` / `autodetect-failed` 才判定成功、失败或取消；登记释放而未收到结论时记为离线，迟到的结论仍可覆盖。新的排队登记总是一次新运行；与当前记录共用登记的第二次调用（首任务进行中又从镜像信息页发起识别）不会重开已结束的记录。检材加入案件时的首任务标为自动触发，镜像信息页的识别标为手动。成功摘要为平台与检材类型，结果入口打开镜像信息页；运行中可取消，取消不算失败。镜像信息页把进行中的识别视为识别中，不再重复发起。执行记录在当前案件会话内保留。
重新打开案件时复用现有元数据恢复每份检材最近 100 次搜索和最近一次时间线构建；时间线重建更新该检材的同一任务记录，Docker 合并记录仅在当前案件会话内保留。搜索记录删除和重新执行仍从全局搜索页操作，时间线重新构建仍从时间线页操作。

### 前端职责入口

业务面板按生命周期、查询、操作和显示拆分，后续新增代码优先进入已有职责 owner：

| 文件 | 建议方向 |
| --- | --- |
| `src/features/files/components/DiskFileList.vue` | 列表装配与公开定位契约；查询、筛选、选择、键盘、菜单、预览、导出、派生写入分属 composable，工具栏/表格/底栏/浮层独立 |
| `src/features/preview/components/DiskFileDetail.vue` | 预览装配与按需 viewer；前缀/解码/识别、媒体租约、搜索、hex、拖拽各自持有资源 |
| `src/features/search/components/SearchResultsPanel.vue` | 配置、历史/进度、分页、执行和导出分离，36px 结果表由子组件拥有 |
| `src/features/timeline/components/TimelinePanel.vue` | 构建、聚合、筛选、记录分页和导出分离 |
| `src/features/carving/components/CarvePanel.vue` | 配置、历史/任务、结果游标、预览与恢复分离 |
| `src/features/files/components/DiskFileTree.vue` / `src/features/preview/components/viewers/ZipViewer.vue` | 树加载/元数据/纯投影、归档/成员读取/虚拟树各自拥有实例生命周期 |

拆分时保持现有行为和 IPC 契约不变，先加边界和测试，再移动逻辑。

## 功能与 IPC 对照

下表按工作流列出前端入口与后端命令域；完整注册表以 `src-tauri/src/lib.rs` 为准，组件只调用 `src/lib/ipc/` 的类型封装。

| 功能 | 前端入口 | 主要命令 / 事件 |
| --- | --- | --- |
| 案件与证据 | 主壳、案件管理、文件会话 | `case_*`、`image_load`、`image_unload`、`physical_disks_list`、`image-load-progress` |
| 镜像信息 | 分区、校验、识别与 RAID | `image_info`、`image_bad_regions`、`os_detect`、`autodetect_evidence`、`raid_*` |
| 文件浏览与预览 | 文件列表、详情、独立预览窗 | `fs_children*`、`fs_flat_page`、`fs_read*`、`fs_preview*`、原类型预览命令 |
| 索引与快照 | 卷快照、全显和文件查询 | `snapshot_*`、`filedb_*`、`fs_child_counts` |
| 校验与导出 | 哈希、导出任务 | `image_hash_compute`、`image_export_raw`、`fs_digest`、`fs_export*` |
| 数据浏览 | 数据库、日志、命名流和 BCP | `fs_sqlite*`、`fs_realm*`、`fs_*journal*`、`fs_stream*`、`bcp_*` |
| 全局搜索 | `SearchResultsPanel.vue` | `global_search_*` |
| 时间线 | `TimelinePanel.vue` | `timeline_*`、`fs_timeline_export` |
| 文件雕刻 | `CarvePanel.vue` | `carve_*`、`carve-scan-progress`、`task-progress` |
| 加密与历史卷 | 解锁控件、卷影副本 | `bitlocker_*`、`encrypted_volume_unlock`、`shadow_copy_*` |
| iOS 本地备份 | 镜像信息页的解锁与异常检查 | `ios_backup_unlock`、`ios_backup_audit`，操作与边界见 [iOS 备份](IOS_BACKUPS.md) |
| 挂载 | 卷挂载与共享控件 | `fs_mount_*`、`fs_unmount_readonly`、`fs_share_mount`、挂载点命令 |
| QR/OCR | 文件详情 | `qr_scan`、`ocr_scan`，可用性由构建特性决定 |
| 插件与任务 | 设置、任务中心、结果面板 | `plugin_*`、`sched_*`、`analysis_*`、`sched-task-event` |
| 后台任务 | 任务中心 | `task_list`、`task_status`、`task_cancel`、`task-lifecycle`、`task-progress` |

## 扩展流程

新增跨 IPC 功能通常需要同一改动完成：

1. `src-tauri/src/core/` 增加核心逻辑。
2. `src-tauri/src/commands/` 增加命令 wrapper。
3. `src-tauri/src/lib.rs` 注册到 `tauri::generate_handler!`。
4. `src/lib/ipc/<domain>.ts` 增加类型化封装。
5. 组件或 composable 调用封装。
6. 运行 `just ipc-check`、`just typecheck`、`just test-fe`、`just build-fe` 和相关 Rust 检查，完整矩阵见 [贡献指南](../CONTRIBUTING.md)。

事件需要明确的 payload、归属、清理和错误处理；插件能力与 QR/OCR 等可选能力要显示准确的可用状态。新增入口必须有已注册的后端命令，不保留“暂未接入”的空壳菜单。修改后检查切换镜像、面板卸载与迟到响应，避免旧任务的结果写入新的上下文。
