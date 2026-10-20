# 前端规范

本文是 LovelyCarve 前端（Vue 3 + TypeScript + Vite）的设计与开发规范，覆盖设计原则、主题 token、
字号与间距、组件基元、交互状态与反馈、表格、弹窗与层级、图标、动效、文案、可访问性和验证流程。
新增或修改界面前先读本文，再读三个样式源文件：

- `src/styles/themes.css`：light / dark / pink 三套主题 token。
- `src/styles/ui.css`：全局共享的 `ui-*` 基元。
- `src/styles/motion.css`：动效层（共享关键帧、基元入场、`ui-stagger`、主题圆形展开）。

样式以这三个文件为准；本文与代码不一致时，以代码为准并同步修正本文。`AGENTS.md` 的
「Frontend Guidelines」是本文的英文摘要，规则变更时两处一起改。前端架构（窗口、标签、面板、
任务中心、动效开关的实现）见 [ARCHITECTURE.md](ARCHITECTURE.md)，开发命令见
[DEVELOPMENT.md](DEVELOPMENT.md)。

前端按应用装配、12 个业务功能、共享 UI/能力和类型化 IPC 组织。模块接口与资源生命周期见 [前端模块与生命周期](FRONTEND_STRUCTURE.md)。

## 1. 设计原则

LovelyCarve 是信息密集的取证工作台：界面要让人一眼看清当前对象、可做的操作和操作结果。

| 原则 | 在本项目中的做法 |
| --- | --- |
| 可供性与提示 | 控件自己说明用法：可点元素有悬停态与指针，选中项有底色与主题色，禁用项降低透明度并显示禁止指针。纯图标按钮必须有 `title` 与 `aria-label`。 |
| 视觉层级 | 越重要越大、越醒目。同一区域至多一个实心主按钮；强调色只给当前项、主操作和链接；整列重复的行内操作平时用次要色。 |
| 网格与间距 | 间距走 4px 节奏；组内紧、组间松，用留白而不是线条分组。 |
| 字体与字号 | 只用界面字体与等宽字体（品牌字只用于 Logo）；工作区字号集中在 11–13px，全部为整数像素。 |
| 色彩 | 品牌色调亮做浅底、调暗做文字；语义色含义固定（蓝/主题色＝信息与链接，绿＝成功，琥珀＝警告，红＝错误与危险）。颜色要有目的，不做纯装饰。 |
| 不用左缘色条 | 卡片、列表行、表格行、通知和面板的左缘不画彩色竖条（`border-left`、`::before` 竖条、`inset Npx 0 0` 阴影都算）：状态用徽章、图标或状态横幅，分类用图例色块，选中用底色加主题色文字，表格选中行用细描边。中性的 1px 分隔线、树形缩进线和 Markdown 引用块的竖线不在此列。 |
| 深色模式 | 靠表面明度而不是阴影分层：卡片比画布亮；边框低对比；标签降低饱和度，把对比放在文字上。 |
| 阴影 | 卡片最弱、浮层更强、弹窗最强；阴影不应是第一眼看到的东西。 |
| 图标与按钮 | 图标高度约等于相邻文字的行高；幽灵按钮悬停才显示底色；弹窗按钮宽度不小于高度的两倍。 |
| 反馈与状态 | 每次操作都有回应：悬停、按下、聚焦、禁用、加载、成功、失败；输入框有聚焦态和错误态。 |
| 微交互 | 复制后图标变为对勾、计数变化时跳动、通知从右侧滑入；所有动效可一键关闭。 |
| 遮罩 | 弹窗背景压暗并轻度模糊；不在图片上直接叠文字，确需叠加时使用渐变底。 |

## 2. 技术约定与目录

| 位置 | 内容 |
| --- | --- |
| `src/AppShell.vue` | 主窗口外壳：标题栏、侧栏、内容区、全局弹窗。顶层界面元素只能加在这里。 |
| `src/App.vue` | 主窗口根组件，挂载外壳、`ToastHost` 与 `ConfirmDialogHost`。 |
| `src/views/FilePreviewWindow.vue` | 独立预览窗口（`?view=file-preview`），不构建主窗口外壳。 |
| `src/app/` | 挂载前初始化、主壳组件、标签/导航和跨功能工作流；唯一的业务接线层。 |
| `src/features/` | cases、evidence、files、preview、search、carving、timeline、analysis、bcp、tasks、raid、settings；每个功能就近放组件、composable、纯模型及测试。 |
| `src/components/` | `ui/`、`feedback/`、`layout/`、`display/` 共享组件，不引用业务状态。 |
| `src/composables/` | 与业务无关的通知、确认、主题、动效、持久化及通用交互。 |
| `src/lib/models/` | 文件定位等跨功能纯契约；传输类型仍属于 IPC。 |
| `src/lib/ipc/` | 类型化 IPC 服务层：`client.ts`（唯一的 `invoke` 封装）、`events.ts`（`onEvent` / `emitEvent`）与各领域命令模块。 |
| `src/utils/` | 纯函数：格式化、文件分类、HTML 净化等，配套 `*.test.ts`。 |
| `src/styles/` | 主题、基元、动效与共享样式表。 |

- 单文件组件一律 `<script setup lang="ts">` + Composition API；组件状态用 `ref` / `computed` / `watch`。
  业务状态放进 `features/<功能>/composables/useXxx.ts`，通用能力才放根 `composables/`；不引入 Pinia、Vuex 或其他全局 store。
- 功能之间使用小型 `api.ts`、明确的组件入口或纯模型契约；功能内部不回引自己的 API，不能反向依赖 `app/`。公开 API 不导出整批 viewer。共享组件/状态不引用业务模块，纯模型不读取 Vue 状态、宿主对象或调用 IPC。`just frontend-structure-check` 检查允许边、静态/动态导入、运行时依赖环及失效路径。
- 单例由主窗口装配持有清理；工厂由实例 scope 持有。切页不取消后台任务；面板停用关闭键盘/菜单等可见性资源，销毁释放订阅、计时器、读取租约和 Blob URL。异步读取用请求代际校验，迟到的订阅/媒体句柄立即释放；切案先收尾标签及派生数据写入。具体所有权见结构方案第 7、14 节。
- 组件和 composable 不直接 import `@tauri-apps/api/core` 或 `@tauri-apps/api/event`，统一调用
  `src/lib/ipc/*` 中带类型的函数（如 `bitlockerStatus({ imageId, partitionId })`）。`just ipc-check`
  强制检查这一边界。后端错误以 `IpcCommandError`（`code`、`message`、`retryable`）抛出。
- 较重的面板通过 `defineAsyncComponent` 懒加载，延迟 120ms 后显示 `AsyncPanelLoading`；文件列表、
  搜索、雕刻、时间线与分析结果放在 `KeepAlive` 中，切换页面保留状态。
- 组件样式写在 `<style scoped>`；共享样式表通过 `<style scoped src="../../../styles/xxx.css">` 引用（相对路径按组件实际层级调整）。
  内联 `style` 只用于运行时数值（宽度、位置、CSS 变量），不写颜色与字号。
- 新增本地持久化的键名以 `lovelycarve-` 开头；重构不改既有键（包括 `lc.fileTags`）。读写包在 `try / catch` 中（受限环境下 `localStorage`
  可能不可用），简单值可用 `usePersistedRef`。主题与动效偏好通过 `storage` 事件同步到独立窗口。
- `v-html` 只能绑定 `SafeHtmlString`：由 `src/utils/safeHtml.ts` 的 `escapeHtml` / `composeSafeHtml` /
  `sanitizeHtml` 生成。界面会渲染来自检材的不可信数据。
- 全局单例：通知 `useToast`、确认框 `confirm()`（`useConfirm.ts`）、主题 `useTheme`、动效 `useUiMotion`。

### RAID 信息

- RAID 卡片只在块设备检材的信息页挂载；文件夹、逻辑归档和 Android / iOS 备份不因“无分区”而显示 RAID 入口或触发检测。
- 已重组阵列的成员盘保留案件登记与运行会话，但不显示为独立标签；标签切换、编号快捷键及关闭后的回退只使用可见标签。重新打开成员盘进入对应阵列，关闭阵列标签后不自动展开成员标签。案件恢复先注册源镜像并检测阵列，只有独立证据与重组镜像创建可见标签；尚未重组的成员仍可查看诊断。
- 普通磁盘与重组镜像的「导出源镜像」放在检材头部右侧、平台识别刷新按钮之前；导出开始时已有解锁分区则导出其明文并保留整盘布局。文件夹、逻辑归档、iOS 备份与离线索引不显示此入口。当前重组镜像的 RAID 卡片不再提供手动重组、重新检测或浏览文件按钮；成员盘页保留重组、检测与「打开重组镜像」入口。自动重组的标签页与页面标题共用「RAID N 重组镜像」，完整会话标识移至可复制的详情字段。镜像信息与 RAID 容量按 1024 换算时使用 KiB / MiB / GiB 等 IEC 单位，文件系统名称共用展示映射。展开参数列保持紧凑，条带图不随宽卡无限拉伸。
- 镜像信息的 RAID 卡片默认只展示阵列类型／状态、成员数量和按槽位排列的成员名，成员盘页的打开与导出集中在卡片右上方；容量和文件系统沿用上方镜像／卷信息，不重复展示。块大小、校验布局、数据偏移及条带示意合并到「重组详情」，单阵列检测说明也归入其中；多阵列的全局诊断保持独立。自动生成的阵列名称与 UUID 放在提示中，成员缺失或校验重建仍明确标记；陈旧、重建中、备用、故障与重复加载的成员以徽章标出，不占槽位的成员列在槽位之后。等待确认的降级阵列在卡片右上方提供「降级重组」，选择在案件会话内保留，切换案件时清除。条带示意由纯模型 `features/raid/model/stripeLayout.ts` 生成，与后端读取器使用同一套 RAID0/1/4/5/6/10 布局规则；跨 PV 的逻辑卷在卷组行打开。
- RAID 自动检测弹窗打开时，原生拖入文件优先进入成员盘队列，成员区显示松开提示，全局「添加检材」遮罩不盖住弹窗；选择文件与拖入共用去重、串行加载和自动检测，确认后才打开重组镜像。切换案件或重开队列时不回填旧请求。

### 设置与案件管理

- 设置中心使用横向分类导航；界面页以「语言 / 主题 / 动画 / 挂载」选项行组织，更改自动保存。
  文件夹挂载选中后才显示根目录和自动挂载；说明只保留决策所需信息。
- 主窗口不设全局底部状态栏。主题切换仅在「设置中心 → 界面」提供；「日志」查看当前会话输出，保留搜索、级别筛选、复制、清空及自动滚动；「关于」显示软件版本与本地构建时间。
- 「关于」提供手动版本检查与默认关闭的自动检查开关；开启后每次启动读取项目 GitHub tags，正式版忽略预发布标签，不自动下载或安装。常驻设置壳拥有检查请求，关闭弹窗不取消请求；关闭自动检查或销毁设置壳取消在途请求并丢弃迟到结果。网络仅允许 `https://api.github.com`，检查不发送凭据或检材信息。
- 插件列表默认展示名称、版本、运行状态与「重置」操作，能力、权限、指纹与路径放入默认折叠的详情。
  重置表示下次使用时重新启动；进度、成功与失败显示在当前插件卡片中。
- 案件管理优先展示检材及「添加检材」，卷快照与案件详情默认折叠，关闭案件放在详情内。
  切换 / 关闭 / 锁定等操作期间禁用弹窗关闭并显示当前操作；失败在弹窗内提示。
  案件列表加载失败保留已有记录并提供重试，避免把失败显示成空列表；时间统一标注「本地」。

## 3. 主题与设计 token

主题由 `<html data-theme>` 切换：`:root` 是 light，另有 `[data-theme='dark']` 与 `[data-theme='pink']`。

- 颜色、圆角、阴影与时长一律通过 `var(--*)` 使用。`themes.css` 以外不写十六进制色、`white` / `black`
  或硬编码 `rgba(...)`；只有找不到合适 token 的半透明叠加层例外。
- 新增 token 必须同时加到三个主题块里。
- 只用已有的 token 名。常见误用：没有 `--muted`（用 `--text-secondary`）、没有 `--border`
  （用 `--border-color`）、没有 `--font-sans`（用 `--font-ui`）。

### token 分类

| 类别 | token | 用途 |
| --- | --- | --- |
| 画布与表面 | `--bg-primary` | 应用画布 |
|  | `--surface-primary` | 次级条带：页面页脚（`ui-footbar`）、详情栏标题条、记录侧栏 |
|  | `--surface-secondary` | 卡片、表格、输入框、弹窗、菜单 |
|  | `--surface-tertiary` | 凹陷区：分段控件底、空态图标底、表格行悬停 |
|  | `--sidebar-bg` / `--titlebar-bg` | 侧栏、标题栏 |
| 文字 | `--text-primary` / `--text-secondary` | 正文 / 次要文字与说明 |
| 线条 | `--border-color` | 边框与分隔线 |
| 强调色 | `--primary-color` | 文字、图标、边框、焦点环、进度条等强调元素 |
|  | `--primary-glow` | 输入框聚焦光圈、主按钮悬停投影 |
|  | `--sidebar-active-bg` | 选中项浅底（侧栏、表格选中行、开关按钮） |
|  | `--hover-bg` / `--active-bg` / `--sidebar-hover-bg` | 中性悬停与按下底色 |
| 实心填充 | `--primary-solid(-hover)` / `--danger-solid(-hover)` | 主按钮、危险按钮、开关的填充色 |
|  | `--accent-contrast` | 实心填充上的文字 |
| 语义 | `--sev-{critical,warn,success,info}-{fg,bg,bg-hover,border,tag-fg,tag-bg}` | 提示条、徽章、状态行；另有 `--sev-critical-border-strong`、`--sev-info-selected-*` |
|  | `--sev-snapshot-*` | ReFS 快照行与标签（琥珀色） |
| 图表 | `--chart-1` … `--chart-6` | 分类色（磁盘布局、类别徽章），配合 `ui-badge tone` 与 `--badge-tone` |
| 形状 | `--radius-sm` 4px / `--radius-md` 8px / `--radius-lg` 12px | 徽章与小图标按钮 / 控件、菜单、提示条 / 卡片、弹窗、通知 |
| 阴影 | `--shadow-sm` / `--card-shadow` / `--card-shadow-hover` / `--modal-shadow`；`--modal-backdrop` | 分段选中项 / 卡片 / 悬停抬起与通知 / 弹窗、菜单与浮动面板；弹窗遮罩 |
| 字体 | `--font-ui` / `--font-mono` / `--font-brand` | 见第 4 节 |
| 动效 | `--transition-fast` / `--transition-normal`；`--motion-fast` / `-normal` / `-slow`；`--ease-out` / `-in` / `-spring` | 见第 11 节 |
| 品牌 | `--gradient-accent`、`--brand-*` | 品牌渐变（不定进度条）与欢迎页插画，不用于工作区控件 |

### 对比度要求

| 内容 | 最低对比度 | 背景 |
| --- | --- | --- |
| 正文、次要文字、强调色文字 | 4.5:1 | 各级 `--surface-*`、`--bg-primary`、`--sidebar-bg`、`--sidebar-active-bg` |
| 实心按钮文字（含悬停态） | 4.5:1 | `--primary-solid` / `--danger-solid` 及其 `-hover` |
| 输入框占位符 | 3:1 | 输入框底色 |
| 图标、焦点环、进度条、表格选中描边等非文字元素 | 3:1 | 相邻颜色 |

品牌浅色（light 的 `#3b82f6`、pink 的 `#e8849f`）只出现在渐变、光晕、浅底与图表里，不能用作文字。
三套主题当前实测值（WCAG 2.x 相对亮度）：

| 组合 | light | dark | pink |
| --- | --- | --- | --- |
| 正文 / 卡片 | 17.9 | 15.1 | 13.5 |
| 次要文字 / 卡片、侧栏 | 5.2、4.7 | 6.2、7.0 | 5.4、4.9 |
| 强调色文字 / 卡片、选中浅底 | 5.5、4.6 | 7.2、7.1 | 5.7、4.7 |
| 实心按钮文字 | 5.2 | 4.6 | 4.6 |

调整任何文字类 token 后，按上表重新核对三套主题，不达标不合入。

### 三套主题

- **light**：石板白画布，蓝色强调。
- **dark**：深夜蓝画布，浅天蓝强调；卡片比画布亮来分层，边框低对比。
  语义色前景使用浅色系（如 `#fca5a5`），背景透明度更低。
- **pink**：樱粉画布，深玫红强调、墨紫正文；柔和的樱粉留给渐变、浅底与图表。

## 4. 字体与字号

字体只用本地资源：应用 CSP 不允许远程样式与字体，需要新字体时打包到 `src/assets/fonts/`。

| token | 字体 | 用途 |
| --- | --- | --- |
| `--font-ui` | Inter → Segoe UI → 平台中文字体 | 默认界面字体 |
| `--font-mono` | Cascadia Mono → 系统等宽字体 | 偏移、哈希、扇区、ID、恢复密钥、时间戳、表格数字、计数与状态徽章 |
| `--font-brand` | Quicksand（已打包 500 / 700） | 仅品牌字与 Logo |

没有衬线字体，也不要为单个页面引入新字体。

| 字号 | 用途 |
| --- | --- |
| 11px | 次要信息：说明、提示、徽章、计数、表单提示与错误、页脚统计 |
| 12px | 正文：表格（等宽数字同为 12px）、按钮、输入框、列表、详情字段 |
| 13px | 强调：卡片与分组标题、页签、通知标题、侧栏分组、大按钮 |
| 14px | 详情标题、空态标题、欢迎页主按钮 |
| 15px | 弹窗与确认框标题 |
| 16px | 页面级标题（`ui-heading-title`）、欢迎页卡片标题 |
| 18–22px | 统计数字与结论标题 |
| 32px 起 | 仅欢迎页品牌字 |

- 只用整数像素；工作区内不超过 24px；一个页面内的字号不超过 6 种。
- 大号数字收紧：字距 -0.01em～-0.02em，行高 1.1～1.2；会变化或需要对齐的数字加
  `font-variant-numeric: tabular-nums`。中文标题不加负字距。
- 行高：说明文字 1.5～1.7；表格单行 18px；按钮、徽章等单行控件 1。
- 字重：标题 600，强调标签 500，正文 400；700 只用于少量数字强调（统计值、计数角标），更粗的字重只用于品牌字。

## 5. 间距与布局

### 间距

- 间距取 4px 节奏：4 / 8 / 12 / 16 / 20 / 24…；小控件内部可用 2px 半步（如 6px、10px）。
  不使用奇数值，1px 的视觉微调（图标对齐）与 `.sr-only` 除外。
- 组内紧、组间松：图标与文字 4–8px（`ui-btn` 为 6px，`sm` 为 4px）；同组控件 6–8px；
  卡片内边距 16px；区块之间 16–24px。

### 控件尺寸

| 元素 | 高度 |
| --- | --- |
| `ui-btn` | 30px（`sm` 26px、`lg` 38px） |
| `ui-icon-btn` | 28px（`sm` 24px、`lg` 30px） |
| `ui-input` / `ui-select` / `ui-search` | 30px（`sm` 26px） |
| `ui-seg-item` / `ui-tab` | 24px / 40px |
| `ui-badge` / `ui-count` / `ui-kbd` | 20px（`sm` 18px）/ 18px / 20px |
| `ui-menu-item` | 不低于 30px |
| `ui-toolbar` / `ui-footbar` / 卡片标题行 / 详情栏标题条 | 不低于 48px / 32px / 48px / 48px |
| 表头 / 表格行 | 34px / 36px |
| 标题栏 | 38px |
| 导航宽度 | 总宽 288px（大类栏 64px）；窗口不超过 1000px 时总宽 252px，收起后仅保留大类栏 |

### 外壳与页面结构

- 外壳由标题栏（标签页、「任务中心」、案件、设置入口）、左侧导航和内容区组成；页面自己的 `ui-footbar` 保留上下文与统计。
- 标题栏检材标签页使用独立的激活与关闭按钮，当前标签页标记 `aria-current="page"`，关闭按钮至少 24px 且在键盘聚焦时可见；待加载状态用时钟图标与提示表达，不降低可用标签页文字的对比度。标签页区独立横向滚动，激活的标签页在切换和缩窄窗口后保持可见，「新标签页」按钮位于滚动区外。案件入口固定写「案件管理」，当前案件名放在提示中；任务计数复用 `ui-count`。
- 「任务中心」「案件管理」「设置中心」共用 `ui-btn sm titlebar-chip`，正常宽度均显示图标与文字，窗口宽度不超过 760px 时同时只显示图标；始终保留无障碍名称和悬停提示。
  三个入口的文字和图标统一使用 `--text-primary`，悬停与打开状态通过背景和边框表达。
  不在 `AppShell.vue` 之外新增顶层界面元素。
- 标签页行下方采用三栏：左侧紧凑大类栏、中间当前大类的子页面、右侧工作区。镜像包含镜像信息与文件列表；
  工具包含全局搜索、时间线与文件恢复（文件夹或离线索引隐藏文件恢复）；分析直接列出插件执行结果，点击结果在右侧查看记录。
  点击大类展开子类栏并恢复该大类最近访问的页面；任务跳转、文件定位与标签恢复同步导航选中项。
  子类栏可收起；BCP 检材额外保留 BCP 数据大类。左侧大类栏底部只保留子类栏展开 / 收起按钮，不提供过滤或添加检材入口。
  中间栏直接从子页面列表开始，不重复显示大类标题；分析结果目录保留自身的搜索框。
  悬停或键盘聚焦大类显示对应子类：展开时直接在中间栏预览，收起时临时浮出；不切换右侧页面，也不挤压内容区。
  移入子类栏保持显示，移出导航或按 Esc 恢复当前页面的大类；收起状态下同时关闭浮层。
  点击浮出栏中的子页面或插件结果后打开页面并收起浮层；点击大类则固定展开。
- 工作区页面不设标题栏（侧栏已经给出页面名），统一为三段：
  1. `ui-toolbar`：搜索与筛选在左，页面操作放在右侧 `ui-toolbar-actions`，主操作在最右。
  2. 工作区：表格、卡片或预览。
  3. `ui-footbar`：当前任务、状态与计数，数字写成 `<span class="ui-footbar-stat">命中 <strong>213</strong> 处</span>`。
- 尚无数据的页面使用 `WorkspaceStartState`：图标、标题、说明、一个主操作；历史记录等入口放进其 `toolbar` 插槽。
- 右侧详情在当前窗口内展开，切换页面与开合详情都不改变原生窗口尺寸：
  - 用 `<UiTransition kind="dock">` 包裹 `DetailDock`，内容用 `FileDetailLayout`、`FileDetailSection`、
    `FileDetailFields` 组织。
  - 承载详情栏的行设 `position: relative; container: detail-host / inline-size`。
  - 宽度 300–720px（默认 380px），所有详情栏共用 `lovelycarve-detail-width`，分隔条支持键盘左右键调整；
    容器窄于 800px 时浮在表格上方。
- 面板内的响应式用容器查询（`@container`），只有整窗口级变化才用媒体查询；页面在约 760px 宽的窗口中仍要可用。
- 文件预览的页签与窗口动作放在首行，编码、字符集和搜索放在下一行工具栏左侧，识别操作靠右；窄面板中工具组可换行，保持左侧起点一致。
- 文件列表预览拆分成功后由独立窗口接管，主窗口不再展开内嵌预览；后续打开的文件同步到同一窗口。停靠时恢复当前文件，直接关闭独立窗口后下一次打开文件恢复内嵌预览。
- 文件列表在地址栏（页面的 `ui-toolbar`）下方，于表格上方另有一条筛选工具栏：列表范围、文件类型与生效条件标签在左，导出与显示列在右，与目录树、预览栏的标题条同高。每个收窄列表的条件都显示为可修改、可移除的标签，空结果区分「被筛掉」与「空目录」。
- 文件预览按 `file-workspace` 容器宽度响应：不超过 1200px 时浮在表格上方，更宽时并排停靠并为目录树与默认表格列保留至少 880px。预览宽度仍使用 `lovelycarve-preview-width`，与通用详情栏宽度独立。
- 拆分与停靠保留当前标签、搜索词与命中、文本编码、Hex 页码/分页大小及正文滚动位置（含代码视图）；切换文件保留视图选择，目录使用详细信息。加密状态没有检测依据时标为「未检测」。
- 大小和时间范围在校验通过后才应用。非法数值、无效日期或颠倒的上下限保持编辑器打开，在字段旁说明问题并聚焦错误字段；原有筛选结果继续有效，不自动交换用户输入。

## 6. 组件与基元

界面控件从 `ui-*` 基元搭建，组件的 scoped 样式只写布局与组件特有的视觉，不重新声明按钮、输入框、
徽章或弹窗外观。基元都是单类选择器，修饰与状态写在 `:where()` 里，因此任何 scoped 规则无需
`!important` 即可覆盖；新增基元必须保持这一点。

| 基元 | 用途与要点 |
| --- | --- |
| `ui-btn` | 文字按钮。变体：默认、`primary`（区域主操作，至多一个）、`danger`（确认框内的破坏性操作）、`ghost`（次要操作，悬停才显底色）、`link`（表格行内与文字型操作）、`danger-soft`（不需要醒目的移除、清空，可与 `ghost` / `link` 组合）。尺寸 `sm` / `lg`，`icon` 为方形图标版，`block` 占满宽度。开关型按钮用 `.active` 或 `aria-pressed`。 |
| `ui-icon-btn` | 无边框方形图标按钮，`sm` / `lg` / `bordered` / `danger`；`.active`、`aria-pressed`、`aria-expanded` 显示激活态。必须有 `title` 与 `aria-label`。 |
| `ui-input` / `ui-select` / `ui-textarea` | 文本框、下拉框、多行文本，`sm` 为紧凑尺寸。占位符写示例或格式，不代替标签。`ui-select` 是 `UiSelect` 触发按钮的外观，不直接用在原生 `<select>` 上。 |
| `ui-search` | 搜索框：前置图标 + 无边框输入框 + 可选的清除按钮。 |
| `ui-field` / `ui-field-label` / `ui-field-hint` / `ui-field-error` | 表单字段：标签、控件、提示；校验失败时用 `ui-field-error` 替换提示（见第 7 节）。 |
| `ui-check` / `ui-switch` | 带标签的复选框 / 单选框；`role="switch"` 的复选框画成开关。立即生效的设置用开关，需要提交的选项用复选框。 |
| `ui-seg` + `ui-seg-item` | 2–4 个互斥的视图或模式。 |
| `ui-tabs` + `ui-tab` | 页内分区导航（下划线页签）。 |
| `ui-badge` | 状态徽章：`info` / `success` / `warn` / `danger` / `outline`；分类色用 `tone` 加 `--badge-tone`；`sm`、`pill`。 |
| `ui-count` / `ui-kbd` | 标题或页签旁的数字计数（`accent` 为强调）/ 键盘按键。 |
| `ui-card`（`-header` / `-title`）、`ui-section-title`、`ui-heading` | 卡片、面板内小节标题、页面级标题与说明。卡片＝`--surface-secondary` + 1px `--border-color` + `--radius-lg` + `--card-shadow`。 |
| `ui-alert` | 页内提示条：`info` / `success` / `warn` / `danger`。 |
| `ui-progress` | 进度条：`success` / `danger` / `warn` / `indeterminate`。 |
| `ui-empty`（`-icon` / `-title` / `-desc` / `-actions`） | 空状态。 |
| `ui-toolbar` / `ui-toolbar-actions` / `ui-footbar` / `ui-footbar-stat` / `ui-footbar-spacer` / `ui-divider-v` | 工作区页面结构与工具条分隔线。 |
| `ui-modal-overlay` / `ui-modal`（`-header` / `-icon` / `-heading` / `-title` / `-subtitle` / `-body` / `-footer`） | 弹窗（见第 9 节）。 |
| `ui-choice`（`-icon` / `-text` / `-title` / `-desc` / `-arrow`） | 整行选项卡片：作为按钮选择来源，或包住单选 / 复选框选择设置。 |
| `ui-menu`（`-item` / `-label` / `-sep`） | 菜单与气泡；`ui-menu-item danger` 为危险项，`aria-selected="true"` 为列表的当前值，`data-highlighted` 为焦点留在触发器上时的键盘高亮。 |
| `ui-disclosure` + `ui-chevron` | 原生 `details` 折叠区，配合 `v-disclosure-motion`。 |
| `spin` / `mono` / `sr-only` | 加载旋转、等宽文字、仅供读屏的文字。 |

共享样式表：`data-table.css`（表格）、`report-viewer.css`（PE / ELF / Mach-O / ar / GGUF 结构视图）、
`db-browser.css`（SQLite / Realm）、`analysis-workspace.css`（搜索与雕刻工作区）、`analysis-detail.css`（记录详情块）。

共享组件：`DetailDock`、`FileDetailLayout`、`FileDetailSection`、`FileDetailFields`、
`WorkspaceStartState`、`AsyncPanelLoading`、`UiTransition`、`UiSelect`、`FileNodeIcon`、`BrandMark`。

下拉框一律用 `UiSelect`（`v-model` + `options: { value, label, disabled? }[]`，`@change` 在值更新后触发），
不用原生 `<select>`：系统弹出的列表不跟随主题。触发按钮沿用 `ui-select` 外观并按最宽选项定宽，切换选项时
布局不跳动；列表是 `ui-menu`，通过 Popover API 放在顶层，弹窗里也不会被遮挡或裁切。键盘遵循 select-only
combobox 模式：方向键、Home / End、PageUp / PageDown、首字母跳转，Enter / 空格选中，Esc 只关闭列表。

## 7. 交互状态与反馈

### 状态

| 状态 | 表现 | 实现 |
| --- | --- | --- |
| 默认 | 清楚显示可点（边框、文字色或图标） | 基元默认样式 |
| 悬停 | 边框或底色略向主题色靠拢；幽灵按钮出现底色 | 基元内置 |
| 按下 | 底色加深，动效开启时轻缩 | 基元内置，`motion.css` |
| 键盘聚焦 | 2px `--primary-color` 焦点环 | `:focus-visible`，不要移除 |
| 禁用 | 透明度 0.45–0.55，`cursor: not-allowed` | `disabled` 属性 |
| 选中 | `--sidebar-active-bg` 浅底 + 主题色文字；侧栏另有圆点，表格选中行另加 1px 主题色描边 | `.active` / `aria-pressed` / `aria-selected` |
| 加载 | 按钮前置图标换成 `<Loader2 class="spin" />` 并禁用；面板用 `AsyncPanelLoading` | 见下文 |
| 错误 | 红色边框与焦点环 + 原因说明 | `aria-invalid` + `ui-field-error` |

### 加载与长任务

- 按钮内操作：前置图标换成 `Loader2` 加 `.spin`，文案可改为进行态（「导出中」），同时禁用按钮防止重复提交。
- 等待超过约 1 秒的内容区显示原因（「正在加载镜像…」）；长任务优先使用骨架或分段进度，能给出百分比时
  用确定进度条，并提供取消入口。后台任务接入任务中心，离开页面后仍能看到状态。
- 不定进度条在动效关闭时显示为静态、半透明的满条（`:root[data-motion='reduced']` 下的静态样式），
  不能停在半途。`.spin` 在动效关闭时继续以半速旋转，避免用户误以为程序卡死。

### 通知（Toast）

| 函数 | 用途 | 显示时长 |
| --- | --- | --- |
| `notifySuccess(title, detail?)` | 操作完成，且界面上没有其他可见结果 | 3.2s 起 |
| `notifyInfo(title, detail?)` | 中性信息 | 3.2s 起 |
| `notifyError(title, detail?)` | 操作失败 | 4.2s 起 |
| `pushPersistentToast(options)` | 长任务进度或需要用户处理的提醒，返回可 `update` / `dismiss` 的句柄 | 直到调用方关闭 |

- 显示时长随内容增加（约每字 90ms，最长 10 秒）；鼠标悬停或键盘聚焦时暂停计时，离开后至少再停留 1.5 秒。
- 弹窗（`.ui-modal-overlay`）或菜单（`.ui-menu`）打开时，常驻通知暂时隐藏，关闭后恢复，避免遮挡其按钮。
  提醒类常驻通知在用户开始处理时应主动关闭（如打开 BitLocker 解锁弹层即收起「检测到 BitLocker」）。
- 标题写结果（「导出成功」「复制失败」），说明写对象、原因和下一步；批量操作汇总成一条
  （「已打开 3 个镜像页」），不要逐项弹出。
- 通知不能代替表单字段的错误提示，也不重复报告界面上已经可见的结果。

### 复制

复制按钮成功后把图标换成 `Check` 约 1.5 秒，并用 `sr-only` 的 `role="status"` 播报「已复制」
（实现示例见 `FileDetailFields.vue`）；也可以弹出成功通知。失败时 `notifyError('复制失败', 原因)`，不能显示为已复制。

### 确认破坏性操作

删除、清空、覆盖等不可撤销的操作先 `await confirm({ title, message, confirmText, danger: true })`。
正文说明后果和不受影响的部分（「已恢复到磁盘的文件不受影响」）。同一时间只有一个确认框，
新的请求会以取消结束旧的。不要使用 `window.confirm()`。

### 表单与校验

- 校验失败时给控件加 `aria-invalid="true"`（红色边框与焦点环），在控件下方用 `ui-field-error` 写明原因，
  并通过 `aria-describedby` 关联（id 用 `useId()` 生成）；同一位置平时显示 `ui-field-hint`。
- 确定错误的输入（格式不合法、校验和不通过）在输入时立即提示，并指出具体位置；
  「还没填完」只在提交或离开输入框后提示，半截输入不显示为错误。
- 输入合法前禁用提交按钮，并在提示里给出进度或格式要求；可以安全规范化的输入（空格、全角符号、
  缺少分隔符）自动整理，在离开输入框时显示整理后的结果。
- 实现示例：BitLocker 恢复密钥输入（`BitlockerUnlockControl.vue` + `src/features/evidence/model/bitlockerRecoveryKey.ts`）。

```html
<input class="ui-input mono" :aria-invalid="error ? 'true' : undefined" :aria-describedby="noteId" />
<p v-if="error" :id="noteId" class="ui-field-error" role="alert"><AlertCircle :size="12" /><span>{{ error }}</span></p>
<p v-else :id="noteId" class="ui-field-hint">共 8 组 6 位数字，可直接粘贴</p>
```

### 空状态与错误状态

- 空状态使用 `ui-empty`：图标、标题（「还没有搜索记录」）、说明下一步（「输入关键词或正则，查找检材中的文件内容」）、
  一个主操作。区分「还没有数据」与「筛选后没有结果」，后者提供清除筛选的入口。
- 加载失败在原位置显示 `ui-alert danger` 或状态条，给出原因和「重试」；不展示堆栈。
  后端原始错误可附在中文说明之后，并补充恢复步骤（「解密会话已关闭，请重新解锁源镜像」）。

## 8. 表格与列表

- 分析结果按插件记录结构选择阅读方式：多条同构记录默认横向表格，插件声明 `properties`
  时默认纵向属性列表。属性名、主值、辅助键名由字段 id 指定，不在前端按插件名或字段名猜测。
  工具栏保留「属性视图 / 表格视图」切换，并按插件与记录类型保存选择；搜索、分页、导出共用同一组记录。
  横向表格直接使用可见字段，仅无可见字段时补名称列；纵向属性列表保留同名多条记录，值允许换行，
  英文键名退为次要文字；没有属性角色的记录类型在属性视图中每条记录一张卡片，字段按宽度分列。
  源文件显示实际路径，点记录查看完整字段与全部来源，隐藏字段仍可在详情读取。
  表格列宽只由字段类型决定（文本、列表与 JSON 平分剩余宽度），不按记录内容伸缩；时间戳显示原始精度与时区，
  未记录时区的不补时区；缺失值是次要色的「—」。运行概览沿用镜像信息页的头部与卡片结构。

- 原生数据表格加 `data-table` 类，并通过 `<style scoped src="../styles/data-table.css">` 引用文件列表基准样式。
- 表头 34px，常规行 36px（虚拟列表依赖固定行高，如 `DiskFileList.vue` 与 `SearchResultsPanel.vue` 的
  `ROW_HEIGHT = 36`），字号 12px。数字列右对齐（`.numeric`）并使用等宽数字；长内容单行省略，
  `title` 中给出完整值。
- 文件列表的列标题即排序按钮，未排序时排序图标只在悬停时出现。列筛选入口（漏斗）放在表头单元格边缘，
  平时隐藏，指针进入表头时显示，有条件的列常亮；数值列标签右对齐，漏斗放在左侧。生效的条件同时在表格上方显示为标签。
- 选中行使用 `--sidebar-active-bg` 浅底，并在行内侧画 1px 描边（`--primary-color` 的 70%，三套主题下对画布
  不低于 3:1）；只靠浅底时，选中行与悬停行在 light / pink 下几乎分不开。快照等取证状态行选中时保留
  自己的行底，只靠描边表示选中。行可获得焦点时显示实线主题色焦点轮廓。
- NTFS 命名数据流及其所属文件沿用普通行底色，用图标和「数据流」标签标识；ReFS 快照保留语义底色。
- 已删除条目沿用普通行底色、悬停和选中反馈，通过红字、名称删除线与「已删除」标签标识，不使用整行红底。
- 行内操作统一用 `ui-btn link` 并带图标，同一列的操作图标与文字对齐；平时显示次要色，
  悬停、选中或键盘聚焦该行时才亮起主题色。操作列使用固定宽度，按钮高度 24px。
- 大列表使用 `useVirtualList` 虚拟滚动；虚拟列表的占位行不加边框与内边距，也不使用 `ui-stagger`
  （回收或移动的行会重新播放入场动画）。

## 9. 弹窗、浮层与层级

### 弹窗

```html
<Transition name="ui-modal">
  <div v-if="visible" class="ui-modal-overlay">
    <div ref="dialogEl" class="ui-modal" role="dialog" aria-modal="true" aria-labelledby="xxx-title"
      tabindex="-1" @keydown.esc.stop="close">
      <header class="ui-modal-header">
        <span class="ui-modal-icon"><Search :size="18" /></span>
        <div class="ui-modal-heading">
          <h2 id="xxx-title" class="ui-modal-title">新建搜索</h2>
          <p class="ui-modal-subtitle">在检材的文件内容中查找关键词或正则表达式</p>
        </div>
        <button class="ui-icon-btn" aria-label="关闭" title="关闭" @click="close"><X :size="16" /></button>
      </header>
      <div class="ui-modal-body">…</div>
      <footer class="ui-modal-footer">
        <button class="ui-btn" @click="close">取消</button>
        <button class="ui-btn primary">开始搜索</button>
      </footer>
    </div>
  </div>
</Transition>
```

- 用 `useDialogFocus(() => visible, dialogEl)` 在打开时把焦点移入弹窗，关闭后还给打开它的元素；
  Esc 绑在弹窗容器上并 `.stop`，使叠在上层的弹窗只关闭自己。处理中（提交、启动任务）时禁用关闭。
  可能同时存在多个实例的组件用 `useId()` 生成标题 id。
- 页脚：次要内容（提示、危险链接）放左侧并加 `.start`，右侧依次为「取消」与主操作；
  页脚按钮最小宽度为高度的两倍。
- 位于可滚动容器内的弹窗用 `<Teleport to="body">`，避免被裁切。

### 菜单与气泡

- 使用 `ui-menu`，从触发元素处弹出：向下弹出时 `--ui-pop-offset` 为负，向上弹出时为正。
- 点击外部或按 Esc 关闭；用 `position: fixed` 定位时把边缘限制在视口内，空间不足时翻转方向。
- 靠近窗口右下角的浮层（通知所在区域）应使用 `ui-menu`，常驻通知才会自动让开。
- `UiSelect` 的列表 Teleport 在 body 下；内含下拉框、按外点或滚动关闭的浮层要放过 `.ui-select-list`
  里的事件（见 `DiskFileList` 的列过滤气泡）。

### 层级（z-index）

| 层级 | 用途 |
| --- | --- |
| 1–60 | 面板内部：吸顶表头、详情栏浮层与分隔条（个位数），面板内绝对定位的下拉（几十） |
| 90 / 100 | 侧栏、「任务中心」浮动面板 / 标题栏 |
| 900 | 终端输出抽屉 |
| 1000 | 菜单、气泡、下拉 |
| 9000 | 弹窗（`ui-modal-overlay` 默认值） |
| 9100–9999 | 叠在弹窗上的弹窗、全局入口弹窗、拖放遮罩 |
| 12000 | 通知 |
| 13000 | 确认框（可从任何弹窗中唤起） |

新增层级取用上表中的档位，不另造数值。

## 10. 图标

- 只用 `lucide-vue-next`（`import { Search } from 'lucide-vue-next'`，`<Search :size="14" />`）；
  不引入其他图标库，界面中不使用 emoji。
- 图标高度约等于相邻文字的行高：

| 尺寸 | 场景 |
| --- | --- |
| 11–12px | 页脚统计、徽章、表单错误前缀 |
| 13–14px | 按钮、菜单项、工具条、表格行内操作 |
| 16px | 图标按钮、关闭按钮 |
| 18px | 弹窗标题图标 |
| 26–30px | 空状态图标 |

- 图标块（`ui-modal-icon`、`ui-choice-icon`、`ui-empty-icon.accent`）中的图标取强调色；纯装饰图标加 `aria-hidden="true"`。
- 状态不能只靠颜色或图标表达，要配文字（「已完成」「失败」）。

## 11. 动效

- 动效可以开关：设置 → 界面 → 界面动画提供「跟随系统 / 开启 / 关闭」。
  `useUiMotion` 在挂载前把偏好与系统「减少动态效果」合并成 `<html data-motion="full | reduced">`。
  `data-motion` 是唯一开关：CSS 中不要查询 `prefers-reduced-motion`，否则会使用户显式选择的「开启」失效。
- `reduced` 时 `base.css` 让所有过渡与动画瞬时完成（加载旋转除外）。装饰性动效写在
  `:root[data-motion='full']` 下（全局样式表中用 `:where(:root[data-motion='full'])` 保持单类优先级）。
- 时长与曲线取 token：普通状态变化用 `--transition-fast`（0.15s）/ `--transition-normal`（0.25s）；
  动效用 `--motion-fast|normal|slow`（150 / 250 / 400ms）配 `--ease-out`（入场）、`--ease-in`（退场）、
  `--ease-spring`（小幅弹出）。入场用较长时长，退场用较短时长。
- 复用 `motion.css` 的关键帧：`ui-rise-in`、`ui-pop-in`、`ui-zoom-in`、`ui-bounce-in`、`ui-grow-x|y`、`ui-bump`，
  以及仅用于插画与拖放目标的循环动画 `ui-float` / `ui-twinkle` / `ui-breathe`。关键帧只声明起始状态，
  使用独立的 `translate` / `scale` / `rotate` 属性并以 `backwards` 填充，可与组件自身的 `transform` 叠加。
- 短小、非虚拟化的列表与卡片网格加 `ui-stagger` 依次入场；计数变化加 `ui-bump` 并以数值作 `:key`。
- 由脚本驱动的动效统一走 `animateUi` / `motionTiming`、`UiTransition`（`collapse` / `horizontal` / `fade` /
  `pop` / `dock`）、`v-content-motion`（轻量淡入，`.rise` 用于页面与页签切换）和 `v-disclosure-motion`；
  动效关闭时它们立即落到最终状态。
- 切换主题时把被点击的控件传给 `setTheme(theme, from)`，新主题从该处圆形展开。

## 12. 主题检查

每个界面都要在 light、dark、pink 三套主题下检查：

- 文字与图标达到第 3 节的对比度；实心按钮文字清晰；焦点环可见。
- 深色主题下卡片与画布有明度差，边框不刺眼，徽章与提示条不发亮。
- 樱花色主题下强调文字使用深玫红（`--primary-color`），不出现浅粉色文字。
- 图表与磁盘布局使用 `--chart-*`，相邻色块可区分。

## 13. 文案

- 界面支持简体中文与英文，默认简体中文；在「设置中心 → 界面 → 语言」切换，即时生效并保存到 `lovelycarve-locale`。主窗口与独立预览窗口在挂载前由 `initLocale()` 设置 `<html lang>`，通过 `storage` 事件同步，不重建工作区或取消后台任务。
- 应用文案使用 `composables/useLocale.ts` 的 `tr('中文源文案', { 0: value })`；英文集中在 `src/i18n/en.json`。同一个中文词有不同语境时使用语义键，例如 `motion.off`、`timeUnits.day`，并在 `src/i18n/zh-CN.json` 和英文词典中分别定义。动态内容用完整句子的插值词条，保留占位符，不能先拼接字符串再翻译。界面上的列表用 `i18n/translate.ts` 的 `joinList(items, tr)` 连接（语义键 `list.separator`：中文「、」，英文「, 」），不写死 `join('、')`。
- 标题、按钮、占位符、提示、无障碍名称、校验信息和原生选择器标题都要翻译。静态选项用 getter 或 `computed` 在读取时翻译，不能在模块初始化时固定语言。纯模型通过可选的 `Translator` 参数接收翻译函数，默认 `sourceText`，不导入 Vue、偏好状态或宿主 API。技术名词保留原文：NTFS、BitLocker、MD5、E01、VMDK 等。
- 文件名、检材路径、卷标签、案件名、解析内容、插件提供的名称/字段和后端原始诊断保留原文；只翻译应用自己定义的标签。持久化标签值、布尔筛选值、命令参数、状态枚举和检材身份不能使用译文。应用自有选项可用 `localizeOptions()` 翻译 label，保留 value。
- `just i18n-check` 检查词条覆盖、插值参数和模板静态中文；该检查包含在 `just verify-fe` 中。新增或修改文案时同步维护两种语言。
- 简洁、可执行：按钮用动词短语（「新建搜索」「恢复全部」「导出 CSV」），状态用完成时（「已完成」「已复制」）。
- 中文与英文、数字之间加空格（「检测到 BitLocker 加密分区」「共 4 项」）；中文语境使用全角标点；
  引用界面上的按钮或名称用「」（「前往解锁」）；省略号用「…」。
- 数字：计数用 `toLocaleString()`（「1,204」），容量用 `formatBytes`，时间用 `formatEpochSeconds` 并写明时区
  （本地 / UTC / UTC+8）；表格与页脚中的数字使用等宽字体。
- 错误：标题写「操作 + 失败」（「打开镜像失败」），说明写原因与恢复步骤；空状态说明下一步。
- BitLocker 状态徽章 LOCKED / UNLOCKED 保留英文；中文与英文界面都应简洁可执行。

### 取证术语表

中文与英文按下表一一对应，新增文案沿用这些说法，不另造同义词。

| 概念 | 中文 | 英文 | 说明 |
| --- | --- | --- | --- |
| 案件中登记、供检验的数据源 | 检材 | evidence（计数写 evidence items） | 包括镜像、文件夹、物理磁盘、备份与归档；「证据」只用于 E01 字段「证据编号」（Evidence number） |
| 没有分区表的检材 | 逻辑检材 | logical evidence | 文件夹、归档、AD1、Docker 等；AD1 这类格式本身称「逻辑镜像」（logical image） |
| 检材背后的镜像文件 | 源镜像 | source image | 不写「原镜像」「原始镜像」 |
| 未经修改的检材本身 | 原始检材 | original evidence | 用于删除、移除操作中「不受影响」的说明 |
| 采集 | 采集 | acquisition | 采集时记录的哈希称 acquisition hash；E01 的 Examiner 写「检验人」 |
| 整盘哈希 | 源盘哈希 | source disk hash | 不写「原始介质哈希」 |
| 搜索结果 | 命中 | hit | 「匹配」（match）只用于筛选与预览内查找 |
| 文件雕刻 | 雕刻、签名雕刻 | carving、signature carving | 结果称「雕刻结果」（carving results） |
| 未分配空间 | 未分配空间 | unallocated space | |
| 分区之间的区域 | 分区间隙 | partition gap | 不写「磁盘间隙」「分区之间的空隙」 |
| 坏区读取时补的零 | 补零字节、补零数据 | zero-filled bytes / data | 不写「合成零字节」「合成数据」 |
| 重建出来的分区 | 丢失分区 | lost partition | 不写「遗失分区」 |
| 卷影副本 | 卷影副本 | shadow copy | |
| 文件标签 | 标签 | tag | 界面页签一律写「标签页」（tab） |
| 按原格式渲染的预览 | 原类型视图 | native view | |
| RAID 缺少的成员 | 缺失、缺失盘 | missing、missing disk | 不写「掉线」 |
| 停止进行中的任务 | 取消 | cancel | 不写「终止」 |
| 读入检材或数据 | 加载 | load | 不写「载入」 |

## 14. 可访问性

- 所有操作都能用键盘完成；不移除 `:focus-visible` 焦点环。快捷键写进按钮 `title`（「关闭 (Ctrl+W)」），
  界面中用 `ui-kbd` 展示。现有快捷键：Esc 清空或关闭、Ctrl+O 添加检材、Ctrl+T 新标签页。
- 纯图标按钮有 `title` 与 `aria-label`；弹窗有 `role="dialog"`、`aria-modal` 与 `aria-labelledby`；
  开关用 `role="switch"`；分隔条用 `role="separator"` 并支持键盘调整。
- 动态结果用通知或 `sr-only` 的 `role="status"` 播报，错误用 `role="alert"`；表单错误通过 `aria-describedby` 关联。
- 颜色不作为唯一信息来源；对比度见第 3 节；动效关闭后界面仍完整可用。
- 桌面密集布局下控件不低于 24px，常用操作 26–30px。

## 15. 测试与验证

- 单元测试用 Vitest，与源文件同目录的 `*.test.ts`；需要 DOM 的测试在文件首行写 `// @vitest-environment happy-dom`。
  涉及计时的逻辑用 `vi.useFakeTimers()`；mock 通过 `defineAsyncComponent` 懒加载的 `.vue` 时，工厂返回
  `{ __esModule: true, default: … }`。
- 通过 `just` 运行检查，不直接调用 Vitest：
  - 前端改动：`just typecheck`、`just test-fe`、`just build-fe`。
  - 跨前后端改动：另加 `just ipc-check` 与对应的 Rust 检查。
- 行为或状态逻辑变化要补测试；纯样式改动至少在三套主题下实际查看。
- 界面改动的拉取请求附上三套主题的截图（动效改动附录屏），并列出运行过的 `just` 命令。

## 16. 界面改动自查清单

- [ ] 只用已有 token 与 `ui-*` 基元，没有硬编码颜色、字号或重复的控件样式。
- [ ] 没有左缘彩色竖条：状态、分类与选中用徽章、图标、图例色块、底色或描边表达（见第 1 节）。
- [ ] light / dark / pink 下文字与图标达到对比度要求，焦点环可见。
- [ ] 字号为整数像素且在字号表内；间距为偶数并符合 4px 节奏。
- [ ] 每个可交互元素都有悬停、按下、键盘聚焦与禁用态；纯图标按钮有 `title` 与 `aria-label`。
- [ ] 加载、空状态、错误状态都有设计；长任务可取消并显示进度。
- [ ] 操作结果有反馈（通知、图标变化或界面更新）；破坏性操作有确认。
- [ ] 表单错误用 `aria-invalid` + `ui-field-error` 指出具体问题。
- [ ] 动效开启与关闭两种状态下都正确；虚拟列表没有入场动画。
- [ ] 窄窗口（约 760px）与右侧详情展开时布局仍可用，原生窗口尺寸不变。
- [ ] 中英文文案、无障碍名称与校验信息完整，长英文不遮挡控件；标点与空格符合第 13 节，`just i18n-check` 通过。
- [ ] 跑过 `just typecheck`、`just test-fe`、`just build-fe`（涉及 IPC 时加 `just ipc-check`）。
