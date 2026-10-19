# 前端模块与生命周期

本文记录当前前端的模块接口、状态归属与资源收尾约定。目录和 IPC 总览见 [架构说明](ARCHITECTURE.md#前端结构)，视觉与交互规范见 [FRONTEND.md](FRONTEND.md)。早期重构方案、文件迁移清单、实施批次和验收记录可从 Git 历史查阅。

## 模块边界

- `src/app/` 持有主窗口装配、workspace、窗口启动和跨功能工作流；功能模块不依赖 app。
- `src/features/` 的十二个功能各自拥有 components、composables、纯 model 和测试；跨功能调用使用小型 `api.ts`、明确的组件入口或纯契约。
- `src/components/` 和 `src/composables/` 提供共享 UI 与通用能力，不引用业务状态。纯模型不依赖 Vue 响应式、宿主对象或 IPC。
- `src/lib/ipc/` 封装所有 Tauri 命令与事件；传输类型放在该层，跨功能纯契约放在 `src/lib/models/`。
- 功能内部直接引用邻近实现，不回引自身 API barrel。公开 API 不导出重格式 viewer；主窗口面板和格式预览保持按需加载。

依赖规则在 `scripts/lib/frontend-structure.mjs` 中维护；新增跨功能边需说明实际消费者，并同步本页。

## 公开接口与允许依赖

`cases/api.ts` 提供只读案件/快照查询及案件操作；`tasks/api.ts` 提供任务中心条目（后端统一列表）、操作、插件包目录（含各包的语言包）和只读上下文注册；`evidence/api.ts` 提供证据会话、资源操作和回调注册；`preview/api.ts` 提供停靠/同步；`raid/api.ts` 提供只读检测概览和打开回调；`files/api.ts` 提供写入收尾及定位契约；`analysis/api.ts` 提供结果/焦点及上下文注册。其余功能没有跨域状态消费者，不为目录对称添加出口。功能内部不回引自身 API，公开 API 不导出重面板。

| 边 | 用途 |
| --- | --- |
| files → preview/cases API | 预览、卷快照状态与 Docker 合并的案件目录 |
| evidence → tasks API | 镜像信息页读取平台识别任务的状态 |
| evidence/settings → cases API | 挂载与案件上下文、设置显示 |
| analysis/settings → tasks API | 插件包目录：结果标签按插件语言包翻译，插件包的安装、卸载与列表 |
| cases → raid API | 只读 RAID 检测概览 |
| raid → evidence API | RAW 导出操作 |
| files → DiskFileDetail/FilePreviewDock/PluginTaskDialog；evidence → RaidStackPanel | 明确的按需组件入口，props/emits 协作 |
| search → files/model/tags；tasks → evidence/model/autodetectLabels | 小型纯定义；没有反向状态引用 |
| 跨功能 type-only → model | 纯契约，不作为运行时 bundle 依赖 |

应用装配向任务/分析提供可替换的只读案件和完成运行列表，向证据操作提供案件代际及解锁后快照回调。所有已声明边在 `scripts/lib/frontend-structure.mjs` 中检查；新增边需要说明实际消费者，不能扩大成任意内部引用。当前生产运行时图没有强连通依赖环。

## 状态范围

| 状态范围 | 典型内容 | 创建者与持有者 | 清理时机 |
| --- | --- | --- | --- |
| 每个 WebView 的共享界面状态 | toast、confirm、theme、motion、UI 偏好 | 现有共享 composable；主窗口和预览窗口各自初始化 | 对应 WebView 结束；持久化值保留 |
| 主工作区状态 | 活动案件、标签、当前证据、任务中心条目、RAID 探测、分析结果状态 | 主壳唯一装配点；保留现有单例/工厂语义 | 案件切换/关闭、对应任务结束、主窗口结束 |
| 检材/卷/任务的缓存与进行中状态 | 哈希、解锁、识别、导出、标签缓存、快照进度 | 所属功能；按当前契约使用 imageId、partitionId、taskId | 资源确实关闭或上下文失效；普通切页不清理进行中任务 |
| 面板实例/单次读取状态 | 筛选、页码、选中、滚动、DOM 高亮、预览读取与租约 | 面板或其 composable | imageId 重置、请求替换、停用/销毁（按资源性质区分） |

模块级 refs 是**每个 WebView 内**的单例，并非多个窗口共享的 JS 内存。独立预览只能通过既有事件/存储同步共享数据，不能靠 import 主窗口单例获取当前案件或标签。

标签快照 LRU、KeepAlive 面板缓存、后端证据运行时是三个不同概念。淘汰前端快照不等于卸载后端镜像，关闭可见标签也不能未经契约确认就取消后台任务/关闭被独立窗口引用的证据。

## 所有权与收尾

| owner | 存活与收尾 |
| --- | --- |
| app/bootstrap | 挂载前应用主题/动效/日志；main 仍用两个动态根分派窗口 |
| app/workspace/useTabManager | 主窗口标签、MRU 与五份 fileSession LRU；关闭/切案清空快照，不替代五类 KeepAlive 面板状态 |
| bindWorkspaceServices | 持有案件/任务/分析/证据上下文和回调注册；scope 销毁解除注册并调用案件、识别、任务中心、分析、BitLocker、镜像哈希/导出的生产 dispose |
| 案件切换 | busy 防止并发变更；关闭标签/视图 → flush 标签防抖和已发出的派生数据写入 → 后端 create/open/close/remove；旧 active/snapshot/list 查询不能覆盖新案件 |
| 文件标签/派生缓存 | 保留 `lc.fileTags` 等持久化键及格式；hydration generation 阻止旧案件水合触发写入；flush 等待已发出写入，失败保留本地副本并反馈 |
| fileSession 工厂 | 各实例拥有 load generation；reset/restore/dispose 失效在途读取；迟到 progress 订阅即时释放 |
| 缓存面板 | 切页停用文档键盘、菜单与未执行排序；后台任务继续。image/context 改变使查询和界面回填失效，组件销毁释放计时器/订阅 |
| 预览/ZIP/雕刻读取 | 各实例拥有读取代际、租约和 Blob URL；旧完成/销毁后不创建新 URL，迟到资源释放；范围及容量限制保留 |
| BitLocker/哈希/导出注册表 | 工作区资源 owner；部分订阅失败释放已成功句柄并允许重试，迟到句柄停止，dispose 幂等；选择目录时捕获原上下文，旧选择不能启动新案件操作 |
| 识别/任务/分析单例 | 可重试订阅、可释放 effectScope、请求/案件代际；主窗口装配结束才 dispose，不因普通面板卸载取消后端任务 |
| 独立预览 | 保留 owner/preview/revision、先订阅后请求最新选择、停靠确认及关闭协议；窗口媒体资源随实例释放，主题/动效同步维持原 storage 协议 |

UI dispose 与后端取消/卸载分别由各自的生命周期管理。已发出的后端导出/恢复可继续，并按对应资源通知。事件没有案件编号时，前端使用订阅代际、上下文失效和权威数据刷新，不能假定该事件携带案件身份。

## 异步请求与窗口边界

每次查询、选择器、订阅和读取捕获案件、证据或请求代际，发布结果前再次核对；只比较 imageId 不能识别 A→B→A 的过期响应。销毁后到达的订阅、媒体句柄和租约立即释放，dispose 保持幂等，订阅部分失败允许释放后重试。

切换案件先关闭标签与视图，再 flush 标签防抖和已经发出的派生数据写入，随后调用后端 create/open/close/remove。代际检查只能阻止界面回填，不能撤销已经发出的后端写入。

KeepAlive 面板停用时释放可见面板的键盘、菜单及未执行排序；后台任务继续。实例销毁时释放订阅、定时器、媒体租约与 Blob URL。标签的 fileSession LRU、KeepAlive 缓存和后端证据会话分别管理，不相互替代。

主窗口和独立预览是不同 WebView；通过既有 owner/preview/revision 事件与存储协议同步。独立预览先订阅再请求最新选择，媒体资源随窗口实例释放；不能通过 import 共享主窗口 JS 内存。

## 验证

结构与 IPC 边界运行 `just frontend-structure-check`、`just ipc-check`。前端改动按 [贡献指南](../CONTRIBUTING.md#验证) 执行类型检查、测试与构建。

生命周期变化需要覆盖案件 A→B→A、同节点跨检材、迟到订阅与读取、部分订阅失败重试、写入收尾、KeepAlive 停用和独立预览关闭。浏览器 mock 验证不能代替真实 Tauri 窗口、镜像读取或挂载验证；发布验收见 [发布指南](RELEASING.md)。
