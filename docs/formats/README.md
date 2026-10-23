# 格式与读取器

LovelyCarve 用纯 Rust 读取镜像容器、分区表、文件系统和加密容器，证据始终只读打开。本目录记录各格式的实现范围、已知限制和验证方法；修改读取器之前先读对应专题和下文的维护要求。适配器、证据会话与索引的整体结构见 [架构说明](../ARCHITECTURE.md)，第三方代码的来源和本地补丁见 [vendor/UPSTREAM.md](../../src-tauri/vendor/UPSTREAM.md)。

多数专题以 [libyal](https://github.com/libyal) 对应库的固定提交为参照，逐项对照格式能力和公开 API。这类对照说明“实现了什么、刻意不同在哪里”，不等于两个实现已在所有真实镜像上逐字节比对；未覆盖的场景在各文档的“验证”或“限制”部分单独列出。

## 专题

表格按文件系统、分区与阵列、加密与快照、镜像容器、压缩与备份的顺序排列。

| 格式 | 文档 | 要点 |
| --- | --- | --- |
| NTFS | [NTFS.md](NTFS.md) | libfsntfs 对照；MFT 与属性链、LZNT1 / WOF 压缩、命名流、硬链接、重解析点、安全描述符、独立 `$MFT`、USN 日志 |
| ext2/3/4 | [EXT.md](EXT.md) | libfsext 对照；内联数据、EA-inode、旧式目录、`meta_bg`、全部间接块层级、带符号与纳秒时间戳 |
| FAT / exFAT | [FAT.md](FAT.md) | libfsfat 对照；按簇数判定 FAT 类型、大逻辑扇区、范围读取、ValidDataLength、时区与删除项 |
| XFS | [XFS.md](XFS.md) | libfsxfs 对照；格式 1–5、目录与属性全部布局、BIGTIME、metadata UUID、范围读取与来源映射 |
| ReFS | [REFS.md](REFS.md) | 纯 Rust `refs-reader`；文件流快照、CoW 共享块、公开镜像来源与独立解析器验证 |
| APFS | [APFS.md](APFS.md) | libfsapfs 110 API 对照；事务 OMAP、多卷、两类目录键、克隆/空洞、范围读、xattr 与快照元数据；明确加密/历史浏览边界 |
| 传统 HFS | [CLASSIC_HFS.md](CLASSIC_HFS.md) | fstool / rusty-backup 改造；catalog / overflow、MacRoman、data / resource fork、本地时间和范围读 |
| HFS+ / HFSX | [HFS.md](HFS.md) | libfshfs 95 API；完整 fork / overflow、resource fork、attr continuation、Unicode 3.2、硬链接与逻辑范围读取 |
| ISO 9660 / 光盘 BIN / CUE | [CDFS.md](CDFS.md) | libodraw 全 API 对照；多文件/多轨 CUE、Mode 1 / XA Form 1、来源字节保留与范围读取 |
| UDF | [UDF.md](UDF.md) | lamfold-udf；1.02–2.01 单物理分区、FE / EFE、tag / CRC、续链、空洞和范围读取；高级 partition maps 明确拒绝 |
| CramFS / Linux ROMFS | [ROM_IMAGES.md](ROM_IMAGES.md) | lamfold Rust 库；CramFS v2 大小端、CRC、按页范围读取；ROMFS 校验和、硬链接、目录链边界 |
| JFS（Linux JFS2） | [JFS.md](JFS.md) | rusty-backup / partmgr 改造；多 IAG、dtree / xtree、分配位图、目录索引、EA / 符号链接与纳秒范围读 |
| ReiserFS v3 | [REISERFS.md](REISERFS.md) | 3.5 / 3.6、混合键、多层树、尾部打包、稀疏块、硬链接与范围读；dirty 拒绝，Reiser4 独立未支持 |
| Amiga OFS / FFS | [AMIGA.md](AMIGA.md) | DOS0–7、Latin-1 / LNFS、硬链接、原生 RDB 与多块几何；dirty / BADB 明确拒绝 |
| BeFS | [BEFS.md](BEFS.md) | BeOS / Haiku、大小端、四种块大小、B+tree、全部 run 层级及扩展属性；原生官方镜像 / 系统包验证 |
| GFS2 | [GFS2.md](GFS2.md) | 1801 / 1802、大端、rindex / 分配位图、exhash、定高文件树、稀疏与 EA；检查全部日志并明确拒绝重放 |
| OCFS2 | [OCFS2.md](OCFS2.md) | 分配链 / 位图、extent / refcount、内联及索引目录、稀疏 / unwritten 与属性；全部 slot 必须离线干净 |
| NILFS2 | [NILFS2.md](NILFS2.md) | 双超级块 / 日志 CRC、DAT / 分配位图、direct / B-tree、检查点与 GC 搬移；应用读取当前视图，独立库可读历史 snapshot |
| Minix FS V1 / V2 / V3 | [MINIX.md](MINIX.md) | rusty-backup 元数据改造；五种布局、大小端、位图 / zone 边界、全部间接层级与有界范围读；独立于 MINIFS |
| MINIFS | [MINIFS.md](MINIFS.md) | 原创独立 Rust 库；VxWorks / TP-Link v3、共享 LZMA 块、有界范围读及官方固件逐文件 oracle |
| F2FS | [F2FS.md](F2FS.md) | 纯 Rust checkpoint / NAT 读取、动态内联区域、完整间接寻址、inode CRC、硬链接及纳秒时间 |
| EROFS | [EROFS.md](EROFS.md) | lamfold-erofs；紧凑 / 扩展 inode、四种压缩、chunk 空洞、有界范围读取与超级块 CRC32C |
| QNX4 / QNX6 | [QNX.md](QNX.md) | 原创 Rust 库；QNX4 extent 链、QNX6 Power-Safe / MMI、大小端、代际 CRC、间接树及 QNX 原生镜像 TAR oracle |
| SquashFS | [SQUASHFS.md](SQUASHFS.md) | backhand Rust 读取器；v1/v2 旧布局、v3/v4、大小端、尾片段和有界 LZMA / 范围读取 |
| JFFS2 | [JFFS2.md](JFFS2.md) | jffs2-rs 结构 / RTIME 改造；新格式大小端、CRC、版本化目录与片段、空洞和范围读取 |
| YAFFS1 | [YAFFS1.md](YAFFS1.md) | 原生 Rust 分散 tags、标签 / 页正文 ECC、两位序号、删除页、shadow、硬链接及独立文件长度策略 |
| YAFFS2 | [YAFFS2.md](YAFFS2.md) | Binwalk Rust 前缀改造；page / spare、packed tags / ECC、版本与 shrink / shadow、硬链接、稀疏和范围读取 |
| UBIFS | [UBIFS.md](UBIFS.md) | rust_ubidump 节点/索引适配；干净的已提交 v4/v5 树、CRC、硬链接/空洞与范围读；明确日志、加密和恢复限制 |
| GPT | [GPT.md](GPT.md) | libvsgpt 对照；主备头与数组 CRC、扇区大小推断、备份恢复、原始槽位与属性 |
| MBR / EBR | [MBR.md](MBR.md) | libvsmbr 对照；原始槽位与 CHS、多扩展入口、循环与分叉检测、大扇区几何 |
| LVM2 | [LVM.md](LVM.md) | libvslvm 对照；标签与元数据 CRC、活动副本、环形缓冲回绕、碎片线性卷映射 |
| Linux md RAID | [MDRAID.md](MDRAID.md) | 元数据 0.90 / 1.0 / 1.1 / 1.2；linear 与 RAID0/1/4/5/6/10 全部布局；陈旧成员、降级重建与 RAID6 双盘恢复；md 上的 LVM |
| 固件 RAID | [FIRMWARE_RAID.md](FIRMWARE_RAID.md) | Intel IMSM（RST / VROC）与 SNIA DDF（PERC、MegaRAID、Adaptec、mdadm ddf）；校验和与 CRC、主备副本、按序列号或引用号定位成员；RAID0/1/4/5/6/10、RAID1E 与串联；降级规则同 md |
| LUKS | [LUKS.md](LUKS.md) | libluksde 对照；LUKS1/2、AES / Serpent / Blowfish / ARC4、IV 方案、主密钥解锁 |
| BitLocker | [BITLOCKER.md](BITLOCKER.md) | libbde 对照；Vista、Windows 7+ 与 To Go 卷，六种加密方法、EOW 与各种解锁方式 |
| FileVault 2 | [FILEVAULT.md](FILEVAULT.md) | filevault-forensic 接入与 libfvde 88 API；多 LV、密码/恢复/VMK、主备元数据、segment map 与 HFS 明文视图 |
| 卷影副本 | [VSS.md](VSS.md) | libvshadow 对照；存储区链重建、原始块枚举、catalog 校验与尺寸恢复 |
| RAW / 分段 RAW | [SMRAW.md](SMRAW.md) | libsmraw 72 API 对照；分段发现、随机读写、缺段/源变化检查；读取接入宿主 |
| QCOW / QCOW2 | [QCOW.md](QCOW.md) | libqcow 全部 42 API；v1/2/3、DEFLATE、RAW/QCOW 父链、legacy AES 与完整性检查 |
| Parallels PHDI | [PHDI.md](PHDI.md) | libphdi 56 API 对照；纯 Rust v2 Plain / Sparse / Split / 差分父链、XML 元数据及完整性检查 |
| UBI / HDR1 固件 | [UBI.md](UBI.md) | rust_ubidump 只读库；小米扩展分段表、UBI 逻辑块重组、原始卷读取与内嵌 SquashFS / UBIFS 浏览 |
| 路由器固件 | [FIRMWARE.md](FIRMWARE.md) | Buffalo ENC、TRX、TP-Link SafeLoader、U-Boot uImage、Netgear CHK、Sercomm NAND ZIP；来源范围与解压逻辑视图分离 |
| GZIP | [GZIP.md](GZIP.md) | libgzipf 42 API 对照；纯 Rust 多成员、元数据、校验、流式/seek；接入 tar.gz / Docker 层 / 小米备份 |
| Android 备份 | [ANDROID_BACKUP.md](ANDROID_BACKUP.md) | `.ab` 1–5 与小米 `.bak` 独立打开；原始容器字节与逻辑文件树分离 |
| 小米备份 | [MIUI_BAK.md](MIUI_BAK.md) | ZIP / 目录内 `.bak` 自动展开；Android Backup 头、TAR 与压缩流 |
| iOS 备份 | [IOS_BACKUPS.md](../IOS_BACKUPS.md) | 本地备份浏览、加密备份解锁与异常检查 |

其余格式（Btrfs、ZFS、UFS，以及 EWF、VMDK、VHD / VHDX、VDI、DMG、AD1 等镜像容器）的实现位置见 [架构说明的镜像与证据源](../ARCHITECTURE.md#镜像与证据源)，上游版本、本地补丁与测试样本记录在各 vendor 目录的 `LOVELYCARVE.md` / `ORIGIN.md` 中。命令行下的镜像打开、解锁和导出示例见 [CLI 手册](../CLI.md)。

## 维护要求

### 时间戳

保留源格式的时间角色、精度和时区依据。没有偏移量的本地时间不能标成 UTC：FAT 本地时间、exFAT UTC offset、NTFS FILETIME 和 APFS 纳秒值需要分别处理；NTFS 的标准信息与文件名属性时间独立保留。

测试覆盖零值、未设置值、合法边界、时区偏移和精度转换。格式化字符串只用于展示，排序与投影使用数值时间；时间线的投影规则见架构说明。

### 目录与删除项

遇到删除或损坏的目录项时，不能提前结束后续有效项的枚举。区分现存目录项、原生删除项、结构残留和雕刻候选；删除项的目录身份与数据块可恢复性分别验证。

恢复读取按文件逻辑长度裁剪，不把簇尾空间拼入正文。重用记录、同名候选、多个父目录和符号链接需要显式处理，不猜测缺失的父链。独立的文件系统孤儿扫描尚未接入。

### 多卷与派生视图

APFS 容器内卷、Btrfs 子卷、LVM 逻辑卷和 Docker 合并卷各自保留卷身份，不能只取第一个卷，也不能在缓存键中混用身份。命名流、扩展属性与文件系统元数据节点要有明确的读取路径和来源范围。

加密视图和卷影副本会话依赖源镜像，卸载时一并释放其会话、索引、预览、插件证据句柄和挂载。

### 读取与压缩

支持的压缩结构解码为文件逻辑字节，不支持的编码明确报错。`Volume::read_range`、完整读取和流式读取必须返回相同的字节；不能随机寻址的适配器可以回退到流式读取，但代价是 O(offset)。NTFS、ext2/3/4、FAT / exFAT、XFS、ReFS、APFS、HFS+ / HFSX 和连续镜像区间支持直接范围读取，Dokan 挂载的缓存未命中复用这条路径。

文件大小、extent / run / cluster 范围及乘加运算都需要校验。短读不能冒充完整文件；整盘哈希、RAW 导出和解密导出必须检查 `offset == total`。格式定义的空洞、未写入区间和未初始化尾部读零；已分配数据越过卷边界或来源被截断时报错。

ext 的内联数据需要拼接 `i_block` 与 inode 内的 `system.data`，内联目录的两段记录不能跨边界；所有层级的间接块与 extent 使用同一份逻辑片计划。NTFS 的默认流和命名流经同一逻辑读者处理 LZNT1 与 WOF，未初始化尾部按 NTFS 规则读零，EFS 加密内容明确拒绝。

`just run-example probe_ntfs_stream <镜像> <分区ID> <MFT记录号> [对照文件]` 可以核对 NTFS 的完整、流式与范围读取；对照文件应由独立实现生成。

### 验证矩阵

| 场景 | 检查 |
| --- | --- |
| 完整、截断及损坏镜像 | 分区边界、错误上下文、坏区报告与探测兜底 |
| 多卷与多镜像 | 卷键、节点身份、缓存隔离与卸载清理 |
| 时间与编码 | 数值、角色、偏移、精度与非 ASCII 名称 |
| 删除、稀疏与压缩文件 | 逻辑长度、字节校验值、状态与来源 |
| 全量、流式与范围读取 | 不同块边界下逐字节一致 |
| 取消与资源上限 | 输出清理、终态与资源释放 |
| 只读挂载 | 在 Windows 上实际挂载，核对读回哈希、随机读取、只读保护与卸载清理 |

优先使用逐字段构造的最小合成样本；公开样本记录来源、许可证和哈希，不提交真实检材。可选的外部样本测试通过环境变量开启，未设置时跳过，不能计为已验证。对应 vendor 读取器用 `just test-*` 或 `just test-vendor <目录>` 测试，配方列表见 [开发指南](../DEVELOPMENT.md)。
