# Changelog

本文件记录 LovelyCarve 的重要变更。

格式遵循 [Keep a Changelog 0.3.0](https://keepachangelog.com/zh-CN/0.3.0/)，
版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。条目以用户可感知的变化为主，按版本倒序排列。

## [Unreleased]

### Added

- 新增中文 `plugin.toml` 配置参考，覆盖字段含义、默认值、允许范围与当前校验限制；实现手动触发后台插件目录检查和诊断，新增插件即时同步分析菜单，保留运行任务与会话；构建和打包资源由插件清单生成，支持声明附带资源，不再写死插件名单。

- CLI JSONL 会话支持 `session --jobs N` 并发查询，保留案件／检材切换屏障与结果引用依赖；命令清单公开并发能力，响应按 ID 关联，便于 AI agent 自动化。

- QCOW 读取补齐 v1、DEFLATE 压缩、RAW/QCOW 差分父链和 legacy AES 密码/原始密钥入口，提供 libqcow 全部 42 API 对照；修复零簇覆盖父镜像、现代 v3 头部与读取边界校验。

- 新增参照 libsmraw 重写的纯 Rust `smraw` vendor 库，支持单文件/分段 RAW 读写；应用和 CLI 从首段读取完整镜像，检查编号缺段与所有源段的稳定性，提供 72 个公开 API 对照与格式限制说明。

### Fixed

- CLI 命令线程等待 Tauri 完成初始化后启动，避免首条插件命令与运行时发现竞争。

- TAR.GZ / TGZ、Docker / OCI 压缩层与小米备份的 GZIP 读取接入纯 Rust `gzipf`，完整读取拼接成员并校验全部尾部，避免忽略后续成员或损坏校验值；GZIP 备份提取前先验证完整压缩流。

## [1.1.0] - 2026-10-13

补齐分区表、卷管理与加密容器的读取能力，接入 FileVault 2 并扩展 APFS 与 HFS+ 读取，与 libyal 系列库逐项对照并修正一批取证读取问题，同时重新整理 README 与文档。

### Added

- 普通磁盘与 RAID 重组镜像顶部提供原始 RAW 导出；已解锁的 LUKS、VeraCrypt 和 BitLocker 分区按明文导出，保留整盘布局与大小。
- BitLocker 支持 AES-256 Diffuser（8001）、Vista、Used Disk Space Only（EOW）与 4 KiB 扇区卷，新增明文密钥、BEK 启动密钥、FVEK / TWEAK 与 VMK 直接解锁，并提供 libbde 能力对照。
- XFS 读取覆盖格式 1–5、目录 v1–v3 的全部布局、扩展属性、BIGTIME 与 metadata UUID，支持文件范围读取与来源区间映射，并记录 libfsxfs 全部 82 个公开 API 的能力对照。
- 补齐 VSS 原始块/存储区间枚举、签名检查与有界 seek/read，增加只读检查入口和 libvshadow 全部 48 个公开 API 的能力对照。
- 补齐 LVM2 的 VG/PV/LV/segment/stripe 元数据与原始 LV 范围读取，支持单 PV 碎片多段卷，提供 libvslvm 全部 69 个公开 API 的能力对照。
- 补齐 GPT 原始槽位、类型/唯一 GUID、属性、UTF-16 名称、主备头校验与有界 seek/read，增加分区探针 `--gpt` 及 libvsgpt 全部 42 个公开 API 对照。
- 补齐 MBR/EBR 原始类型、启动标志、CHS、槽位来源和有边界限制的 seek/read 接口，提供分区探针 `--mbr` 及 libvsmbr 全部 38 个公开 API 的能力对照。
- 补齐 LUKS 的 AES-CBC/ECB、Blowfish、Serpent、ARC4、更多 IV 和 SHA-224/384，提供 LUKS1/2 主密钥解锁入口及 libluksde 全部 36 个公开 API 的能力对照。
- 补齐 FAT/exFAT 文件范围读取、原始属性与目录项偏移等后端元数据接口，并记录 libfsfat 全部 53 个公开 API 的能力对照。
- 补齐 ext2/3/4 内联文件与目录、EA-inode 扩展属性、旧式目录和 meta_bg 读取，并记录完整 libfsext 能力对照。
- ext 读者提供卷挂载历史、feature masks、UID/GID、链接数和设备编号等元数据 API。
- 接入 filevault-core / filevault-forensic：CoreStorage 识别、逻辑卷库存与审计、密码/恢复/VMK 解锁、受检多 extent / 主备元数据，复用原分区解密视图；完整 libfvde 对照见 docs/formats/FILEVAULT.md。
- HFS+ / HFSX 对照 libfshfs：完整 data/resource/attribute 映射、固定 Unicode 规则、硬链接、压缩/随机读取和元数据边界；完整 95 API 对照见 docs/formats/HFS.md。
- APFS 提供直接范围读取与原检材数据区间，补齐容器/卷身份、OID 文件访问、可寻址扩展属性与原生快照元数据 API，并记录 libfsapfs 全部 110 个公开 API 的能力对照；APFS 原生加密解锁和历史快照浏览仍未实现。

### Changed

- 插件 SDK、隔离宿主与 `linuxinfo` 改由独立的 [LovelyCarve-Plugins](https://github.com/LovelyTeams/LovelyCarve-Plugins) 子模块维护；源码构建需递归克隆或运行 `just bootstrap` 初始化子模块。
- RAID 自动重组的标签页与页面标题统一为「RAID N 重组镜像」，完整会话标识移至可复制的详情字段。
- 重写 README 并加入界面截图；整理文档，格式与读取器专题移至 `docs/formats/`。

### Fixed

- VSS 损坏或循环 catalog 明确报错，存储头完整读取并校验；异常 catalog 卷尺寸按 NTFS 主备几何恢复可读范围，保留原始值，长副本链不再依赖递归栈。
- LVM2 验证标签、元数据头及文本 CRC，拼接活动环形缓冲回绕并读取远端备份；修复同大小 PV 身份、4Kn 的固定 512 字节单位、首段截断与未使用 PE 判定，跨镜像缺失/冲突成员不再组装为残缺 LV。
- GPT 优先采用头与数组 CRC 完整的副本，磁盘 GUID 与分区保持同一来源；修复 1024/2048/4096 字节几何、单扇区分区和完整范围校验，超过 256 个可浏览分区明确报错，避免 ID 回绕。
- 修复 Vite 8 干净安装后前端构建因缺少 esbuild 失败，改用内置 Oxc 压缩器。
- Windows 默认构建恢复 MSVC 链接器，移除对额外 LLVM 链接器的强制要求；完整 QR/OCR 构建仍需 libclang。
- 修复含主数据分区的 1024/2048/4096 字节扇区 MBR 几何识别，支持多个扩展入口及分叉 EBR，并校验完整分区/链接范围；循环、损坏和遍历限额提供告警，分区 ID 不再回绕。
- LUKS 密钥材料按扇区补齐，LUKS2 密钥槽支持与卷主密钥不同的长度；解密 seek 溢出、无效 PBKDF2 轮数和不合法的算法组合明确报错。
- 修复 exFAT 文件初始化尾部、负时区与 10 毫秒时间精度、跨目录槽位 UTF-16 名称及连续子目录的删除项扫描。
- FAT 按数据簇数识别格式并支持大逻辑扇区；FAT/exFAT 读取校验损坏簇链与短读，保留同名删除记录，并避免把未知本地时区标成 UTC。
- ext 文件范围与流式读取统一完整间接块映射，未写入 extent 读零，越过卷边界或来源短读时报错。
- 修复 ext signed/扩展 epoch/纳秒时间戳及 birth 投影、32768-block extent 长度、64 KiB 目录记录和旧组描述符 CRC16 校验。
- 安装包、便携包与 CLI 包随附 VC++ 运行库（`vcruntime140.dll`、`vcruntime140_1.dll`、`msvcp140.dll`、`msvcp140_1.dll`），未安装 Visual C++ 运行库的电脑也能启动主程序与 CLI、加载 OpenSSL 并启动插件宿主。
- APFS 对象映射遵守检查点/卷事务上限与删除记录；完整、流式与范围读取统一校验，正确处理显式空洞，并拒绝缺失尾部、重叠、溢出、截断与越过容器的数据。
- APFS / HFS+ 共享压缩解码器支持 LZFSE 的原样存储块，并校验 RAW resource block 标记与偏移表边界，避免将损坏数据当作文件正文。
- APFS 支持两类目录键及卷内大小写/Unicode 规范化查找；独立 APFS 分区镜像可识别，更大 superblock 按完整块校验，目录索引保留压缩文件逻辑长度和符号链接目标。

## [1.0.0] - 2026-10-12

首次公开源码发布，对应 Git 标签 `v1.0.0`，采用 AGPL-3.0-only 许可证。桌面开发与发布配方面向 Windows x64。
当前案件格式不兼容早期开发版的 `.carve` 案件，需要新建案件。

### Added

- 中文桌面工作区，提供浅色、深色、樱花色主题及可配置的界面动画。
- 案件创建、打开与最近案件记录，支持多检材登记、文件标签和操作审计。
- 卷目录快照、文件索引及分析结果持久化；原镜像缺失时，可通过已验证的索引浏览目录和元数据。
- RAW、E01/EWF、VMDK、VHD/VHDX、QCOW2、VDI、DMG、AD1 镜像及 Windows 物理磁盘的只读访问。
- 文件夹、归档与 iOS 本地备份检材，支持 Docker/OCI 镜像及磁盘中 Docker 容器的合并文件视图。
- GPT、MBR/EBR、APM、BSD 分区识别，LVM 逻辑卷浏览，以及 mdadm 成员检测与 RAID0/RAID5 重组。
- NTFS、Ext4、FAT16/32、exFAT、APFS、HFS+/HFSX、Btrfs、ZFS、XFS、UFS/UFS2、SquashFS、ReFS 文件系统读取。
- NTFS 命名流、LZNT1/WOF 压缩文件读取，以及 ReFS 文件快照流浏览。
- BitLocker、LUKS、VeraCrypt 和加密 iOS 本地备份解锁，建立按需读取的只读解密视图。
- NTFS 卷影副本枚举与只读历史会话，可浏览快照时刻的文件内容。
- 目录树、分页文件列表、排序与筛选，按文件系统能力展示删除项、未分配空间、符号链接及元数据节点。
- 文本、Hex、图片、音视频、PDF、办公文档及归档等原类型预览；文件预览可拆出为独立窗口并重新停靠。
- SQLite、Realm 数据库与 PE、ELF、Mach-O、GGUF 等结构化预览，以及 SQLite 删除记录恢复。
- 镜像与文件的 MD5、SHA-1、SHA-256、SM3 校验，文件、目录、ZIP、RAW 镜像及解密镜像导出。
- 多编码关键词与正则全局搜索，支持范围筛选、任务历史、分页命中、源文件定位及 CSV 导出。
- 文件修改时间、插件事件与 NTFS USN 日志的时间线投影，提供热力图、趋势图、筛选和导出；可查看 NTFS USN 与 Ext4 日志。
- 图片、音频、归档、PDF、SQLite 等格式的结构化文件雕刻，支持暂停、继续、取消、结果恢复及 SHA-256/DFXML 输出。
- LPP 动态库插件接口与独立进程宿主，内置 `linuxinfo`，分析 Linux 系统、用户、登录、网络、服务、软件与定时任务等信息。
- 任务中心统一展示分析与后台任务的进度、控制操作和错误，分析结果支持分页、详情、源文件定位与 CSV 导出。
- BCP 索引与字段说明、按数据集分类的分页浏览、当前页筛选及来源文件查看。
- 独立 `lovelycarve-cli`，支持交互式证据浏览、哈希、导出、分析命令及 JSONL 自动化会话。
- Windows Dokany 只读挂载，可将支持的卷与 Docker 合并视图挂载为盘符或文件夹。
- 可选二维码识别与 OCR 构建；默认轻量构建关闭这些功能，完整构建通过 `*-full` 配方启用。
- 源码构建、Windows 安装包与便携包、CLI 和插件的本地打包配方，以及使用、开发、接口和第三方来源说明。

[Unreleased]: https://github.com/LovelyTeams/LovelyCarve/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/LovelyTeams/LovelyCarve/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/LovelyTeams/LovelyCarve/releases/tag/v1.0.0
