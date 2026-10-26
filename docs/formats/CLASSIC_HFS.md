# 传统 HFS（Mac OS Standard）

`classic-hfs` 为独立纯 Rust 只读库，复用并加固现有 Rust 实现：fstool 0.5.0
的 HFS 字段布局 / MacRoman 解码器，以及 rusty-backup 的 HFS 名称比较表。
选型前也检查了 `hfs-reader` 0.1.1，其整镜像借用 / 整 fork 缓冲接口不适合检材范围读取。
固定提交、原始许可、样本来源见 [ORIGIN.md](../../src-tauri/vendor/classic-hfs/ORIGIN.md)。
不依赖 C 文件系统库、系统挂载、修复工具或写入实现。

## 支持范围

| 能力 | 实现 |
| --- | --- |
| 识别 | 1024 字节处 `BD` MDB，校验分配块 / 位图几何；裸卷和 APM / MBR / GPT 分区内容细化，使用独立 `hfs` 类型 |
| 目录 | 512 字节 catalog B-tree、固定宽度 index key、多级索引、目录 / 文件 / thread 记录，保留 CNID；验证父节点、循环、valence 和 thread 一致性 |
| 文件内容 | 三个 inline extent、overflow extents、碎片化目录树和 extents 树；有界整读、seek、stream、哈希、导出和目录索引 |
| 两个分叉 | 默认文件读取 data fork；现有命名流接口中的 `com.apple.ResourceFork` 读取 / 导出 resource fork，包括 data fork 为空的文件 |
| 名称 | 原始字节保留在库 API；显示 / 查询按 MacRoman，使用 HFS 大小写不敏感、重音敏感的排序比较（含非 ASCII 大小写） |
| 时间 | 创建 / 修改日期按 1904 起点解码为无偏移本地日期字符串；原始备份日期保留在库 API，不冒充修改或访问时间 |
| 完整性 | MDB / 文件逻辑长度、物理长度、分配范围、位图、fork 内 extent 重叠、缺失续链、B-tree 节点位图 / 数量 / 高度 / 子节点 / 分隔键 / 叶链 / 记录边界 / 键序错误明确失败 |

普通浏览、预览、哈希和导出使用 data fork；目录索引也按 data fork 大小计数。
资源分叉可通过已有 CLI 使用，例如：

```text
files streams list --path /Café.bin
files streams get /Café.bin --stream-name com.apple.ResourceFork
files streams export /Café.bin --stream-name com.apple.ResourceFork --destination resource.bin
```

命名流传输沿用既有 `name / size / resident` 结构，HFS 两个分叉均为非 resident。
桌面目前没有分叉选择器；普通文件 / 目录导出不会自动生成 AppleDouble 或合并资源分叉。
需要保留完整经典 Mac 文件时，分别导出两个分叉，或保留原始镜像。

HFS 使用冒号分隔路径，磁盘名中的 `/` 显示为 `:`；原始反斜杠显示为 `＼`，
仅由一个或两个点组成的合法 HFS 名称显示为 `．` / `．．`，均可逆查询，避免与宿主路径语法冲突。
其他脚本系统的名称没有可靠的磁盘编码标记，目前按 MacRoman 显示；不宣称支持日文等旧 Macintosh 编码。
比较严格沿用 HFS 的旧排序表，不使用现代 Unicode 大小写折叠；表中部分后加字符（如 `â` / `Â`）保留历史上的不等价关系。

## 时间语义

Apple [TN1150](https://developer.apple.com/library/archive/technotes/tn/tn1150.html)
明确传统 HFS 日期使用本地时间，与 HFS+ 大部分日期的 GMT 语义不同。
因此输出形如 `2024-05-31T11:07:28`，没有 `Z` 或假定的时区偏移。
`lamfold::LocalFileTimes` 与 UTC 时间独立传输；没有源时区证据的日期不投影到 UTC 案件时间线。
零时间视为未记录。Finder alias 作为普通文件读取，不解析为宿主符号链接。

## 已知限制

- 只使用主 MDB；不自动恢复损坏主 MDB、回放未提交修改、扫描删除目录项或修复文件系统。
- HFS+ wrapper 内嵌卷、MFS、Disk Copy 4.2 容器、原始软盘 GCR / MFM / flux 流不属于本读取器支持范围；原始 HFS 卷字节可读。HFS+ / HFSX 仍走[原有读取器](HFS.md)。
- 单个元数据 B-tree 上限 64 MiB，目录深度上限 256；超过范围明确报错。元数据需载入有界内存，文件 fork 按需范围读。
- extents 树自身的下一段映射必须能从已知段覆盖的 header / leaf 链取得；若自描述记录不可达则失败，不扫描不属于已验证树的区域。
- 不宣称完整 fsck 或全卷跨文件块所有权审计。损坏内容不会用零补齐；分配块仍须在源卷内且位图已分配。
- 原始 FinderInfo、文件共享权限、桌面数据库等没有新增 UI 展示；不将 resource fork 解码成资源条目。
- 当前独立样本覆盖 800 KiB hfsutils 格式化卷和构造的碎片化布局，尚未对大量真实经典 Macintosh 系统盘、原生 GUI / Dokan 挂载或 macOS / Linux 发布产物验证。

## 验证

- `just test-classic-hfs` / `just clippy-classic-hfs`：外部 hfsutils 样本，独立 Python 编码的 512 / 1024 / 4096 字节分配块、双 fork、空文件 / 资源独占、MacRoman、碎片目录与自 overflow、边界读及损坏拒绝。
- `just generate-hfs-fixtures`：重建原创样本并记录 SHA-256，不使用 Rust reader / writer 生成预期数据。
- 主仓库测试覆盖裸卷 / APM 细化、源字节与身份、浏览 / 范围读 / stream / 导出、资源分叉、新建目标防覆盖、自动快照和本地时间保留；CLI smoke 覆盖目录、范围读、SHA-256、data / resource 导出。
