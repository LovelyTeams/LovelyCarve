# LovelyCarve 架构说明

LovelyCarve 是一个 Rust Cargo 工作区：GPUI Kit 原生桌面应用与独立 CLI 两个前端在进程内嵌入同一个后端，直接调用其命令函数，不经过 WebView 或 IPC 桥。后端使用异步任务和纯 Rust 的镜像、文件系统与加密容器读取器。

主流程是：创建或打开案件，添加镜像文件、文件夹证据、物理磁盘或 RAID 成员盘，检测分区和文件系统，浏览文件列表与原类型预览，执行哈希、导出、解锁、卷快照与索引，再运行全局搜索、时间线、文件恢复和插件分析。

原镜像缺失时，`image_load` 可恢复当前案件已发布的文件系统索引：校验索引格式、检材与卷身份、构建时间和指纹后，沿用案件检材 ID 建立 `index_only` 会话。目录分页、过滤与元数据查询仍由只读 filedb 提供；原镜像的分区几何未知，且所有字节读取都明确失败，不触发自动快照、哈希或平台识别。文件详情默认显示索引属性。恢复原路径的镜像后重新打开检材，会替换离线会话并恢复正常读取；没有有效当前索引时仍报告打开失败。

离线会话的分析来源定位通过已验证的当前卷索引解析卷入口、祖先链与卷内路径；LVM 逻辑卷可位于分区索引的子树中，无需原盘分区几何。解析保留符号链接跳转限制与删除项唯一性要求，目录未被索引时提示恢复原镜像，定位成功后仍只提供索引属性。

使用与构建入口见 [文档导航](README.md) 和 [开发指南](DEVELOPMENT.md)。本文先说明工作区与 crate 边界，再按证据读取、任务与插件、案件存储与索引、分析功能组织后端实现，最后记录桌面应用的结构与交互约定；各格式的实现范围与读取器维护要求见 [格式与读取器](formats/README.md)。

主要章节：[工作区结构](#工作区结构)、[后端结构](#后端结构)、[镜像与证据源](#镜像与证据源)、[任务与事件](#任务与事件)、[插件任务调度](#插件任务调度)、[插件运行时](#插件运行时)、[案件数据库](#案件数据库)、[元数据索引](#元数据索引层filedb)、[时间线](#时间线)、[文件恢复与雕刻](#文件恢复与雕刻)、[桌面应用](#桌面应用)、[功能与命令对照](#功能与命令对照)、[扩展流程](#扩展流程)。

```text
┌──────────────────────────────┐   ┌──────────────────────────────┐
│ lovelycarve-desktop          │   │ lovelycarve-cli              │
│ LovelyCarve.exe (GPUI Kit)   │   │ lovelycarve-cli (no window)  │
│ views → Backend::run         │   │ generated dispatch           │
│ EventHub ← event channel     │   │ JSONL / shell / resident     │
└──────────────┬───────────────┘   └──────────────┬───────────────┘
               │ in-process calls                 │
               ▼                                  ▼
┌─────────────────────────────────────────────────────────────────┐
│ lovelycarve-backend                                             │
│ embed · command_registry! · commands::* · app::workflows        │
│ state::AppState · task_manager · host (State/AppHandle/…)       │
└──────────────────────────────┬──────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│ lovelycarve-core: evidence · filesystem · catalog · casework ·  │
│ analysis · carving · search · timeline · preview · extensions · │
│ platform · common          (vendor/ readers, plugins/ SDK)      │
└─────────────────────────────────────────────────────────────────┘
```

## 工作区结构

```text
Cargo.toml          工作区：成员、共享依赖版本、lint、构建 profile 与 notatin 补丁
crates/
├── core/           lovelycarve-core：十二个取证引擎域；examples/ 探针与基准，tests/、test_data/ 测试样本
├── backend/        lovelycarve-backend：AppState、任务登记、宿主类型、应用工作流、命令函数与命令注册表
├── cli/            lovelycarve-cli：独立 CLI；构建脚本由命令注册表和 commands.toml 生成分发与命令目录
└── desktop/        lovelycarve-desktop：GPUI Kit 桌面应用，可执行文件 LovelyCarve；构建脚本嵌入 Windows 应用图标
vendor/             vendored 读取器 crate（各自独立的清单与锁文件）
plugins/            插件子模块（独立 Cargo 工作区：SDK、lc-plugin-host 与随附插件）
python/             基于 lovelycarve-core 的只读 Python 读取库
binaries/           随可执行文件发布的运行时 DLL
resources/          QR/OCR 构建运行时加载的模型
```

依赖只沿一个方向：`lovelycarve-core` 不依赖其他工作区 crate；`lovelycarve-backend` 依赖 core，并以 `pub use lovelycarve_core as core` 重导出，后端模块统一写作 `crate::core::…`；CLI 与桌面应用只依赖后端，在 crate 根部为所用的后端模块起同名别名。其他 crate 需要的条目才声明为 `pub`，其余保持 `pub(crate)` 或更窄。多个 crate 的测试共用的样本构造器放在 core 的 `test_support` 模块，以 `any(test, feature = "test-support")` 编译，`test-support` 特性只由开发依赖开启。

GPUI 会为 serde_json 打开 `preserve_order`。若与 CLI 在同一次 Cargo 调用中编译，CLI 动态 JSON 输出的键顺序会随之改变；因此工作区 `default-members` 不含桌面应用，`just` 配方也总是单独编译它。检查 CLI 输出时不要把 `-p lovelycarve-desktop` 与 CLI 放在同一次构建中。

后端构建脚本把 `binaries/` 下的 Dokan、OpenSSL 与 VC++ 运行时复制到当前 profile 目录，CLI、桌面应用及其测试直接运行即可找到；发行包由 `scripts/pack.ps1` / `scripts/pack-cli.ps1` 放到可执行文件旁。

## 后端结构

后端分为两个 crate：`lovelycarve-core` 是不带应用状态的取证引擎，`lovelycarve-backend` 在其上提供共享状态、任务、工作流和命令函数。基础证据读者不引用文件系统或插件实现，文件系统不引用 catalog 或插件宿主，持久化调度器不直接读写分析结果和时间线。案件发布、索引和路径模型仍按业务协作；十二个域不要求形成完全独立的 crate 或 DAG。

```text
crates/backend/src/
├── lib.rs                  唯一的命令注册表 command_registry![…]；重导出 core
├── embed.rs                前端的初始化、关闭与导出名清理入口
├── host.rs                 宿主接口：State、AppHandle、LeaseOwner、Response
├── app.rs + app/           setup、lifecycle、access、error、module_catalog
│   └── workflows/          证据加载/卸载、案件、快照、识别、Docker、挂载、外部卷和插件任务编排
├── state.rs + state/       AppState；预览、快照、遍历与运行状态辅助类型
├── task_manager.rs + task_manager/  即时任务登记、取消、资源、模型和测试
└── commands.rs + commands/ 命令函数，按十个业务域组织
crates/core/src/
├── lib.rs                  只声明十二个职责域
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

`commands` 的十个域是 `evidence`、`casework`、`filesystem`、`preview`、`analysis`、`carving`、`search`、`timeline`、`extensions`、`system`。跨域流程在 `app/workflows` 中复用；应用层不调用命令层，core 不依赖后端、前端或 GUI 工具包。应用错误定义在 `app/error.rs`，`commands/app_error.rs` 保留传输层导出。

命令是 `commands/` 下普通的 `pub`（多为 `async`）函数，宿主注入的参数使用 `host::State`（借用共享状态）、`host::AppHandle`（事件与每用户数据目录）和 `host::LeaseOwner`（物化媒体的租约所有者），其余参数可由 JSON 反序列化，返回 `Result<T, AppError>`，原始字节用 `host::Response`。`lib.rs` 的 `command_registry![…]` 是唯一注册表：展开时逐个引用这些函数，改名或删除的命令直接编译失败；CLI 构建脚本读取同一份列表，沿模块声明解析嵌套命令路径和真实函数签名，与 `crates/cli/src/commands.toml` 的命令定义一起生成分发代码与命令目录（见[命令行与桌面只读挂载](#命令行与桌面只读挂载)）。命令需要新的宿主能力时扩展 `host`，不把前端类型传入命令。

两个前端都经 `embed` 准备与关闭后端：`embed::initialize` 调用 `app/setup.rs` 的 `initialize`（预览临时目录、插件发现，以及任务、调度器与插件事件到宿主的路由），`embed::shutdown` 调用 `app/lifecycle.rs` 的 `shutdown`，依次取消任务、关闭案件、停止插件并释放媒体缓存。CLI 的 `AppHandle` 把事件写到 `--events` 的标准错误或直接丢弃，`LeaseOwner` 归进程所有；桌面应用在专用 tokio 运行时上调用命令函数，`AppHandle` 的事件经通道交给界面线程，再分发给订阅的页面。两者使用同一个每用户数据目录，名称为 `host::APP_IDENTIFIER`（`app.lovelycarve`）。

`python/` 是 PyO3 / maturin 的只读 `lovelycarve` 包，直接使用 `DiskImage`、`EvidenceSession` 与文件系统读取接口，不经过命令、`AppState` 或应用初始化。它依赖 `lovelycarve-core` 并开启 `reader-library` 特性，只编译 evidence、filesystem、platform 与 common，排除案件、catalog、分析、预览、搜索、插件和挂载模块；其他构建与 core 的单元测试仍编译完整模块。Cargo 依赖声明仍与 core 共用，扩展只链接读取实现。证据路径规范化实现在 `common::evidence_path`，原有 `casework::case::evidence_path_key` 保留同一函数的重导出，使读取器不必依赖案件模块。Python API、构建与限制见 [Python 读取库](../python/README.md)。
CLI 的可选 `--with-source` / JSONL `withSource` 在解析检材与路径后、命令执行前捕获来源，随后独立于条目投影封装 `source/result`；常驻会话传递同一请求标记，客户端先拆开原结果检查完整性、投影，再封装来源。来源不通过命令完成后的上下文查询猜测。SQLite 主键另标记 `sqlite` 地址空间，保留数据库节点和表名。DSH 默认启用来源封装，并在输出超限时保留原页位置生成只读重试建议。

`filedb_volumes` 的 `indexes` 列出当前已发布索引对各文件系统的覆盖：直接卷索引的 `pathPrefix` 为 null，父分区内逻辑卷的覆盖由索引中的实际节点路径定位。`filedb_query_files` 区分缺失索引与零命中，并在查询前验证索引身份和完整性；截断索引不能产生完整的否定结论。

CLI JSONL 调度位于 `cli/session.rs`。`session --jobs N` 使用有界输入队列和查询工作线程，主协调器统一发布按请求 ID 关联的 JSON 响应；stdin 等待不会阻塞完成结果输出。并发查询复制派发时的会话上下文，案件／检材／目录切换及其他未标记并发的命令先等待已有查询完成。结构化 `$ref` 等待先前请求结果，EOF 收齐请求后再等待后台任务。新增命令默认是屏障；确认其生命周期与并发安全后，才在 `cli/commands.toml` 中标记 `concurrent = true`，并通过命令目录的 `concurrent` 字段公开。

`crates/core/examples/` 按用途分类，所有 example 目标在 Cargo 中显式声明，通过 `just run-example <名称>` 调用；`just check-examples` 检查这些目标，用法见 [后端探针](../crates/core/examples/README.md)。新增模块使用 `<name>.rs` 加 `<name>/`，不新增 `mod.rs`。

`core::evidence::image::DiskImage` 负责镜像源、分区布局、字节读取、解密和定位缓存。`AppState.images` 登记的对象是 `Arc<core::filesystem::evidence_session::EvidenceSession>`：它持有同一个字节读者，并拥有宿主 Docker、NTFS 删除记录投影及外部卷注册表。字节扫描、RAID、哈希和源字节导出借用 `disk_image()`；文件树、预览和插件证据浏览借用 session。原证据 ID、路径、序列化字段和分区寻址保持不变。

`EvidenceSession::raw_export_reader` 提供独立的 RAW 导出投影。`image_export_raw` 默认 `mode: source`，桌面镜像头部使用 `unlockedPartitions`，导出开始时固定已解锁的 LUKS / VeraCrypt / BitLocker 读者。明文放在原分区起始位置，容器开销对应的分区尾部补零；整盘大小、分区表、未解锁分区和分区间隙保留原字节。范围重叠、溢出、越界、明文过长及短读明确失败，关闭解密会话不回退到密文。该投影不影响普通读取、源哈希和证据身份；尾部补零是导出转换规则，源坏区仍沿用读故障审计。导出目标以 `create_new` 创建。

Buffalo ENC / Broadcom TRX、TP-Link SafeLoader、U-Boot legacy uImage 与 Netgear CHK 固件由 `evidence/readers/firmware.rs` 在打开阶段校验封装并定位分段，Buffalo ENC 另建立明文视图；原始字节读取与源哈希保留容器内容。固件段按字节寻址，SquashFS 适配器通过 backhand 的版本无关接口选择 v1/v2、标准 v3/v4 或 DD-WRT `hsqt` 逐块参数 LZMA 读者，标准 v3 块优先 gzip、其次用纯 Rust adaptive / 标准 LZMA 解码并验证终止与源边界；范围、流式与完整读取共用同一逻辑内容。v1/v2 在读取器 crate 内建立有界元数据与块计划，旧特殊文件时间不代入文件系统创建时间，无 EOS 的已知长度 LZMA 由独立 Rust 解码器验证最终 range code 和源 EOF。范围与上游依据见 [固件](formats/FIRMWARE.md) 和 [SquashFS](formats/SQUASHFS.md)。

Sercomm NAND ZIP 固件由独立 Rust `sercomm-firmware` 库复用 ZIP / DEFLATE / UBI 读取能力，验证 seal、成员范围、CRC、尾部与 NAND 布局，宿主复用 uImage CRC 校验。`FirmwareImage` 只持有逻辑 `SectorSource` 与元数据，`DiskImage` 之间不相互持有；源 `DiskImage` 保留压缩字节与真实分段范围，由 `EvidenceSession` 构建并持有独立的 native firmware views；文件系统分派只对对应的分段 / UBI 节点解析逻辑字节，普通源读取及 RAW 导出不走该视图。视图沿用检材与分段 ID，快照取逻辑卷长，压缩数据不生成伪造物理 runs；不占用加密解锁槽，也不进入 `unlockedPartitions`。native view 与活动读取句柄由会话所有，释放后不保留额外全局缓存。

MINIFS 适配器 `filesystem::volumes::minifs` 复用本仓库独立 Rust `minifs` 库，通过有界 `ReadAt` 读取 v3 名称、文件与共享 LZMA 块索引；目录由路径合成，完整、流式和范围读取共用一个可释放的解压块缓存。裸镜像和 TP-Link 显式分段保留字节几何、源身份和原始容器字节，不虚构时间戳或物理 runs。旧布局与外层完整性边界见 [MINIFS](formats/MINIFS.md)。

JFFS2 适配器 `filesystem::volumes::jffs2` 通过有界 `ReadAt` 复用改造后的 vendor `jffs2::Jffs2Image`。扫描校验节点结构，建立最高版本目录 / inode 和非重叠片段索引；文件数据延迟校验与解码。`jffs2:part:` 节点按分区和路径区分硬链接身份，完整、流式与范围读取共用索引，源字节不变。支持边界见 [JFFS2](formats/JFFS2.md)。

YAFFS2 适配器 `filesystem::volumes::yaffs2` 通过有界 `ReadAt` 和 `Box<dyn ReadOnlyFilesystem>` 复用 Rust `yaffs2-reader`，具体页索引及其所有权留在库内。根据 page / spare 标签探测几何，按 sequence 与物理页序重建当前名称、shrink / shadow 与数据页；`yaffs2:part:` 路径保留硬链接目录身份，内容 inode 沿用目标。完整、流式与范围读取共用逻辑页图；原始读取保留 OOB，不生成虚假的连续物理 runs。支持范围与 NAND ECC 限制见 [YAFFS2](formats/YAFFS2.md)。

同一 Rust 库的 YAFFS1 扩展使用独立分散 tags / ECC 与前向 serial 扫描，复用只读接口和目录索引。分区类型与节点前缀为 `yaffs1`，适配器检查前缀与实际布局一致，不能通过另一版本前缀访问或混用缓存。已分配数据读取验证完整 512 字节页的已提供 ECC；宿主采用头声明长度，库可按已知源配置选择数据页长度策略。范围见 [YAFFS1](formats/YAFFS1.md)。

UBIFS 适配器 `filesystem::volumes::ubifs` 将裸分区、固件明文和 UBI 逻辑卷接到 vendor `ubidump::UbifsImage` 的有界 `ReadAt`。按主节点选择已提交索引，缓存目录/inode 和数据键位置；数据 CRC 与解码延迟到实际范围读取。`ubifs:part:` 与 `ubifs:ubi:part:` 节点保留分区、卷与路径身份，硬链接不折叠成同一个目录节点；源读始终保留原始证据。支持边界见 [UBIFS](formats/UBIFS.md)。

CramFS / Linux ROMFS 复用 vendor 的 lamfold Rust 读取器，适配器位于 `filesystem/volumes/rom_images.rs`。完整、流式与范围读取经同一文件内容路径，声明范围、CRC/校验和与循环目录明确校验；裸镜像用字节几何，真实分区保留原始 LBA，原容器字节不变。能力和限制见 [ROM_IMAGES](formats/ROM_IMAGES.md)。

外部卷通过 `filesystem::external_volume::ExternalVolume` 提供只读 Volume 和关闭契约；插件 `RemoteVolume` 实现这个接口。未知分区的插件探测、源登记和卷安装由 `app/workflows/filesystem.rs` 编排。卸载先撤销 AppState 登记、取消任务和停止挂载，再 drain 外部卷并逐项关闭、移除源和其他缓存；持有调用句柄的线程释放前，provider backing 继续存活。字节读者本身不持有插件连接。

`core/extensions/task_scheduler` 保留台账、状态机、派发、恢复、进度、看门狗和 post-action 契约。生产 `PluginTaskRunner` 位于 `app/workflows/scheduler.rs`，负责准备运行（`core/extensions/plugin/run_setup.rs`）、实例存活、执行、`records.db` 发布、时间线投影和结果清理；清理失败仍按原语义记录告警并完成任务删除。插件 SDK 保留 ABI、LPP 载荷与记录结构定义，`core/analysis/results/` 的 `record_store` / `record_query` 按结构保存与查询类型化记录，`models.rs` 是结果树、表格与详情的 IPC 投影；存储不依赖插件宿主，插件卸载后结果仍可读。

LVM 标签/CRC/活动副本位于 `evidence/partitions/lvm2/header.rs`，有界文本 grammar 与 VG/PV/LV/segment/stripe 模型位于 `text.rs`，完整线性 LV 字节映射与 read/seek 位于 `layout.rs`；文件系统与 RAID 共用。活动 ring 回绕、远端备份和成员 UUID 按同一读取规则处理，不以历史文本扫描替代损坏的当前副本。原生 ext/XFS/Btrfs/NTFS 读者支持单 PV 碎片段，来源 runs 映射回原始镜像；完整线性 LV 由 RAID 检测消费：作为 md 成员，或注册为可选择的合成检材（单 PV 与跨 PV 一致），缺失成员不缩短为可读卷。文件系统层构造导航、原始 LV 文件与未使用 PE 投影，RAID 不反向调用导航实现。完整 libvslvm API 对照与限制见 [LVM2](formats/LVM.md)。

RAID 检测（`evidence/raid`）分六步：读取各检材的 LVM PV 并物化完整线性 LV；在 LV、分区和整盘上探测 md 超级块（0.90 / 1.0 / 1.1 / 1.2，末尾超级块归属最内层设备）；`md_array.rs` 以事件计数最高的超级块为准判定当前成员、槽位与降级条件，`source/build.rs` 按级别构建 linear、RAID0/1/4/5/6/10 读取器；不属于整盘 md 成员的整盘检材再探测 Intel IMSM（`imsm.rs`）与 SNIA DDF（`ddf.rs`）容器元数据，`imsm_identity.rs` 把检材对应到 IMSM 磁盘记录，`imsm_array.rs` / `ddf_array.rs` 选定权威副本并把成员对应到槽位，`firmware.rs` 按同一套降级规则用同一个 `build_volume` 组装每个卷；阵列上的 PV 与检材上的 PV 重新组成卷组（固件 RAID 成员盘上看到的 PV 不计入），跨 PV 的 LV 另行组装；剩余等大镜像交给无元数据启发式，md、固件 RAID 与卷组已占用的检材不参与。读取器只读数据块，缺失列按 P / Q 或副本重建，阵列范围内的短读报错。元数据仍登记但未加载的成员不触发自动降级，前端通过 `raid_auto_detect` 的 `degradedSets` 传入检验人员的确认。格式能力与限制见 [Linux md RAID](formats/MDRAID.md) 与 [固件 RAID](formats/FIRMWARE_RAID.md)。

无元数据 RAID 按等大小分组后，每块成员的奇偶采样行、占用采样和数据偏移探测每组只读取一次：各成员并行读取，同一成员按偏移升序读取以复用其 64 KiB 块缓存；子集排序、奇偶投票、陈旧成员判定和偏移推导都在内存中计算，不再为每项检查和每个候选子集重读、重解压成员。数据偏移探测保留小偏移候选，并以 512 KiB 步长扫描各成员前 256 MiB；只有命中 MBR、NTFS 或 ext4 签名的偏移才进入重组搜索，偏移候选总数最多 32 个，随后仍须通过文件系统结构和目录／MFT 验证。ext4 验证最多检查 256 个 inode 和 24 个目录，继续进入共享目录的子树并核对 `.`／`..`，不要求根下存在三个目录；备份超级块及已知文件格式的正文签名优先用于区分早期目录同样可读的布局，文件扩展名与签名不一致只是不贡献分数。此路径覆盖 mdadm 头被清空而数据区仍在大预留区之后的 NAS 检材。`just run-example probe_raid --member-signatures <image>...` 可只读检查成员头尾和该范围内的 ext4 签名；真实 NAS 回归可设置 `LOVELYCARVE_NAS_FIXTURE_DIR` 后运行 `just test real_nas_erased_mdadm -- --ignored`。

读者入口保留公共模型、打开方式和源身份；具体实现按职责放在入口同名目录。Btrfs 分为 open/directory/data/tree/codec，NTFS 分为 backing/directory/deleted/scan/read/metadata/models 与 logical_stream/data_stream/lznt1/wof/reparse/forensic_metadata/security/mft_metadata/recovery，ZFS 分为 directory/read；`fs_dispatch` 保留 Volume/FsAdapter 契约和原注册顺序，各特殊卷适配在 ntfs/ext4/btrfs/zfs 子模块。RAID 的 manual/detect/members/progress/mdadm_super/md_array/firmware/imsm/imsm_identity/imsm_array/ddf/ddf_geometry/ddf_array/lvm_meta/lvm_stack/probe/xfs_probe、读取器 source/（striped/mirror/raid10/linear/parity_layout/gf256/rebuild/traced/build）与 heuristic 的 samples/search/parity/validation/ext4_probe/ext4_anchor/ntfs_probe/output 分开，原探测优先级、搜索预算和取消检查不变。

插件 client 保留请求句柄和实例存活状态，process/connection/router/host_requests 分别负责启动、握手、消息路由和反向 RPC；`run_context` 服务一次运行的 `run.*` / `file.*` / `view.*` 调用，`view` 实现系统视图与检材视图。Dokan 入口保留挂载会话和引用计数，filesystem/callbacks/paths/read 负责上下文、只读回调、路径和读取缓存。ReFS 通过本地 `refs-reader` crate 解析只读卷，filesystem 的 refs/backing/walk 分别负责节点适配、证据范围读取和可取消遍历；目录缓存随卷释放。普通文件、ADS 与快照版本按同一 extent 映射随机读取；platform/ffi 的旧实现只在显式 `refs-dll-oracle` 验证功能下编译。XLSX 预览分为 workbook/format/style，案件状态将检材登记和卷发布分别放在 case/evidence.rs、case/volumes.rs。相关原有测试与实现同域。

`commands.rs` 同时存放各命令文件共享的辅助函数。CSV 导出（时间线、全局搜索）的数据单元格都经
`commands::quote_csv_field` 转义：以 `=`、`+`、`-`、`@`、制表符或回车开头的值前置 `'`（`-12`、`-3.5` 等纯负数除外），
防止检材取值在电子表格中作为公式执行。桌面应用的 CSV 导出同样调用这些后端命令；新增 CSV 导出须复用该函数。

`crates/backend/src/lib.rs` 的 `command_registry![…]` 是后端命令注册的唯一入口。当前已注册的主要命令域包括：

- 镜像：`image_load`、`image_info`、`image_bad_regions`、`image_unload`、`image_list`、`image_export_raw`、`image_hash_compute`
- RAID/物理磁盘：`raid_auto_detect`、`raid_manual_assemble`、`physical_disks_list`
- 文件恢复（文件系统记录与签名雕刻）：`carve_formats`、`carve_start`、`carve_scan_status`、`carve_scan_pause`、`carve_scan_resume`、`carve_scan_list`、`carve_scan_remove`、`carve_results_page`、`carve_filesystem_results`、`carve_preview`、`carve_recover`
- 文件系统：`fs_children`、`fs_flat_page`、`fs_flat_ext_counts`、`fs_size_tree`、`fs_read`、`fs_read_large`、`fs_media_temp`、`fs_preview*`、`fs_export*`、`fs_digest`、`fs_identify_type`
- 原类型预览：SQLite、Realm、二进制、GGUF、ZIP、CSV、Vim swap 等由后端解析。需要完整文件的查看器经 `fs_media_temp`
  物化到临时文件，以 `LeaseOwner` 持有租约，结束时调用 `fs_media_release`；同一 `(image,node,ext)` 的物化互斥，
  完成前使用唯一临时文件，单写入通道约束瞬时磁盘占用。`MediaTempCache` 的 4 GiB / 6 文件预算只淘汰无人引用的
  最久未使用文件；租约所有者报告关闭后由后端兜底释放引用。新缓存位于 `lovelycarve-preview-cache/<uuid>`，
  持有目录外的进程锁；启动只回收能取得锁的旧目录，退出清理当前目录。
- 卷/挂载：`fs_mount_readonly`、`fs_unmount_readonly`、`fs_mounts`、`fs_share_mount` / `fs_unshare_mount`、挂载点建议和创建
- BitLocker：状态、密码/恢复密钥/VMK 解锁、内存扫描、导出解密镜像
- 案件和快照：案件创建/打开/关闭、证据列表/移除、卷快照构建/列表/删除
- 插件与任务调度：`plugin_list`/`plugin_restart`/`plugin_install`/`plugin_uninstall`；
  `sched_task_functions`、`sched_systems_list`、`sched_task_create`、
  `sched_task_cancel`/`start`/`restart`/`remove`、`sched_evidence_post_actions_set`；
  `analysis_result_tree`、`analysis_records_page`、`analysis_record_detail`、`analysis_run_get`
- 任务中心：`task_list`、`task_status`、`task_cancel`
- 系统辅助：模块摘要、路径信息与在文件管理器中显示、二维码、OCR、OS 检测、Windows 盘符识别
- 全局搜索、时间线、加密卷与 iOS 备份解锁、卷影副本、BCP、标签与派生数据等其余命令见[功能与命令对照](#功能与命令对照)

## 核心数据流

1. 桌面应用的工作区（`workspace.rs`）确保当前有活动案件，检材标签页总是属于活动案件。
2. 用户选择镜像文件、文件夹证据、物理磁盘或 RAID 成员。
3. 界面经 `Backend::run` 调用 `image_load`，后端把证据注册到内存状态和活动案件。
4. 检材的导航栏显示镜像信息与文件列表，目录树按需调用 `fs_children_page` 或读取快照索引。
5. 文件详情通过 `fs_preview`、`fs_preview_text` 与原类型预览命令读取内容；原类型视图把文件物化后交给独立进程中的查看器。
6. 长任务通过后端事件推送进度，例如镜像加载、哈希、导出、BitLocker、快照构建；任务中心经 `task-entry` 跟随所有任务。
## 镜像与证据源

支持的主要源类型：

| 类型 | 后端路径 | 说明 |
| --- | --- | --- |
| RAW / DD / IMG / ISO / CUE | `core::evidence::image` + `vendor/smraw` + `readers::cue` | 单文件 mmap、分段 RAW positioned I/O；CUE 多文件/多轨保持原始字节，轨道用字节跨度寻址；Mode 1 / XA Form 1 单独投影 2048 字节文件系统数据；CUE 与所有成员进入稳定性及指纹，见 [SMRAW](formats/SMRAW.md) 与 [CDFS](formats/CDFS.md) |
| E01 / Ex01 | `core::evidence::readers::ewf_image` → vendored `ewf` (SecurityRonin/ewf-forensic) | 纯 Rust、多分卷、随机读取、v1 延迟块索引；无需 libewf DLL |
| VMDK | `core::evidence::readers::vmdk_image` + `vendor/vmdk` | vmdk-forensic 的纯 Rust flat / sparse / streamOptimized 读取；分卷与父盘源变化检测 |
| QCOW / QCOW2 | vendored `qcow2` | v1/2/3 稀疏/DEFLATE 虚拟盘、RAW/QCOW backing chains；legacy AES 通过 image_load 可选凭据打开，能力与边界见 [QCOW](formats/QCOW.md) |
| VDI | vendored `vdi` | VirtualBox VDI 1.1 动态盘与固定盘，只读稀疏随机访问；差分盘明确拒绝 |
| Parallels PHDI | vendored `phdi` | XML / .hdd bundle、v2 Plain / Sparse / Split / 差分父链；直接逻辑随机读、所有父链文件稳定性与指纹，见 [PHDI](formats/PHDI.md) |
| BitLocker | `core::evidence::crypto::bde_image` + `vendor/bitlocker` | 纯 Rust 密码/恢复密码/明文密钥/BEK/VMK/FVEK；六种 AES、Vista、EOW；直接读取分区，无 DLL 或临时整卷转储；验证边界见 [BITLOCKER](formats/BITLOCKER.md) |
| VHD / VHDX | `core::evidence::readers::vhd_image` → `vendor/vhd` / `vendor/vhdx` | 纯 Rust 固定/动态盘随机读取；VHDX 日志仅在内存重放，差分盘无父盘参数时明确拒绝 |
| DMG | vendored `udif` + `hfsplus` | UDIF 容器与 HFS+ |
| Xiaomi HDR1 firmware / UBI | `core::evidence::partitions::xiaomi_firmware` + `filesystem::volumes::ubi` → `vendor/ubidump` | HDR1 旧版与扩展分段表按字节寻址；UBI 命名卷重组为可读文件，内嵌 SquashFS 通过有界 seek 读者浏览，原始容器字节保持不变；边界见 [UBI](formats/UBI.md) |
| Android / MIUI backup | `core::evidence::readers::mi_bak` | `.ab` / `.bak` 解码为逻辑 TAR 树，原始字节源仍保留备份头和压缩数据；见 [Android 备份](formats/ANDROID_BACKUP.md) |
| LDBK | `core::evidence::readers::ldplayer_image` | 解包后按多分区盘处理 |
| TAR / TAR.GZ / ZIP / 7Z / folder | 逻辑证据源 | 无分区表，暴露为合成只读文件树 |
| PhysicalDrive | `core::evidence::physical_disk` | Windows 物理磁盘，通常需要管理员权限 |

ZIP 与本机目录检材中的小米 `.bak` / Android `.ab` 复用 `readers::mi_bak`，跳过小米前缀、
解析 Android Backup 头并将 TAR 物化到自动清理的临时文件。ZIP 通过成员流直接
解码，不额外写出 `.bak` 副本；`zip_image::bak` 将 TAR 条目投影到原成员路径下，
读取与流式导出转给对应 `TarImage`。解码后的偏移不作为原 ZIP 的数据位置。
未识别、加密、解码失败或超出条目上限的备份保留原始文件节点；ZIP 成员完整读取、
CRC 与逻辑长度核对成功后才发布目录。格式与外部参考见 [小米备份](formats/MIUI_BAK.md)。
案件内的 `image_load` 把检材缓存目录（`EvidenceCache::Directory`）交给打开器：
`zip_image::bak::listing` 把一次完整解码得到的投影存为 `cache/zip-backups.zst`，按 ZIP 长度与
候选成员的路径、目录序号、本地头偏移、CRC-32 和大小绑定。再次打开时直接投影清单，备份在首次读取
内容时才解码，解出的 TAR 与清单逐条一致才对外提供；还原的备份参与证据指纹的值与即时解码相同，
案件索引照常复用。

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
调用方只在首次进入或展开 `host-docker:<container-id>:` 时经 `fs_docker_prepare` 触发合层，并显示不伪造完成比例的
加载状态；子项计数的后台预取会跳过尚未合层的容器根。容器根属于导航与挂载入口，不参与“隐藏
空目录”判定；进入容器后，普通目录仍沿用已有空目录过滤规则。

### EWF 读取器

`core/evidence/readers/ewf_image.rs` 把 ewf-forensic 仓库的纯 Rust `ewf` 读取器适配为
`ImageSource::Ewf`，不加载 libewf。`EwfAcquiryInfo` 提供采集元数据，分卷路径、大小及
时间戳用于长任务的证据稳定性检查。v1 使用延迟块表和有界缓存，v2 使用
上游的完整块表。缺失分卷在打开时拒绝，解码 / 校验故障交由 tolerant read
层报告。上游版本及本地补丁见 `vendor/ewf/LOVELYCARVE.md`。

至少 1 MiB 的 EWF 读取绕过解码块 LRU，按最多四个较大的任务并行解码，直接填入
调用方缓冲区的互不重叠区间；小范围读取继续复用缓存。源盘哈希仍按逻辑偏移顺序
把完整解码字节流交给各算法，保留每块校验、长度检查及原有坏区审计。解压吞吐的
提高不等于所有算法等比例加速，MD5、SM3 等算法本身仍可能限制整条流水线。

源盘哈希、文件摘要和全局搜索结果的 MD5 统一使用 `core/common/hash.rs` 导出的
`fast-md5` 后端；x86_64 / aarch64 使用内联汇编，其他架构使用库的 Rust 回退实现，
不引入哈希 DLL。SHA-1、SHA-256 与 SM3 沿用原有实现。RustCrypto `md-5` 仅用于
测试和探针中的独立对照；MD5 块边界、填充边界、空更新及多算法流水线有回归覆盖。

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

`core/evidence/vss_image.rs` 接入 vendored `vsc-core`（`vendor/vsc`，来自 SecurityRonin/vsc-forensic）。
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
桌面应用的「卷与分区」页（`evidence/volumes.rs`）按分区列出副本，所选副本重建为只读镜像并在新标签页打开。
CLI 的 `shadow-copies list` / `shadow-copies open` 在同一会话中保留父镜像与副本。
读取器公开 `store_blocks` / `store_ranges` 原始枚举和 `Snapshot: Read + Seek`；完整 API 对照与
验证边界见 [VSS](formats/VSS.md)。catalog 损坏或循环时报错而不是返回部分列表，存储头整块读取并校验。
原始 catalog 尺寸保留用于调查，布局尺寸独立求值；异常时要求主备 NTFS 几何一致。

### AD1 逻辑镜像

`core/evidence/readers/ad1_image.rs` 接入 vendored `ad1-core`（`vendor/ad1`，来自 SecurityRonin/ad1-forensic）。
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

`image_load.rawReadMode` 默认为 `strict`。显式 `force` 通过 `image/forced_raw.rs` 与
`smraw::RawImage::open_forced` 把普通文件作为所选起点的 RAW 连续前缀，以 positioned I/O
惰性打开段；跨容量读截短、短读返回前缀，完全越界明确补零并报告绝对偏移的 `BadRegion`。
`OpenSectorSource` 的非缓存读取也保存探测阶段的坏区。该模式只在当前会话有效并进入证据指纹，
`ImageInfo.raw_read_mode` 明示模式；哈希、导出的源稳定性和最终长度校验继续生效。
发现规则、一次性容量警告与 Windows 单次重试见 [SMRAW](formats/SMRAW.md#不完整-raw-的强行读取)。

E01、VMDK、VHD/VHDX、PMFX 等容错后端遇到源读取错误时，先沿绝对源盘的 512 字节扇区边界二分失败范围，
成功的子范围保留原始字节，只有缩小至单个扇区后仍失败的范围才补零；非对齐请求只补零该扇区与请求的交集。
失败调用已写入的字节不会直接采信，必须重读或明确补零。待重试范围按顺序处理，可读范围恢复整块读取，避免逐扇区
重试整个剩余缓冲区造成二次方读取开销；压缩块无法解码时，该块内所有不可读扇区均会报告。补零的绝对源盘区间记录为
`BadRegion`。`image_bad_regions` 返回当前镜像与活动案件中持久化的合并报告；源盘哈希、RAW 镜像导出、
单文件哈希、单文件导出、目录树导出和 ZIP 目录导出都会在成功结果中返回 `bad_regions`、`synthetic_bytes` 和
`read_faults_detected`。文件/目录导出使用一次导出操作内的读故障收集作用域，报告的是该操作实际通过
`DiskImage::read_range` 命中的源盘坏区；它用于审计导出结果是否包含合成零字节，不表示逐个输出文件的
逻辑偏移到物理坏区映射。

CLI `files read` 在单次、JSONL 与常驻会话中都拒绝把有读故障的字节流报告为成功：返回
`readFaultsDetected` 错误，结构化错误的 details 保留本次操作的审计报告。单次 stdout 可能已经写出字节，
消费者必须检查退出码并丢弃失败输出。文件摘要继续容错返回并附带同一报告，CLI 把含补零的摘要和导出判为不完整结果
（照常输出，标准错误警告并以 2 退出）；桌面不将含补零的摘要写入不带审计字段的缓存／存档。

重组的 md 阵列与跨 PV 逻辑卷由组合读取器经 `from_assembled` 注册，成员的补零区间通过
`SectorSource::read_traced` 逐层上传：`WholeDiskMember`、`LinearLvSource` 与各级别读取器
（`core/evidence/raid/source/`）把成员偏移换算成自己的逻辑偏移，`ImageSource::Assembled` 再把它们记入重组镜像
自己的坏区表。因此重组镜像的源盘哈希、RAW 导出和文件导出与普通检材一样返回上述三个字段，严格读取策略同样
生效，叠在阵列上的解密视图与卷影副本也会拒绝这些区间。成员读取走 `DiskImage::read_range_traced`：成员镜像
照常记录自己的坏区，但成员偏移的区间不会混入读取重组镜像那次操作的收集作用域。新的组合读取器必须实现
`read_traced`，否则成员的补零会在这一层被当作真实字节。

有冗余的级别把成员补零的字节视同缺失：RAID1 与 RAID10 改读其他副本，RAID4/5/6 按校验重建，与 md 处理成员
读错误的方式一致；只有没有可信来源能提供的字节保持为零并按逻辑偏移记为坏区，重建不拿补零字节参与运算。
每个重组设备第一次这样改读或重建时记一条警告，之后记为 debug。

## 任务与事件

长任务应返回任务 id 或明确结果，进度通过事件发送，取消走 `task_cancel`。后端长任务统一注册到
`TaskManager`，注册守卫负责在成功、错误、取消和 worker panic 后自动清理。任务记录包含任务类型、可选的
镜像归属、状态（`queued`、`running` 或 `cancellationRequested`）和已运行时间。`image_unload` 会先取消该镜像的
全部关联任务，避免已捕获 `Arc<DiskImage>` 的后台工作在卸载后继续消耗 I/O。

每个任务还带一份任务中心视图（`TaskView`：对象片段、阶段、进度、计数、标签与标志），由命令经自己的登记
填写（`TaskRegistration::update`，或按 id 的 `TaskManager::update` / `emit_progress`），并在最后一个登记释放前
报告结局（`finish` / `finish_with`：成功、失败附错误、取消）；没有报告结局就释放的任务按令牌状态记为取消或
中断。取消的任务立即让出 id，在 worker 停下前仍列为「取消中」。任务种类决定列出方式（`TaskKind::listing`）：
页面自己展示的短任务（预览物化、二维码、OCR、BitLocker 扫描与解锁等）不列出；加载、哈希、镜像与目录导出、
加密卷解锁、平台识别、快照与索引、Docker 合并、恢复文件导出结束后留在本次会话的历史里（检材卸载时清除，
不属于任何检材的打开检材任务在切换案件时清除）；全局搜索、时间线与文件恢复扫描结束后由各自的存储列出
（搜索历史、时间线状态、内存中的扫描会话）。

`TaskManager` 发出 `task-lifecycle`（`phase`：`queued` / `running` / `updated` / `cancellationRequested` /
`finished`，`finished` 带最终快照与结局）和带进度的 `task-progress`（`unit`：`bytes` / `entries` / `attempts` /
`percent`、`done`、可选 `total` 与 `message`）。旧的领域进度事件仍保留给各自页面。

任务中心（`app/workflows/task_center.rs`）把插件任务与原生任务列为同一种条目 `TaskEntry`：id、检材、调度器的
状态词、是否取消中、是否自动触发、创建 / 开始 / 结束时间、进度、错误与可用操作，再加来源——插件任务带完整
任务行，原生任务带种类与视图，文件恢复扫描另带会话 id。`task_list(image_id?)` 列出运行中的、本次会话结束的、
各存储保留的原生任务与全部插件任务（不属于任何检材的任务总会列出），`task_status` 按 id 查一条，`task_cancel`
取消原生任务或 `plugin:<id>` 插件任务。启动时（`app/setup.rs`）把调度器事件、原生任务的生命周期与进度事件和
文件恢复会话的变化统一转成 `task-entry` 事件：`changed` 带条目、`removed` 带 id、`notice` 带调度通知。

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
`core/extensions/task_scheduler/`，以案件台账 `case.db` 的 `task` / `task_followup` 表为持久化来源
（见「案件数据库」）。插件、分析器、触发方式、父任务与失败原因各是一列，只有 `params` /
`result` 是 JSON；线上形态 `SchedTaskRecord` 仍把前几项收在 `info` 里。插件实例只在进程内
登记，不落库。

```text
core/extensions/task_scheduler.rs      模块入口
├── config.rs               SchedulerConfig（LOVELYCARVE_TASK_* 环境变量）与 SchedError
├── model.rs                状态机 created/queued/running/success/failed/cancel/offline（旧案件另有 pause）、
│                           任务行与参数（目标、选项）、EvidenceTaskStatus 即时统计、SchedEvent 广播
├── store.rs                台账 task 表（调度器自己的 WAL 连接）；store_evidence.rs 负责
│                           检材主键映射、任务状态统计与 task_followup 收尾动作
├── functions.rs            Function 注册表：每个分析器 = 一个 function `<plugin id>/<analyzer>`，
│                           作用域、平台、触发、选项与画像事实取自包清单
├── scheduler.rs            绑定/崩溃恢复、调度循环、created→queued 派发、系统来源
├── lifecycle.rs            queued→running→发布→commit、OnTaskFinish、离线分流、进度回流
├── control.rs              创建（目标与选项校验）、取消、开始、重启、删除与证据上下线钩子
├── executor.rs             TaskRunner trait（执行、发布、清理）
├── dependence.rs           检材上线的首任务与按画像事实创建的后续任务
├── post_action.rs          PostActionWatcher：防抖、幂等、initFromDB 恢复
└── watchdog.rs             离线 / 进度超时巡检与恢复分流
```

**作业。** 一个作业是（function、检材、目标）：目标是识别出的系统 ID（系统级）、文件节点（文件级），
或整个检材（检材级，空串）。同一作业同时最多一条未终结任务（`single_instance`，默认开），创建时在同一写事务里检查，
命中时返回已有任务（`created: false`）；整个检材的作业另有台账的部分唯一索引 `task_single_active` 兜底。
任务参数 `params` 记录目标（`nodeId` / `fileName`、`systemId` / `targetLabel`）与按清单校验过的选项；
旧案件任务行里的平台、子项字段读取时忽略。

**生命周期。** `sched_task_create`（或证据上线、画像事实发布）写入 `created` 行 → 派发循环每 500 ms
按创建顺序派发 `created` 任务（`queued`），同时处于 queued/running 的任务不超过
`LOVELYCARVE_TASK_MAX_RUNNING`（默认 4，最大 32，不区分插件）→ 执行前置 `running`、写开始时间
→ `PluginTaskRunner`（`app/workflows/scheduler.rs`）为这次执行单独起一个插件实例（新的
`lc-plugin-host` 进程，内存上限取自清单），按 `run_setup.rs` 准备运行（打开系统视图或检材视图、
解析声明的输入、补齐选项与画像事实、写暂存运行）后发送 `run.start`，同一插件的多条任务因此并行
→ `run.progress` 只回流到这次执行：回报只更新内存登记并立即刷新看门狗计时，派发循环把最新百分比
落库（百分比变化即写、否则 1 s 节流），插件线程从不等待台账 → 运行返回后先把暂存运行发布到检材的
`records.db`（替换同一作业的上一次结果并投影时间线），再 commit `success`/`failed`，实例随后退出
→ OnTaskFinish：PostActionWatcher 计数、成功时按画像事实创建后续任务（见下）。每次状态或进度变化都
广播调度事件，任务中心把它转成 `task-entry`。文件系统插件的共享实例（Unknown 分区浏览）与任务实例互不影响。

**自动任务。** 证据首次登记或重新上线时，系统来源（`state.systems`，按检材缓存的
`identify::systems::discover` 结果）列出识别出的系统；每个自动触发、不等待事实（`requires` 为空）的
分析器建首任务：检材级一条，系统级在每个适用平台的系统上各一条。任务成功后，等待它清单 `provides`
中事实的自动分析器在同一系统（或整个检材）上建任务，所需事实都要有成功的发布者；同一作业已有任务时
不再自动创建，自动链因此不会循环。失败或取消不建后续任务；文件级分析器只能手动。

SQLite 写入：调度器对台账的读写全部在阻塞线程池上进行，台账忙时不占用异步运行时；状态迁移在一个
`BEGIN IMMEDIATE` 事务里先校验当前状态再写（`TaskStore::transition` / `complete`），并行任务的提交、
控制面与看门狗同时改写同一行时以先写者为准，迟到的一方不覆盖终态；进度只更新仍为 `running` 的行。
检材的任务状态统计（PostActionWatcher 的「全部完成」判定）由任务行即时计算，不另存聚合。任务行的写入与其
调度事件广播在同一把顺序锁内完成，广播顺序等于写入顺序，订阅方不会在终态之后又收到旧的
"运行中"进度。`records.db` 的发布与删除、时间线增量投影各是检材自己库上的一个 IMMEDIATE 事务：
同一检材的任务同时收尾时在该库写锁上排队，不同检材互不等待；时间线水位取已有值与发布时间的较大者，
钩子乱序提交也只前进。

控制面：取消允许 `created/queued/running/pause/offline`，运行中的执行撤销 token 并结束这次执行的
`lc-plugin-host` 进程，同一插件的其他任务不受影响；迟到的旧结果被丢弃，暂存运行随之删除。任务不能暂停；
旧案件里处于 `pause` 的任务可以开始（从头重跑）或取消。重启终态任务会新建一条 `parentTid` 指向原任务的
任务，重启离线任务直接重排队；删除只接受终态，并删除该任务发布的运行及其时间线投影。移除检材时先取消它的
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

桌面应用的任务中心（`tasks/center.rs`）从 `TaskStore` 读取插件任务，显示状态、进度、目标、错误与结果计数，并可取消运行中的任务；插件任务的开始、重新分析与删除（`sched_task_start` / `restart` / `remove`）目前由 CLI 提供。

**快速分析。** 任务面板中的快速分析（`analysis/run.rs`）先经 `sched_systems_list` 取检材上识别出的系统，再为所选分析器调用 `sched_task_create`（系统级带 `systemId`，选项取默认值）；作业由插件调度器执行，任务中心跟随其状态。

**分析结果页。** 分析结果页（`analysis.rs`、`analysis/records.rs`）按以下结构呈现：

- 目录（`analysis_result_tree` → `core/analysis/results/analysis_query.rs`）：检材 → 分类 → 分析器运行（每个作业一份已发布运行，带目标与告警数）→ 记录类型，并显示记录数；节点名来自插件声明。旧版本 `analysis.db` 的结果（`legacy_query.rs`）以「旧版」节点并列。
- 记录表格（`analysis_records_page`）：按记录类型服务端分页，列与字段类型来自插件的记录结构；分页固定结果树节点的 `runId`。
- 记录详情：所选记录的全部字段与来源。

单条详情 `analysis_record_detail` 与运行概览 `analysis_run_get`（关键数字、各记录类型产出、告警、输入文件、选项、画像事实、读不出的源字节与技术标识）目前由 CLI 使用。

## 插件运行时

插件运行时位于 `core::extensions::plugin`。插件包是 `plugin.toml`（`manifest.rs`）、导出 C ABI
（`lovelycarve_plugin_v1`）的原生动态库、构建时导出的 `schema.json` 与 `locales/`（`package.rs`）；
`manager.rs` 发现、登记与重新发现插件包，`package_install.rs` 安装与卸载 `.lcpkg`。每个插件实例由随应用分发的
`lc-plugin-host` 子进程加载，宿主经 4 字节大端长度前缀的 MessagePack stdio 帧（LPP v2）与之通信，
独立 reader/writer loop 支持双向嵌套 RPC（`client.rs`）。握手时核对库的 `plugin.hello` 与清单、
`schema.json`。插件 SDK、C ABI 定义、协议载荷、记录结构与 `lc-plugin-host` 在 `plugins/` Rust 工作区
（`plugins/sdk` 同时是宿主依赖；`sdk::bridge` 把入口函数接到 LPP 帧上，加载器与宿主测试的回环连接共用）。

分析运行（`run.rs`、`run_setup.rs`、`run_context.rs`）：每个任务一个专属实例；运行上下文持有这次运行的视图、
文件句柄、查找游标与记录写入器，服务插件的 `run.*` / `file.*` / `view.*` 调用，并按运行收集读故障。
`view.rs` 实现系统视图（系统卷在根部，fstab 与盘符挂载在挂载点）与检材视图（每个卷是以卷键命名的目录），
符号链接按视图根解析；视图目录列表经有界缓存（`directory_cache.rs`，最多 4096 个目录、65536 个节点）。
`identify::systems` 识别系统并组装挂载（`fstab.rs` 解析 fstab，`filesystem::volume_identity` 读取卷的
UUID 与卷标，`win_drive_letters` 读取盘符），结果按检材缓存，卸载检材时清除。插件拿不到证据路径，
只能经视图、文件句柄与 `SourceRegistry` 中的只读逻辑源读取。

文件系统插件：Unknown 分区在首次展开时通过插件管理器的共享实例执行 `fs.probe` / `fs.open`，命中后安装
`RemoteVolume`；其 children、stat、范围读取和流式读取继续复用原有 `Volume` / `FsSession` dispatch。

插件的日志经 `lc-plugin-host` 写出的 stderr 行（`client/process.rs` 的 stderr 读取任务）进入原有 `tracing`
（target `plugin`），同时交给 `log_file::PluginLogStore`：一个专用写线程按插件追加到
`<exe 目录>/logs/plugins/<插件 id>.log`，单文件超过 8 MiB 时轮转为 `<插件 id>.log.1` 并保留一代。
写入只把行放入有界队列（4096 行）后立即返回，磁盘变慢不会阻塞插件调用，队列满时丢弃的行数会在
文件里补一条 `WARN`。日志目录由当前可执行文件路径确定，桌面和 CLI 使用相同规则，不随工作目录变化；目录不可写时明确告警，
不回退到 AppData。文件不放进插件目录，避免改变插件目录的 SHA-256 指纹（`fingerprint.rs`）。
退出时先关闭插件并收完 stderr，再在阻塞工作线程关闭日志队列、等待写线程排空。单条消息最多保留 64 KiB UTF-8 内容，
超长部分带截断标记；特殊或超长插件 id 的文件名附加摘要以免冲突。加载器的 stderr 行带
`ERROR` / `WARN` / `INFO` / `DEBUG` 前缀，落盘时据此还原级别，无前缀的加载器诊断记为 `HOST`。

插件**发现即信任**：清单合法的插件包初始即为可用（`Configured`），可直接探测、派发与分析；
没有批准步骤与禁用开关，插件目录 SHA-256 指纹仅用于设置页展示与版本标识（已加载的库不会卸载）。
加载失败的插件包连同原因列在 `plugin_list` 的 `rejected` 中。插件列表、重启、安装与卸载由
`commands::extensions::plugin` 提供，桌面设置页与 CLI 的插件命令调用它们。协议、安全边界与限制见
[插件协议](PLUGIN_PROTOCOL.md)。

## 自动识别平台

`core/analysis/identify/autodetect` 负责判定检材所属平台、证据类型与备份特征，再回写 evidence。
证据首次上线时由宿主建立内部首任务，手动重新识别通过 `autodetect_evidence` 命令执行。两条路径都登记为
`TaskManager` 的 `autoDetect` 任务（id `autodetect:<image id>`，同时进行的两次运行共用一份登记与取消令牌）：
识别在分区、逻辑卷与归档目录之间检查取消，镜像卸载或 `task_cancel` 后不再回写；登记释放之前把结论（平台、
厂商显示名、检材类型、是否备份）与结局报给登记，任务中心据此列出，同时广播 `autodetect-finished`（报告）或
`autodetect-failed`（`{image_id, error}`，`error` 与命令错误同形，取消为 `cancelled` 码）供镜像信息页使用。

```text
core/analysis/identify/autodetect.rs          数据模型（EvidencePlatform 词表 android/ios/windows/macos/linux/harmony/
│                           huaweihmos/othermobile/drone、DetectRoute、PartitionPlatform、AutoDetectReport）、
│                           三路分发器、分区/LVM 逻辑卷遍历、归档条目走查、聚合
├── fstype.rs               磁盘镜像路：文件系统类型门限、标志文件组全匹配与
│                           四平台复核（macOS→Windows→Linux→Android 短路）
└── backup.rs               归档/备份文件路：detectTar/detectZip 特征文件表、跳过扩展名表、厂商/大类
                            关键词、adb `ANDROID BACKUP` 魔数（含小米 .bak 前缀）、huaweicheck.json
                            品牌白名单、备份类型到平台与厂商的映射、显示名称
app/workflows/identify.rs      `autodetect_evidence` 命令 + 证据首次上线的后台首任务 + `autodetect-finished` /
                            `autodetect-failed`
```

三路分发：磁盘镜像路按会话的分区视图逐分区（已解锁的 LUKS、FileVault 与 VeraCrypt 分区与文件树一样取明文文件系统的
类型，其中的 LVM 经明文视图展开到逻辑卷；锁定的 BitLocker 单列，解锁后按 NTFS 判定）先按文件系统类型
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
iTunes 清单）与关键词匹配；合成根已判出非手机平台（Linux、DJI 无人机等）时检材就是该系统，`var/backups`、
AOSP 的 `SharedStorageBackup` 这类备份线索不再改判，只记入告警；`.ab`/小米 `.bak` 由保留的原始文件头魔数直接判备份，
独立打开时先建立逻辑文件树，加密备份明确拒绝。手机镜像路（小米固件容器、雷电备份）分区扫描后兜底 Android。

DJI 无人机由共享的 `identify/dji.rs` 在通用 Linux / Android 标志检查前识别；`os_detect` 与自动识别使用同一规则。每个可读根只检查 `/build.prop` 或 `/system/build.prop`（完整文件至多 64 KiB），要求 `ro.product.manufacturer=DJI`、非空 `dji.build.version`，并且同一根的 `bin/dji_flight` 与 `bin/dji_navigation` 都是文件；冲突属性、目录冒充服务或不完整特征不命中。结论为 `evidence.platform=drone`、厂商 `dji`，OS / 系统发现的 family 为 `dji`；固件版本、设备代号、原始 model、ABI 与 Android 基础版本按属性原值返回，不映射猜测商品型号。私有检材仅通过 `DJI_ORACLE_IMAGE` 和 `just test dji_external_image_detection --lib -- --ignored` 验证，不加入仓库。

回写：有活动案件且检材已登记时，`evidence.platform` ← 平台词（未命中时保持原值）、`evidence.kind` ← `disk` /
`compressfile` / 备份枚举词、`evidence.detection` ← 完整报告 JSON（厂商、显示名、备份特征、告警与逐分区判定），
任务照常结束。

手动指定：`case_evidence_platform_set`（CLI `images platform set <平台|auto>`）把检验人选择的平台写入台账
`setting` 表的 `evidence.platform.<uid>` 键，与审计动作 `evidence.platform` 同一事务提交；`auto` 删除该键，恢复
自动结论。自动识别只写 `evidence.platform` 列，重新识别不覆盖手动选择。读检材时两者一并带出：`EvidenceRef.platform`
是生效的平台（手动优先，其次自动识别），`manual_platform` 是手动选择，案件管理器与镜像信息页据此展示。要求检材已
登记在活动案件中，案件锁定时拒绝；移除检材时一并删除该键。

桌面应用的 `PlatformStore`（`evidence/platform.rs`）接收 `autodetect-finished` / `autodetect-failed` 事件，按镜像 id 保存本会话的识别报告；检材头部显示结论（平台、厂商与检材类型），支持重新识别，检材在活动案件中时可经 `case_evidence_platform_set` 手动指定平台。逐分区判定、标志文件明细、备份特征与告警不在界面展开。每次识别在任务中心列为一条任务。

识别限制：macOS 两标志均缺失时不按卷名猜测；部分厂商备份归入 `backup` 大类；
iOS 整盘镜像没有独立判定路径；归档与目录检材的合成根没有文件系统类型，不做 Windows 与 macOS 检查；Btrfs 子卷
（如 `@`）和逻辑检材子目录里的系统不向下探测。这些情况可手动指定平台。复核结论时查看案件回写的
`evidence.detection`（逐分区判定、标志文件、备份特征与告警）。

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
    records.db               插件分析运行与类型化记录（core::analysis::results::record_store）
    analysis.db              旧版本的插件结果，只读（core::analysis::results::legacy_store）
    timeline.db              时间线投影（core::timeline::timeline_store）
    search.db                全局搜索任务与命中（core::search::global_search_store）
    cache/                   可随时删除的缓存
      docker/                Docker 容器来源映射
      zip-backups.zst        ZIP 检材内小米 / Android 备份的展开清单
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
  upsert，`records.db` 由插件运行写（暂存运行的记录按批写入，发布与替换各是一个 IMMEDIATE 事务；
  记录按记录类型存在 `rec_<类型>` 类型化表中，结构随结果保存），`timeline.db` 由重建任务与插件发布投影写，
  `search.db` 只由运行中的搜索写。不同检材的写入互不排队。旧版本写下的 `analysis.db` 只读，不迁移。
- 最长的写入——把百万行快照导入文件系统索引——只写 `staging/` 里一个还没有人读的私有文件，写完
  整体发布，发布后只读（`immutable=1` 打开）。导入再久也不占任何共享库的写锁，导入期间浏览、搜索、
  时间线照常读旧索引。
- 读路径（列表、分页、状态查询、导出）从不建库、不跑 DDL、不做无条件写：库不存在就按空结果返回；
  孤儿 `running` 行的修复只短等写锁，等不到就只在本次结果里按中断呈现。

**台账（`core::casework::case_ledger`）。** `application_id = 0x4C434344`（"LCCD"）标识这是 LovelyCarve 台账，
`user_version` 是结构版本，不是当前版本的台账拒绝打开。表：`case_profile`（唯一一行：案件 uid、编号、
名称、检验人、描述、逻辑锁、创建/更新时间）、`setting`（案件级 KV；检验人手动指定的检材平台也存在这里，
键为 `evidence.platform.<uid>`，结构版本因此不变）、`evidence`（`uid` 唯一、
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
`case_lock`/`case_unlock` 翻转逻辑锁（锁定后拒绝改名、移除检材、手动指定平台与重建快照）；`case_remove` 只删除案件自己的文件
（台账、锁、`evidence/`、`staging/`），目录里的其它内容原样保留，目录空了才删除目录本身；
`case_list` 读应用数据目录下的本地登记簿（`core::casework::case_registry`）并附台账档案；
`case_option_query`/`case_option_save` 读写 `setting` 表。`CaseState` 是台账的读缓存（检材与卷记录）
加上索引不可用时回退用的内存快照体，台账本身是唯一事实源。

**检材。** `image_load` 与案件打开时把已加载镜像登记为检材（以 image id 幂等；来源已是另一个检材时
拒绝加载）。首次登记触发自动识别首任务与 `trigger = auto` 的依赖链根任务。移除检材先取消它的全部
未终结任务，再在一个事务里删除检材行（级联）与它的手动平台设置并记 `evidence.remove`，最后在锁外删除整个检材目录；
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

**检材库（`core::casework::evidence_db`）。** 五个检材库共用一套连接策略：写者（`open_writer`）必要时建检材
目录与库文件，新库在一个 `BEGIN IMMEDIATE` 事务里一次建全结构并盖上各自的 `application_id`
（facts "LCFX"、records "LCRC"、timeline "LCTL"、search "LCSR"；旧版只读的 analysis 库为 "LCAN"）与 `user_version`；读者
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
与命中源 MD5/SHA256 回填；关键词走 Aho-Corasick（`core::search::keyword::keyword_search`，编码页可选），
各编码字节变体在编译时合并到原样与纯 ASCII 忽略大小写两台自动机（`keyword_literals`），相同字节只保留
最高优先级编码的来源。纯 ASCII 词直接使用自动机的 ASCII 大小写等价表，不复制并折叠扫描缓冲区；非 ASCII
词按原样及整词大小写变体精确匹配，避免折叠 GBK 尾字节或 UTF-16 高字节。单通道的模式已在编译时去重，
命中无需再逐条哈希；双通道仍按编码优先级对区间去重，重叠命中与跨块 carry 规则不变。正则走
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
卷键），`plugin_event` 投影检材 `records.db` 中已发布运行的时间点（记录里带时区偏移、声明了时间线角色的
时间戳字段；`tid` = run_id，`from_app` = 插件 id，`tl_flag` = 角色，`content` = 记录标题）以及旧案件
`analysis.db` 的插件时间线事件（`tl_flag` = event_type），`ntfs_usn` 投影 NTFS 变更日志；缺时间戳或
时区未知的本地时间跳过。两条投影路径为：`timeline_start` 创建重建任务（`TaskKind::TimelineBuild`）全量
重投影（先清空，幂等），插件运行发布时（`project_published_run`）删掉该运行与被它替换的运行的投影再投影
新时间点——写路径自动投影。`timeline_status`
状态机 `buildRequired`/`running`/`stale`/`ready` 以源水位（投影规则版本 + 各已索引卷的「卷键@来源快照
时间」、插件运行发布时间与旧版运行完成时间的最大值）判定 staleness，水位在重建开始前取，重建期间新到的数据让结果判
为过期而不是被悄悄漏掉；查询面 `timeline_count`（strftime 动态时间桶 + 时区参数化 + 桶分页）/
`timeline_datas`（`tl_time desc` 明细分页）/`timeline_types`（`group by from_app,type` 两级维度）/
`timeline_range`/`timeline_export` 都带 `imageId`，共享参数族（start/end/time_zone/time_unit/
from_apps/data_types/pairs/text/show_delete），两级勾选走 `(from_app, type)` 行值 IN 精确过滤。`timeline_types` 与 `timeline_range` 目前只由 CLI 使用。mactime body-file 导出模块为 `core::timeline::mactime`（命令
`fs_timeline_export`）。

## 元数据索引层（filedb）

`filedb_query_files` 的结构化条件包含 `nameGlob`：在预存的 `name_low` 上使用绑定参数的 SQLite `GLOB`，模式与现有名称条件一样先转小写，按完整文件名匹配 `*`、`?` 和字符组。它与路径、扩展名、类型等条件一起放在 SQL `WHERE` 中，再排序及 `LIMIT/OFFSET`，不会取页后才丢弃不匹配项。每个卷分别分页，目录范围仍由 `pathPrefix` 表示，不把文件名模式解释为路径 glob。

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
live 遍历。`filedb_volumes` / `filedb_query_files` / `filedb_resolve_meta_addr` 由 CLI 的 `indexes volumes list`、
`indexes files list` 与 `indexes files lookup` 调用；桌面文件列表经
`fs_children_page` / `fs_flat_page` 读取同一份索引。

浏览命令的数据源（`commands::filesystem::filesystem_snapshot_index`）：开着案件时是台账里该检材源卷大小与已加载
镜像一致的卷索引，没有案件时是会话索引（无案件时快照导入进程临时目录里的同格式索引文件，随会话快照
释放而删除；完整 body 仅作为索引失败时的回退）。`fs_children_page`、完整 `fs_children`、
`fs_child_counts`、`fs_subtree_empty_dirs` 与 `fs_flat_page` 优先读索引，miss 后再回退到内存快照体或
live listing。`fs_flat_page` 只使用非截断快照的索引，仍会对索引 / 快照未下探的 deleted 目录拼接 live
恢复遍历。删除单卷快照（`snapshot_drop`）删除卷记录与整个卷目录。

**文件全显（allowAllFiles）。** 列表请求与逐级浏览同构，只多
一个布尔 `allowAllFiles`；置位后起点 `parentId` 从「列一层」变成「以该
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
在 `AppState::flat_walk_cache`（FIFO 2 条），调用方的「刷新」传 `refresh` 作废。`fs_flat_ext_counts` 给出
起点子树的扩展名直方图，只供 CLI 使用。全显请求不进入后台任务队列（SQL 路径以整卷为起点时在百万行卷上
为亚秒级，以目录为起点时随子树规模增长；实时遍历只在无索引无快照时发生并被缓存）。桌面文件列表（`files/table.rs`）随滚动分页读取，排序与筛选由后端执行。

**空间占用树（`fs_size_tree`）。** 空间占用树要的是每个目录其下的合计，目录浏览只知道直接子项，所以由
`core::catalog::size_tree` 把一个卷索引整表读一遍（不读 `node_json`），建成只存数字的紧凑树：每行一个
32 字节节点（`ordinal`、计费大小、修改时间、类型与删除标记），子项为压缩邻接表，每个目录一份
「仅存活 / 含已删除」两套合计（字节、文件数、目录数、最新修改时间），百万行约 50 MB。目录行先编号再按
`pid` 挂接，不依赖行序；从卷根后序汇总，挂不上卷根的行不计入并记 warn。大小是逻辑大小，与列表一致：
同一 MFT 记录（含命名流的流名哈希）在多个目录出现时只计首个名称；稀疏且等于卷长的 `$BadClus:$Bad`
（记录 8 的命名流）不计入。命令先在案件或会话的索引目标里找出已遍历该目录的那一个，按 `ntfs:` 节点或
卷根下有 NTFS 子项判定是否 NTFS（其它返回 `unsupported`，索引未覆盖返回 `unindexed`），再从
`AppState::size_trees` 取树：按 `(检材, 索引文件)` 键、最近使用的 2 棵、`OnceCell` 单次构建，重建快照发布
新索引文件即换键，卸载检材时清除。一次视图从目录出发按层广度优先展开 `depth` 层：每个目录只挑出度量最大的
至多 96 个子项排序，小于目录合计 `minShare` 的条目与超出部分并入 `others`；下层目录只有全部子项放得下（总数
6000）才展开。最后按 `ordinal` 批量回表读取要显示的节点。
该命令目前只由 CLI 调用，桌面应用没有树图视图。

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

## 分页命令

大规模目录、Journal 和案件派生数据不应新增全量返回接口。新增分页接口统一使用
`Page<T>` 形态：`items`、`nextCursor`、可选 `total` 和 `truncated`。游标应基于稳定排序键，而不是裸
offset。当前普通目录已提供 `fs_children_page`，案件模式下会优先使用 SQLite 快照索引返回单页，
索引未覆盖时沿用 `fs_children` 的案件快照 / session 快照优先路径；
文件列表通过 `fs_children_page` 的可选 `query` 参数发送元数据过滤和排序，采用目录优先、排序键、节点 ID 的稳定游标；SQLite 索引只取单页，实时来源在后端过滤排序。
未传 `query` 的兼容调用仍按 `(name,id)` 分页。哈希/标签等派生列、路径和隐藏空目录筛选通过有界分页取得完整目录后沿用客户端语义；普通滚动按需补页，定位目标会补齐到目标页。
目录计数只对可见行查询，未索引目录通过 `pageSize:1` 获取总数，不再传输其完整子列表。无快照的实时来源仍可能需要枚举整目录以计数和排序。NTFS USN Journal
提供 `fs_usn_journal_page`，ext4 jbd2 历史 block revisions 提供 `fs_ext4_journal_blocks_page`。旧
`fs_children`、`fs_usn_journal` 和 `fs_ext4_journal` 仅作为兼容入口保留，新的 UI 或
批量调用应优先使用分页接口。案件派生数据虽然仍保留 `fs_digest_load`、`fs_ftype_load`、`fs_tags_load`
全量兼容入口，但新调用方应优先使用 `*_load_nodes` 按当前节点集合水合，避免一次目录切换拉取整镜像
摘要、类型识别或大型标签表；标签编辑应优先使用 `fs_tags_apply_changes` 增量写入，不应为了清除一个
标签而提交不完整的整镜像标签 map。

后端事件：

- `task-entry`
- `task-lifecycle`
- `task-progress`
- `image-load-progress`
- `raid-detect-progress`（阶段与当前阶段的对象计数，总量未知时为 null；不是全程百分比）
- `image-hash-progress`
- `image-export-progress`
- `fs-export-dir-progress`
- `bitlocker-unlock-progress`
- `bitlocker-scan-progress`
- `snapshot-build-progress`
- `autodetect-finished` / `autodetect-failed`
- `global-search-progress`
- `timeline-progress`
- `carve-scan-progress`
- `qr-scan-progress`

桌面应用经 `events::EventHub` 按事件名订阅；CLI 的 `--events` 输出同一份事件。

## 时间线

- 重建同时投影 filedb 文件修改时间、插件事件，以及已加载镜像各 NTFS 卷的 `$Extend\$UsnJrnl:$J`；USN 以独立类型 `3`（NTFS 变更日志）参与筛选、计数、分页与导出。日志读取不依赖文件目录索引，因此目录索引未完成也可生成日志时间线。旧源水位升级为 `usn-v1:` 前缀，已有成功结果会提示重建一次。
- `volumes/ntfs_journal.rs` 复用现有 VMDK/NTFS 只读数据源，按数据运行列表跳过稀疏前缀；连接属性走原 NTFS 流式读取路径。固定大小缓冲区处理分块和页间零填充，时间线不受日志分页读取的 64 MiB 上限限制。取消在每个数据块检查，已投影批次保留并标为取消；读取失败向任务中心传播。
- `ntfs-core`（Apache-2.0）负责 V2/V3 二进制记录解析，本地适配层验证记录边界、保留 FILETIME 原始精度并拒绝缺失时间。V4 仅含区间信息、没有时间戳，不伪造时间线事件。原始解析结果保存在时间线 `extra` JSON 中，行时间仍为 Unix 秒。
- USN 行使用卷键与 USN 唯一标识历史事件，保留事件文件名、MFT 与父目录编号；不把可能复用的 MFT 编号直接跳转为现存文件，不声称重建了完整历史路径。删除日志是有效事件，默认“排除已删除文件”筛选不会隐藏该事件；该筛选继续作用于 filedb 删除文件行。

- 桌面时间线页（`timeline.rs`）显示当前检材的事件：按年、月、日的事件计数图（`timeline_count`）与分页事件列表（`timeline_datas`），未创建时提供创建入口；CSV 导出（`timeline_export`）沿用已应用的查询条件。

## 文件恢复与雕刻

全局搜索、时间线、文件恢复和分析结果的初始页共用 `ui::workspace_start`：工具栏下方居中显示图标、标题、简短说明与主按钮。有记录的页面由 `ui::result_workspace` 组织记录侧栏与工作区，选中项的详情在右侧展开（`ui::detail_frame`），区域较窄时浮在表格上方。

恢复页（`crates/desktop/src/carving.rs`）通过 `carve_scan_list` 列出当前检材在本进程中保留的会话（沿用最近三个会话的回收策略，活动扫描不会回收），切换记录时隔离迟到的结果请求；`carve_scan_remove` 仅删除当前检材已结束扫描的内存记录，不删除已恢复文件，也不清除任务中心的执行历史。重启应用后不会保留这些内存记录。

一次扫描是一个 `CarveScanSession`，按 `phase` 依次运行两个可选阶段：`filesystems` 选中任一文件系统族时先做文件系统恢复（见下文），`signatures` 为真时再做签名雕刻。两个阶段共用任务 ID、取消令牌、资源许可与暂停状态（`carve_session::wait_while_paused`）；快照的 `filesystem` 摘要报告阶段进度、卷/目录/文件数、截断与读取故障，阶段失败时记录错误，后面仍有雕刻阶段则继续，否则整个会话失败。扫描范围由 `carve_start` 的 `scope` 解析为字节区域（整盘、分区、分区间隙或 `range` 指定的扇区段），文件系统阶段使用其中的整扇区（`carve_filesystem::sector_ranges`）。

新建扫描表单（`carving/create.rs`）选择文件系统记录恢复与签名雕刻：雕刻的文件类型按类别分组，并可限定扫描范围。运行时显示阶段进度、暂停/继续和取消。签名雕刻结果按 `sequence` 游标经 `carve_results_page` 分页读取（`carving/results.rs`），文件系统记录结果在阶段结束后经 `carve_filesystem_results` 读取，按记录保存的路径列出（`carving/filesystem.rs`）；可将全部或所选结果恢复到主机目录。

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
暂停状态；桌面恢复页通过 `carve-scan-progress` 增量同步，以 `sequence` 游标分页。恢复按
`f{绝对块号}.{扩展名}` 命名，流式计算 SHA-256 并输出 DFXML。代码版权与许可证见
[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。

`probe_carve` 示例可导出全部结果（`--dump`）并指定线程数（`--threads`）；镜像带有完好的 ext4 文件系统
时，`probe_carve_truth` 列出每个文件的首个物理偏移与大小，二者按偏移对照即可统计各格式的召回率与长度
准确率。

### 文件系统恢复阶段

`core/filesystem/recovery` 负责只读的 FAT/NTFS/exFAT 引导发现、卷目录或 MFT 枚举和逻辑流恢复。`discovery` 在每个扫描扇区段内报告起始扇区落在段内的已知分区，并按扇区查找引导扇区与残留 `$MFT`；遗失卷使用独立的 `Partition` 几何传入既有读取器，不修改源镜像的分区 ID 或分区表。候选身份由扫描和卷索引限定，结果以 `Arc<RecoveryScan>` 挂在会话的 `FilesystemPhase` 上，随会话回收或卸载检材时的 `purge_image_scans` 一并释放。阶段在 `collect_read_faults` 范围内运行，`RecoveryScan.read_faults` 记录本阶段的坏区。

引导扫描阶段按字节、目录/MFT 阶段按条目报告 `task-progress` 与快照进度。扫描错误和目录读取失败保留在卷结果中，达到卷/候选/深度/节点上限标记截断。FAT/exFAT 恢复枚举使用严格的删除元数据入口，普通浏览沿用容错入口。

恢复工作流还按 NTFS 选项扫描残留 `$MFT`。`volumes/ntfs/recovery` 用分配字节数/完整 run 数推导簇大小，用首 run LCN 和物理记录位置推导卷基址，并验证 `$MFTMirr`、根目录与 USA fixup。BootstrapReader 仅在 `Ntfs::new` 初始化时提供推导的 boot；随后所有数据（含 `$Boot`）来自真实来源。`RecoveryVolume.reconstruction` 明确记录推导过程和来源位置，不改变原镜像的分区表。

`recovery/recycle` 用经过边界验证的回收站 `$I` v1/v2 内容配对同父目录 `$R` 项，恢复原名和路径；`source_node/source_path` 保留未投影的节点。父目录无法按当前身份确认时建立独立的虚拟目录，避免把不同时代的同名删除目录合并。

`carve_recover` 在同一任务中先恢复所选雕刻结果（直接写入目标目录并生成 DFXML），再按 `file_ids` 恢复文件系统候选。候选编号必须属于该会话的文件系统结果且具有可读映射，否则在登记任务前拒绝；不能传入任意原始数据映射。文件系统输出写入新建的 `recovery-<UUID>` 子目录，`recovery/layout` 按浏览树的规则还原目录：每卷一个 `v<卷号>-<卷标>` 文件夹，目录身份先按记录、缺少父记录时才按路径补齐，环和过深的父链挂到卷文件夹下；每个组件经 `sanitize_export_component` 处理，同一文件夹内忽略大小写去重并加 `-n` 后缀。输出采用独占创建，逐块检查取消和逻辑长度，并在失败时先关闭再移除未完成文件及其留空的文件夹。报告为 JSONL，逐文件记录来源、原路径、相对输出路径、SHA-256、错误和操作范围的 `ReadFaultReport`。完整读取成功不能证明删除文件未被覆盖。

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
删除记录视图会单独读取它。sidecar 在调用方传入的父目录节点下查找，所以 `ntfs:`、`ext4:`、
`folder:` 这类不含路径的节点 ID 也能找到。



`core/preview/sqlite/sqlite_recovery.rs` 接入 vendored `sqlite-core`（`vendor/sqlite-core`，来自
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
SQLite 预览与删除记录目前经 CLI 的对应命令读取；桌面应用没有结构化 SQLite 视图，原类型视图改用 Outside In 查看器。

## 命令行与桌面只读挂载

独立命令行入口为 `crates/cli/src/main.rs`，用法见 [`CLI.md`](CLI.md)。
CLI 不链接任何窗口或 GUI 工具包；进程在 16 MiB 栈的专用线程上运行 tokio 多线程运行时，
自行创建 `AppState` 并调用 `embed::initialize`，退出时经 `embed::shutdown` 取消任务、
关闭案件、停止插件并释放媒体缓存。Windows 的 `asInvoker` 清单由构建脚本在链接时嵌入。

命令目录由构建脚本生成：`crates/cli/build/cli_catalog.rs` 读取 `crates/backend/src/lib.rs` 的 `command_registry!` 注册表和各命令的
真实函数签名，与 `crates/cli/src/commands.toml` 中逐条审阅的命令定义合并，生成按 API 参数名调用命令函数的
`cli_dispatch.rs` 和命令目录 `cli_catalog.json`（命令、参数、用法、类别、并发、敏感字段、隐藏标记与示例，以及结果契约：
后端返回 `()` 的命令记为 `unitResult`，只有它们以 `✓` 代替输出；`missing` 把返回 `Option` 的查找的 `None` 变成
`notFound`；`incomplete` 列出结果不完整的字段）。构建时检查：每个注册命令映射到恰好一个 `<集合>… <动词>` 命令，或在
`[[superseded]]` 中注明替代命令；动词在固定动词表内，不使用同义词（info/status→get、remove→delete、
ls/query→list 等）；命令路径唯一且不是其他命令的前缀；`list`/`count` 不带位置参数；选项名与短选项
不与 CLI 保留选项冲突；示例以程序名开头；`missing` 只用于返回 `Option` 的后端命令，原始字节输出不带 `incomplete`；
命令签名中出现不支持的注入参数时构建失败。手写命令（`files list/get/read/search`、
`files strings list`、`context`、`tasks wait`、`system get`、`schema` 等）也在该文件中声明参数，
`schema`、帮助、补全、JSONL 会话和代理技能的命令参考读取同一份目录。`hide_command = true` 的命令
（如 `sessions serve`）可以运行，但不进入帮助、补全、拼写建议和命令参考。

代理技能 `skills/lovelycarve-cli` 随 CLI 安装包发布：`SKILL.md` 与 `references/workflows.md` 手写，
`crates/cli/src/skill_tests.rs` 解析其中的命令行和 JSONL 请求；`references/commands.md` 由同一测试模块从命令目录渲染，
`just update-cli-skill` 运行其中被忽略的写入测试重新生成，文件与目录不一致时测试失败。

`crates/cli/src/` 按职责分为：`argv`（由目录构建 clap 命令树并解析参数、命令行错误）、`help`（yargs 版式的帮助：
用法、说明、命令／位置参数／全局选项／选项／示例表格与右对齐的类型标注）、`context`（一次执行的统一步骤：
dry-run、删除确认、默认值与生成 ID、提问、`--image` 作用域与来源提示、镜像内路径解析、不覆盖检查、调用和
上下文更新）、`native` / `browsing` / `grep` / `strings`（手写命令；`native::offline` 处理无需后端的发现命令）、
`output` / `prompt` / `progress` / `interactive`（标准输出与标准错误、clack 式提问与进度、终端与代理检测）、
`schema`、`complete`、`dotenv`、`session`、
`shell`、`resident`（命名常驻会话的启动、连接、私有传输及串行执行）、`background`（后台任务的结局）、
`incomplete`（按目录判断结果是否完整）和 `runtime`（进程入口、后端启动与关闭、中断处理）。
`background` 跟随任务中心的 `task-entry` 事件：命令返回后仍在运行的任务、命令成功而它启动的任务却失败，以及
`automatic` 任务算作后台工作；`tasks wait`、一次性命令和 shell／JSONL 退出前的等待、`sessions close` 报告它们的结局，
失败或没有报告结果即以 `tasksFailed` 失败。只跟随在本进程中见到运行的任务，插件任务的 `offline` 是过渡状态。结构化结果以 `Json` 类型保存后端序列化的 JSON
文本，打印和会话响应保留字段顺序，需要读取时才解析为值；后端命令的错误带上 IPC 名称，渲染为「后端错误」块。
可执行文件旁的 `plugins/` 同样参与插件发现，`lc-plugin-host` 也从可执行文件旁查找。
`resident` 的后台进程持有一个 `AppState` 和可变 `Context`；普通命令通过 `--session` 或
`LOVELYCARVE_SESSION` 连接，无需重新打开检材或组装卷。每个名称用用户数据目录中的文件锁保证单一拥有者，
随机凭据只保存在私有连接记录中，服务只监听回环地址；客户端不凭 PID 识别进程、不自动重试请求。
请求串行执行，任务在后台继续；原始字节按块传输，`files read` 固定本次请求的检材及节点后分段读取，
短读或连接中断必须报错。`sessions attach` 使用普通命令语法，断开不关闭后台；`sessions close`
完成共享关闭流程再退出。桌面状态不参与这条连接链路。
全局 `--events` 在普通命令、shell 和 JSONL 中接收同一份后端事件。普通命令的 `Reporter` 将
实际阶段/计数映射到 stderr 的进度行，重定向 stdout 不会关闭它；每 5 秒的心跳只声明进程仍在执行。
`Monitor` 以 RAII 管理刷新线程及 OSC 状态，取消异步 future 时也会清理。命名会话在每次非字节流请求期间
通过私有传输的事件帧转发后端进度，事件与最终结果使用同一连接并按帧分离；文件字节流由客户端持续显示命令耗时。
`just test-cli` 以真实进程验证无窗口启动、会话协议、脚本与中断退出码，以及常驻会话跨进程状态、
组装卷复用、大文件字节与文本传输、会话隔离、认证、案件释放和崩溃后的同名重启。

桌面只读挂载由 `app/workflows/mount.rs` 编排，`commands/system/mount.rs` 提供命令，
`core/platform/dokan_mount/` 负责 Windows Dokan 投影。挂载使用 evidence session，
范围读取复用文件系统适配器；共享、句柄和挂载点在卸载证据及关闭应用时清理。
CLI 的 `mounts create/list/delete/share/unshare` 调用同一组命令，进程退出时释放挂载与共享。

## 桌面应用

桌面应用 `lovelycarve-desktop`（可执行文件 `LovelyCarve`）用 GPUI Kit（GPUI、GPUI Component 及其图标资源）构建，页面是 GPUI 实体，控件优先取自 `gpui_kit::component`。

```text
crates/desktop/src/
├── main.rs                 入口：原类型查看器子进程分派、后端启动、GPUI 应用与主窗口
├── backend.rs              进程内后端：AppState、宿主句柄、tokio 运行时与事件通道；Backend::run
├── events.rs               EventHub：在界面线程重新发出后端事件
├── workspace.rs + workspace/  主窗口：标题栏、检材标签页、欢迎页与添加检材
├── cases.rs + cases/       活动案件与本地案件登记簿；案件管理对话框
├── tasks.rs + tasks/       任务中心数据（task_list + task-entry）；标题栏任务面板与快速分析
├── evidence.rs + evidence/ 一个检材标签页：加载、导航栏、镜像信息、卷与分区、解锁、平台识别、iOS 备份
├── files.rs + files/       文件列表：目录树、分页列表、筛选条、详情（元数据、文本、十六进制、源码、原类型视图）与导出
├── search.rs + search/     全局搜索
├── timeline.rs + timeline/ 时间线
├── carving.rs + carving/   文件恢复
├── analysis.rs + analysis/ 分析结果与快速分析
├── bcp.rs + bcp/           BCP 数据
├── raid.rs + raid/         RAID 检测与手动重组
├── settings.rs + prefs.rs  设置页（外观、插件、版本）与界面偏好
├── theme.rs                浅色 / 深色主题 token
├── ui.rs                   共享展示组件：工具栏、页脚、空态、详情框、字段行
├── motion.rs               界面动效：页面、面板与列表行的入场
├── assets.rs + ../assets/  图标目录与品牌图片、Quicksand 字体、图标源图
└── format.rs               展示格式化
```

后端以 GPUI global `Backend` 保存。`Backend::run(|host, state| …)` 把命令函数放到后端的 tokio 运行时上执行，返回任意执行器都能等待的 future；页面在 `cx.spawn` 中等待结果，从不阻塞界面线程，命令错误转为带取消标记的 `CommandError`。`AppHandle` 的事件进入无界通道，由 `EventHub` 实体在界面线程重新发出，模型按事件名订阅自己的事件。跨页面的状态由 GPUI global 与实体持有：`CaseStore`（活动案件与登记簿）、`TaskStore`（任务中心）、`RaidStore`（会话级 RAID 检测，成员跨检材）、`PlatformStore`（各检材的平台识别报告）和 `Preferences`。GPUI 应用退出后，`Backend::shutdown` 以 15 秒为限执行 `embed::shutdown`。

启动参数：`LovelyCarve [--case <案件目录>] [检材路径…]`。`--case` 打开（不存在时创建）案件，其余参数作为检材加入。开发时用 `just dev -- --case <目录> <检材>`。

主窗口由标题栏和下方的当前检材标签页（或欢迎页）组成。标题栏放检材标签页，以及案件、任务中心和设置入口；检材始终属于活动案件，打开或切换案件会用该案件的检材替换标签页。检材标签页由导航栏和页面组成，页面在首次显示时创建，并在标签页存续期间保留状态。工作页依次是工具栏（左侧搜索与筛选，右侧页面操作）、工作区和页脚（页面上下文与计数），共用 `ui.rs` 的组件；配色全部取自 `theme.rs` 的当前主题，界面必须在浅色和深色下可用。图标是 GPUI Kit 附带的 Lucide 图标，用到的每个图标都要登记在 `assets.rs` 的 `icon_assets!` 中（GPUI Kit 默认组件图标除外），否则编译通过但显示为空白，并在每一帧记录 `could not find asset`；`assets.rs` 的测试扫描界面源码，图标用到却未登记、或登记了却没用到都会失败。文件列表的文件类型、文件夹与展开箭头图标来自 `assets/file_icons/`（来源与许可证见该目录的 README 和 `THIRD_PARTY_NOTICES.md`），以 `lovelycarve/file_icons/<名称>.svg` 提供。界面偏好保存在 `<应用数据>/gpui-preferences.json`，读不到或损坏时回退为默认值。

界面动效由 `motion.rs` 统一提供：页面、面板与列表行进入时淡入并移动几像素到位，列表行依次错开；时长与曲线取自 GPUI Kit 的动效 token（`motion_tokens()`），与组件库自带的对话框、通知和标签页动画节奏一致。页面在切换时以页面为键重新播放入场，导航栏的类别高亮用弹簧滑到当前类别，页面列表随侧栏开关在裁剪框内滑开、收起（列表本身保持全宽，不随动画重排），任务中心面板下落出现、淡出后移除，文件详情等检查器在窄窗口以浮层从右缘滑入、停靠时只让内容滑入。动画都经 GPUI 的动画元素或 GPUI Kit Base 的 `motion`，系统要求减少动画时（Windows「辅助功能 → 视觉效果 → 动画效果」关闭）直接停在最终状态；欢迎页插画的循环另受「首页动画」开关和窗口激活状态控制。动画跟随显示器刷新率推进，不用 `Animation::with_max_fps` 限帧：它在 Windows 上由线程池计时器唤醒，15.6 ms 的计时器刻度会让帧间隔在两三个刻度间跳动。GPUI 的每个动画帧都重绘整窗，因此开发构建在根 `Cargo.toml` 中优化 GPUI、GPUI Kit 组件库和 taffy 布局等依赖。

任务中心（`tasks/center.rs`）位于标题栏，跟随当前检材：`TaskStore` 先由 `task_list` 取得全部任务，再按 `task-entry` 事件增量合并，并把调度器通知转成界面通知；不从领域事件或命令返回值推断内置任务的结局。快速分析在同一面板中，对当前检材或其识别出的系统运行插件分析器，作业交给插件调度器执行。

文件列表的目录树以分区为根，目录首次展开时列出子目录（`files/tree.rs`，行的绘制在 `files/tree/row.rs`）。非分区目录只有一个条目且是子目录时，展开后由子目录接管这一行，标签是整条链（`a/b/c`），每段名称可单独打开；判断时只再读该目录的前两个条目。滚动位置决定首个可见行，它的上级行固定在列表上方，直到其子树滚出这一行的位置。文件图标由 `files/icons/file_types.rs` 的对应表决定：先查完整文件名，再从最长的后缀依次查到最短，最后查第一个点之前的部分，均不区分大小写；对应表之外、源码预览能高亮的文本用源码图标，其余用通用文件图标，分区、ADS、快照、未分配空间等按节点 id 使用各自的 Lucide 图标。列表按当前目录或全部子目录分页读取，排序与筛选由后端执行，行可勾选做批量操作并显示检验人员的标签。详情面板显示所选节点的元数据、解码文本、十六进制（每行 16 字节，按 64 KiB 分块随滚动读取）与源码预览（GPUI Kit 编辑器）。导出把文件写到所选路径，目录可导出为文件夹树或一个 ZIP，进度显示在任务中心，来自检材的名称都经 `embed::sanitize_export_name` 清理。

Windows x64 的原类型视图嵌入 Oracle Outside In Viewer 8.5.7 控件。查看器运行在独立进程中：应用以子进程方式重新启动自身，后端经 `fs_media_temp` 物化文件并持有预览租约，直到该进程退出（包括取消与出错）才经 `fs_media_release` 释放。启动时选择 GPUI 的 HWND 交换链（`GPUI_DISABLE_DIRECT_COMPOSITION`），使嵌入的控件可见。

全局搜索在后端运行，关闭页面后继续；选中运行中的搜索时页面跟随 `global-search-progress`。时间线把文件时间与分析事件合成一条事件流，提供按年、月、日的事件计数图和分页事件列表。分析结果按类别、分析器运行与记录类型组织，记录布局取自插件的记录结构。RAID 检测是会话级的，每个检材页显示其参与的阵列；无法由元数据或启发式恢复的阵列可手动指定几何重组。设置页提供外观（主题与界面尺寸）、已安装插件包与版本信息。

## 功能与命令对照

下表按工作流列出桌面入口（`crates/desktop/src/` 下的模块）与后端命令域；完整注册表以 `crates/backend/src/lib.rs` 为准。

| 功能 | 桌面入口 | 主要命令 / 事件 |
| --- | --- | --- |
| 案件与证据 | `workspace.rs`、`workspace/sources.rs`、`cases/` | `case_*`、`image_load`、`image_unload`、`physical_disks_list`、`image-load-progress` |
| 镜像信息 | `evidence/dashboard.rs`、`evidence/platform.rs`、`evidence/volumes.rs` | `image_info`、`autodetect_evidence`、`case_evidence_platform_set`（手动指定平台）、`image_export_raw`、`image_hash_compute`、`autodetect-finished` / `autodetect-failed` |
| RAID | `raid.rs`、`raid/builder.rs` | `raid_auto_detect`、`raid_manual_assemble`、`raid-detect-progress` |
| 文件浏览与预览 | `files/` | `fs_children_page`、`fs_flat_page`、`fs_read_range`、`fs_preview_text`、`fs_stat`、`fs_media_temp` / `fs_media_release`、`fs_node_locate` |
| 标签与派生数据 | 文件列表的标签与哈希 | `fs_tags_load`、`fs_tags_apply_changes`、`fs_digest`、`fs_digest_load_nodes` / `fs_digest_persist` |
| 导出 | `files/export.rs` | `fs_export`、`fs_export_dir`、`fs-export-dir-progress` |
| 全局搜索 | `search.rs`、`search/` | `global_search_*`、`global-search-progress` |
| 时间线 | `timeline.rs`、`timeline/table.rs` | `timeline_status`、`timeline_start`、`timeline_count`、`timeline_datas`、`timeline_export`、`timeline-progress` |
| 文件恢复 | `carving.rs`、`carving/` | `carve_*`（含 `carve_filesystem_results`）、`carve-scan-progress` |
| 加密与历史卷 | `evidence/unlock.rs`、`evidence/volumes.rs` | `bitlocker_*`、`encrypted_volume_unlock`、`filevault_inspect`、`shadow_copy_*`、`bitlocker-unlock-progress` / `bitlocker-scan-progress` |
| iOS 本地备份 | `evidence/ios.rs` | `ios_backup_unlock`、`ios_backup_audit`，操作与边界见 [iOS 备份](IOS_BACKUPS.md) |
| 挂载 | `evidence/volumes.rs` | `fs_mount_readonly`、`fs_unmount_readonly`、`fs_mounts`、`fs_share_mount` / `fs_unshare_mount`、`fs_available_mount_points` |
| BCP 数据 | `bcp.rs`、`bcp/table.rs` | `bcp_catalog`、`bcp_rows` |
| 插件与分析 | `settings.rs`、`analysis.rs`、`analysis/` | `plugin_list`、`plugin_install`、`plugin_uninstall`、`sched_task_functions`、`sched_systems_list`、`sched_task_create`、`analysis_result_tree`、`analysis_records_page` |
| 任务中心 | `tasks.rs`、`tasks/center.rs` | `task_list`、`task_cancel`、`task-entry` |

下列命令没有桌面调用方，主要作为 CLI 命令的后端（见 `lovelycarve-cli schema --list` 的 `ipcCommand`）：结构化原类型预览（`fs_sqlite*`、`fs_realm*`、`fs_binary_preview`、`fs_gguf_preview`、`fs_zip_*`、`fs_csv_preview`、`fs_vim_swap_preview`）、卷快照管理（`snapshot_build` / `snapshot_list` / `snapshot_drop`）、空间占用树 `fs_size_tree`、日志（`fs_usn_journal*` / `fs_ext4_journal*`）、数据流（`fs_streams` / `fs_stream_preview` / `fs_stream_export`）、`filedb_*`、`fs_source_locate`、`fs_docker_prepare`、`fs_identify_type` 与 `fs_ftype_*`、`qr_scan` / `ocr_scan`、`os_detect`、`win_drive_letters`、插件任务的 `sched_task_cancel` / `start` / `restart` / `remove`、`analysis_record_detail` / `analysis_run_get`，以及 `image_bad_regions`、`image_list`、`fs_children`、`fs_child_counts`、`fs_read`、`fs_read_large`、`fs_preview`、`carve_scan_status`、`carve_latest_scan`、`carve_preview`、`timeline_types`、`timeline_range`、`fs_timeline_export`、`fs_dir_stats`、`fs_subtree_empty_dirs`、`fs_flat_ext_counts`、`fs_digest_load`、`fs_tags_load_nodes`、`fs_tags_save`、挂载点建议与创建、`case_rename`、`case_option_*`、`case_evidence_list`、`plugin_restart`、`task_status`、`module_summary`、`system_path_info`、`reveal_path` 与 `sched_evidence_post_actions_set`。`fs_usn_journal` 与 `carve_latest_scan` 在 `commands.toml` 的 `[[superseded]]` 中指向替代命令。

## 扩展流程

新增跨前后端的功能通常需要同一改动完成：

1. `crates/core/src/` 增加核心逻辑。
2. `crates/backend/src/commands/` 增加命令函数（宿主参数使用 `crate::host` 类型）。
3. 在 `crates/backend/src/lib.rs` 的 `command_registry!` 中注册。
4. `crates/cli/src/commands.toml` 把它映射为一条 CLI 命令（集合、动词、位置参数、敏感与生成字段），否则构建失败。
5. 功能有桌面界面时，在对应页面经 `Backend::run` 调用它，需要的事件经 `EventHub` 订阅。
6. 运行 `just check`、`just test`、`just test-cli`，桌面应用有改动时再运行 `just test-desktop`；完整矩阵见 [贡献指南](../CONTRIBUTING.md)。

事件需要明确的 payload、归属、清理和错误处理；插件能力与 QR/OCR 等可选能力要显示准确的可用状态。新增入口必须有已注册的后端命令，不保留“暂未接入”的空壳菜单。修改后检查切换检材、页面关闭与迟到响应，避免旧任务的结果写入新的上下文。
### 插件结果发布互斥

插件运行把记录写进暂存运行；调度器提交前按 `(task_id, seq)` 校验执行归属，并获取每次执行独立的发布锁。暂存运行的发布（替换同一作业的上一次结果）、时间线投影与任务终态提交在锁内完成；取消、看门狗与案件解绑获取同一把锁后撤销执行。已撤销的迟到结果不能发布，暂存运行随之删除；已经开始发布的执行收尾后控制面重新判定状态。任务台账提交失败时按精确 run_id 删除刚发布的运行，不回删同一作业的其他运行。不同检材的发布不使用全局锁。

直接文件分析复用案件代际与检材移除的发布门；分析开始时捕获案件目录与代际，结束时重新验证并持锁写入，避免旧调用写入新案件。

### F2FS / EROFS 只读卷

固定超级块探测位于 `evidence::partitions::flashfs`，卷接入复用 `filesystem::volumes::rom_images` 的
泛型路径适配器、分区边界源与缓存会话。独立 `f2fs-reader` 维护校验后的 checkpoint、NAT 和
多级 node 寻址；`lamfold-erofs` 维护受限 inode / 解压缓存。`FoldFrontend::timestamps` 默认返回
未知角色，F2FS / EROFS 提供其实际记录的 UTC 秒 / 纳秒，宿主转换展示文本。没有新增全局
每检材缓存，所有读者随 FsSession / 证据引用释放。支持与验证见 [F2FS](formats/F2FS.md) /
[EROFS](formats/EROFS.md)。

### QNX4 / QNX6 只读卷

`evidence::partitions::qnx` 按根 inode / 超级块结构识别格式；`qnx-reader` 通过既有 `rom_images` 泛型适配器接入有界源、分区与路径身份。QNX4 缓存验证后的 extent 链并按逻辑末端二分定位，QNX6 选择 CRC 有效且几何一致的最新提交代际，共用有深度和容量上限的 inode / 长名 / 文件树读取。符号链接不在宿主跟随，原始证据字节不变；范围、流式、哈希和导出使用同一映射。支持范围见 [QNX](formats/QNX.md)。

### UDF 只读卷

`evidence::partitions::udf` 校验盘首 / 盘尾 anchor tag 与 CRC；`lamfold-udf` 复用 `rom_images` 有界源和路径适配器，校验同一 VDS 的单物理分区映射、FSD、FE / EFE 和 AED 续链。节点使用分区相对 ICB LBA，缓存清理不改变 inode / 硬链接身份；路径节点保留源分区 ID。范围、流式、哈希与导出共享映射，未知时区保持未知。每卷缓存随 FsSession 释放，没有新增全局缓存。支持范围见 [UDF](formats/UDF.md)。

### Minix 只读卷

`evidence::partitions::minix` 校验固定超级块与位图容量，`minix-reader` 复用 `rom_images` 有界源 / 路径适配器。每卷持有有上限的 inode 和 metadata block 缓存；范围读按目标 zone 遍历最多三级间接块，zone 大于 block 时只取第一个 block 的指针。inode / zone 分配位与物理范围一起校验，稀疏指针才返回零。V1 不虚构访问 / 变更时间；路径、硬链接和快照保留源分区与 inode 身份，缓存随 FsSession 释放。支持范围见 [Minix FS](formats/MINIX.md)。
