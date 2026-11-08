# JFS（Linux JFS2）

`jfs-reader` 为纯 Rust 只读库。适配前检查了两个现成 Rust 实现：
rusty-backup 的 JFS 读取范围较广；partmgr 0.1.2 的校验较严格，但公开读取 profile
限制为 4 KiB 分配块、单 IAG、低层级分配图以及没有 EA / ACL 等的文件。
本实现复用它们的字段布局与树校验算法，修正 inode 位图索引、内部 xtree 寻址和名称续槽语义，
并扩展分配块大小、多 IAG、目录索引、符号链接和 EA / ACL 分配检查。
固定版本、许可和样本来源见 [ORIGIN.md](../../vendor/jfs-reader/ORIGIN.md)。
运行时只依赖 Rust `lamfold` 字节源，不调用系统挂载、C 文件系统库或修复工具。

## 支持范围

| 能力 | 实现 |
| --- | --- |
| 格式 | Linux 小端 JFS，磁盘 magic 为 `JFS1`，超级块版本 1 / 2；512 / 1024 / 2048 / 4096 字节分配块，inode-map / 外部 xtree 页为 4096 字节 |
| 识别 | 主超级块位于 32768 字节，副本位于 61440 字节；裸卷及分区细化，独立 `jfs` 类型，不把普通 Linux MBR 类型直接视为 JFS |
| Inode | 主 AIT、fileset inode map、按逻辑页寻址的多个 IAG；每个 extent 对应自己的 32-inode 位图字，验证 inode stamp / fileset / 编号 / extent 身份与链接计数 |
| 目录 | 内联根、多级 dtree、512–4096 字节外部叶页、名称续槽、legacy 13 单元头和 indexed 11 单元头；校验有序键、父目录、自由槽链、节点自描述与兄弟链 |
| 目录索引 | 校验内联和外部 persistent directory-index table 与活动叶目录项的位置关系，不把外部索引根残留的未使用字节误当兄弟链 |
| 文件 | 内联根及多级 xtree、40 位逻辑 / 物理块号、稀疏洞、NOTRECORDED 未写入 extent、预分配尾部裁剪、直接范围读和 stream |
| 分配图 | 从 BMAP inode 定位 dmap；计算跨越多层控制页的逻辑位置；检查工作 / 持久位图一致、范围、页计数和所引用块的分配位 |
| 名称 / 链接 | 保留大小写，UTF-16 名称、长名称及字面反斜杠；常规硬链接共享 inode、保留路径节点身份；支持 0–255 字节内联及外部符号链接，读取时不跟随宿主链接 |
| EA / ACL | 接受内联和单 extent 描述符，校验大小及外部分配；文件数据与 EA / ACL / 树页的总分配块数须与 inode 一致；不新增属性内容 UI |
| 日期 | UTC 访问、状态改变、修改、创建日期，保留纳秒；Linux 语义下秒数按无符号 32 位展开，2038 年后日期不会变成负值；Unix epoch 0 保留为有效日期 |
| 应用 | 浏览、原类型预览、范围读、哈希、文件 / 目录导出和自动目录索引；Dokan 使用原生只读范围路径，原始检材字节和分区身份不变 |

内部 xtree 索引 XAD 的物理长度描述子节点页的分配长度，**不描述其覆盖的文件逻辑范围**。
因此遍历按逻辑键和子树结构进行，不能用一个页的物理长度限制子树覆盖的逻辑块。
名称续槽的 `cnt` 是槽计数，不是字符计数；续槽可容纳 15 个 16 位名称单元。

主超级块结构损坏时可以使用验证过的副本，库的 `selected_copy` 保留来源，宿主记录警告。
若主副本仍可识别但声明 dirty / mounted / recovery / extend 或不支持的特性，则直接拒绝；
不能通过选择陈旧的 clean 副本绕过这些状态。磁盘元数据没有被修复或重写。

JFS 原生名称中的反斜杠属于名称本身，只有 `/` 分隔路径。节点 ID 中的路径编码可保留这种名称；
CLI 中建议对包含反斜杠的文件使用列表返回的 `--node-id`，避免输入路径的跨平台分隔符归一化。

## 限制

- 支持 Linux JFS2 读取；AIX JFS1、AIX 专用布局、OS/2 大小写折叠命名空间和 DCE DFS 不在支持范围。
- 不回放日志、不恢复 dirty 卷、不扫描删除目录项或孤儿、不修改分配图 / 目录索引；未提交状态明确拒绝。
- 压缩、COW、transient XAD、indexed / file 型 EA / ACL 等未实现的布局明确拒绝，不当作普通字节或空洞。
- 校验实际读取的元数据、inode 和引用的分配位，**不是全卷 fsck**：不审计所有未访问 inode / 块的所有权，也不重算全部 buddy summary / control tree。
- 元数据树深度上限 8、每棵树外部页上限 65536、每个文件映射 extent 上限 262144；单目录条目上限 262144、名称单元总数上限 8 Mi；超过上限明确失败。
- IAG / dmap 各缓存最多 64 页，inode 缓存最多 128 项，最近目录和文件映射各保留一个。文件正文按范围读取，不因大文件一次性分配全部内容。
- 严格拒绝损坏的 UTF-16 名称和元数据关联；不以有损字符替换隐藏目录名损坏。
- 原生 GUI / Dokan 实际挂载以及 macOS / Linux 发布产物仍需单独验证。Linux 真实样本目前覆盖 4 KiB 块；其他块大小、多 IAG 和较大地址的验证来自独立构造样本或虚拟字节源。

## 验证

- `just test-jfs` / `just clippy-jfs`：外部 `mkfs.jfs` + guestfish 样本、200 项目录与文件内容；独立四种块大小 / 两种目录头布局、多 IAG、EA、长链接、稀疏 / 未写入数据、范围读、位图和树损坏拒绝。
- `just generate-jfs-fixtures`：用独立 Python 编码最小读取样本；样本只覆盖明确的读取结构，不冒充完整 formatter / fsck oracle。
- 使用虚拟字节源验证 32 GiB 以上的 dmap 控制页定位；这是定向寻址测试，不等于对真实大型 JFS 卷执行了完整对照。
- 主仓库测试覆盖裸卷 / MBR 细化、源字节与身份、全读 / 范围读 / stream / 导出、防覆盖、硬链接、特殊名称、快照，以及 dirty 卷的识别与拒绝；CLI smoke 核对 SHA-256、导出与跨 IAG 查询。

格式定义参照 Linux v6.12 的
[`fs/jfs`](https://kernel.googlesource.com/pub/scm/linux/kernel/git/torvalds/linux/+/refs/tags/v6.12/fs/jfs/)
头文件和对应读取行为。C 源码只用于格式核对，没有编译或移植其文件系统实现。
