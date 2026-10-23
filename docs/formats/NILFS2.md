# NILFS2

`nilfs2-reader` 是纯 Rust、只读的 NILFS2 2.0 检查点读取器。适配前检索了
crates.io / GitHub：`unearth` 仅识别卷；`rust-nilfs2` 是空 Rust 库和 C 工具源码；
`btree-ondisk` 是采用不同节点标记的通用 B-tree，引擎不包含 NILFS2 的持久元数据解析。
本实现复用 `unearth` 的 MIT 超级块前缀位置，新增有界的检查点、DAT、目录和内容读取。
固定来源与许可见 [ORIGIN.md](../../src-tauri/vendor/nilfs2-reader/ORIGIN.md)。
产品不链接 C 文件系统库，不调用内核挂载或外部提取器。

## 能力

| 项目 | 范围 |
| --- | --- |
| 格式与几何 | NILFS2 2.0、小端、1–64 KiB 二次幂块；主 / 备用超级块 CRC、设备与 segment 范围、记录尺寸、特性位和根元数据边界校验；兼容 segment 区占满设备、没有备用副本的早期几何 |
| 检查点 | 由选中超级块的最新 partial segment 定位 super-root，校验完整日志及 SR CRC，再解析 cpfile / ifile；双超级块优先较大的检查点号，同号优先主副本 |
| 历史 | 独立 Rust API 可枚举 snapshot 链、按检查点号打开保留视图；验证 snapshot 标志、双向链、计数、ifile 与 DAT 生命周期；不把历史 inode 和当前 inode 的状态混用 |
| 地址与分配 | DAT 物理指针、普通文件 / cpfile / sufile / ifile 的虚拟指针；持久分配器的组描述符 / 位图、组内条目边界及 segment 使用状态；已分配但缺失的映射报错 |
| 块树 | 六项 direct map、内联 B-tree 根及 1–13 层节点；验证节点级别、容量、递增键、父分隔范围、非零指针及循环；按请求逻辑偏移定位 |
| 日志与 GC | summary / 完整 partial-log / SR 的原始 CRC32；finfo / binfo 将正文块绑定到 inode、虚拟块号和逻辑偏移，区分 DAT 正文与树节点；读取 GC 搬移后的 DAT 映射 |
| 目录与内容 | 8 字节对齐目录、64 KiB 特殊记录长度、UTF-8 / 255 字节名称、原始反斜杠、硬链接、稀疏空洞、符号链接和特殊文件类型；删除项不提前截断枚举 |
| 时间 | mtime / POSIX ctime 及独立纳秒，保留负时间与零值；NILFS2 没有独立存储 atime，不把内核从 mtime 合成的值记成访问时间，也不虚构 creation time |
| 应用 | 当前提交视图的裸卷 / 分区识别、浏览、预览、哈希、导出、范围读、目录快照、挂载适配及 CLI；节点保留源分区与路径身份 |

已分配正文必须通过分配位图、segment 状态、DAT 生命周期和日志正文身份核对。
仅格式定义的空洞返回零；范围读、完整读和流式读共用同一逻辑读取路径。
所有宿主导出沿用新建文件、不覆盖规则，源字节始终只读。

## 限制

- 输入必须是完整、稳定、干净卸载的视图。dirty、error、resize 状态、未知
  incompatible / readonly-compatible 特性以及 legacy sketch 检查点明确拒绝。
  不执行日志重放、写入、修复、孤儿扫描或扫描式删除恢复。
- 双副本身份 / 几何冲突明确失败；不扫描整盘寻找任意历史 super-root。
  CRC 损坏时可使用几何有效的备用副本，但不根据损坏副本猜测新事务。
- 日志 CRC 覆盖完整 partial log；请求少量字节也可能需要先读取其所在日志。
  相关日志的其他字节损坏也会使本次读取失败，不做静默修复或绕过 CRC。
  单 partial log 校验最多 64 MiB；单 segment 扫描最多 131072 个日志。
- 日志证明缓存最多 64 项、正文身份合计最多 262144 项；块缓存最多 64 块，
  分配器组 / segment 校验缓存分别最多 128 项，inode 缓存最多 128 项。
  单目录最多 131072 块 / 100 万名称，目录身份最多 100 万项，snapshot 链最多 65536 项。
  超出工作量上限时报错，不截断为成功结果。
- 非 UTF-8 名称 / 文本链接目标无法由当前文本契约无损表示时明确失败。
  atime、扩展属性、ACL 等原生尚未提供的能力不虚构；非零 xattr 引用明确拒绝。
- 应用与 CLI 当前展示最新提交的卷树；历史视图目前通过独立 Rust 读取器 API
  `snapshots()` / `open_checkpoint()` 提供，尚无原生历史快照的界面或命令入口。
- 原生核对覆盖 1 / 2 / 4 KiB 块；更大块通过独立自制夹具覆盖，尚未在相应
  Linux 大页内核上核对。Windows Dokan 实际挂载仍需独立 GUI / 驱动验证。

## 验证

- `just test-nilfs2` / `just clippy-nilfs2`：独立 Python 夹具覆盖三种块大小、
  两个 inode 分配组、direct / B-tree、多个检查点、删除项、链接、稀疏和损坏场景。
  `just generate-nilfs2-fixtures` 可重复生成并记录 SHA-256。
  另有虚拟 8 GiB 源：真实访问的日志位于 6 GiB，文件逻辑长度超过 5 GiB，
  检查直接定位、空洞不读源、EOF 和读取故障传播，未分配数 GiB 缓冲区。
- `scripts/nilfs2-native-fixtures.py --output <新目录>` 在一次性 Linux 主机中新建
  普通镜像文件，通过 `mkfs.nilfs2`、内核写入、`mkcp` 和只读 snapshot 挂载生成清单。
  使用 Debian Linux `6.1.0-54-amd64` / `nilfs-tools 2.2.9-1`，四个 512 MiB 输入覆盖
  1 / 2 / 4 KiB 块、512 / 1024 / 2048 块 segment、block_count 开 / 关，
  一万个或 1500 个目录项、12 MiB 文件、32 MiB 稀疏文件、碎片文件及两个历史版本。
  NILFS2 原生工具尚无 fsck；不把内核读回或 `nilfs-tune` / `dumpseg` 说成 fsck 验证。
- `--gc-copy-source <4096/512 原生镜像>` 与新的 `--output` 另建第五个副本，
  写入并丢弃临时版本，再强制原生 cleaner；只读挂载回读两个保留 snapshot。
  原生样本含 12 个 CRC 有效的 GC 日志摘要，segment 使用表也发生实际变化。
- 设置 `NILFS2_ORACLE_DIR` 为解码目录后运行 `just test-nilfs2-native`。
  GC 输入可在同目录，或由 `NILFS2_GC_ORACLE_DIR` 单独指定。
  审计先核对五个原始镜像的 SHA-256，再核对当前视图及两个历史 snapshot 的全部
  名称、inode / 模式 / 大小、纳秒时间、链接目标、文件完整哈希与范围内容。
  原生输入保存在外部测试缓存；仓库仅包含 CC0-1.0 自制小夹具。
- 应用 / CLI 测试另行覆盖裸卷 / MBR、备用超级块、来源身份、目录快照和常规命令。

格式参考：[Linux NILFS2 文档](https://docs.kernel.org/filesystems/nilfs2.html)、
[固定 on-disk 定义](https://github.com/torvalds/linux/blob/7a78f7ba9f24962c8033e0152bf34a7903ff61ba/include/uapi/linux/nilfs2_ondisk.h)。
