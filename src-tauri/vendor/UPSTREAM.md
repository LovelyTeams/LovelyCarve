# Vendored upstream audit

核对日期：2026-10-12。核对上游默认分支、已声明的来源分支和 crates.io 发布版本。

更新保留 LovelyCarve 的只读接口、边界检查、线程保护、取消支持及独立测试夹具。
表中的提交是核对或选用的源码基准；本地移植/兼容补丁并不等于整个上游仓库。
同一版本的上游维护改动也按源码核对；不通过修改版本号伪造升级。

仓库只保留当前构建使用的依赖源码。移除随已删除分析插件失去用途的 EVTX 库，以及 vendor 中不在本仓库运行的上游 GitHub Actions；保留其余读取器的许可证、来源和回归样本。

| 目录 | 当前版本 | 上游提交 / 来源 | 本次处理 |
| --- | --- | --- | --- |
| `lamfold` / `lamfold-cramfs` / `lamfold-romfs` | `0.1.5` / `0.1.0` / `0.1.1` | crates.io 包；core/ROMFS [fabaad6af0c2](https://github.com/lamco-admin/lamfold/commit/fabaad6af0c28b80535307b339b84110f3a3c12e)，CramFS [44f0343b2487](https://github.com/lamco-admin/lamfold/commit/44f0343b2487f65cfb84508bff375fdda18c18a5) | 2026-10-21 先独立验证再复用；CramFS v2 大小端、CRC、按页范围读取；ROMFS 声明长度/校验和、链循环检查；本地补丁与限制见各 LOVELYCARVE.md 和 [ROM_IMAGES](../../docs/formats/ROM_IMAGES.md)。 |
| `jffs2` | `0.2.1` | [52e1234ce66b](https://github.com/xmoezzz/jffs2-rs/commit/52e1234ce66b0e099b35afabff3fc3e3078a2c0e) | 复用 Rust 节点 / RTIME 并改为有界库；修正 CRC、大小端、硬链接与版本化片段，不引入 C LZO / Rubin；范围见 [JFFS2](../../docs/formats/JFFS2.md) 和 LOVELYCARVE.md。 |
| `yaffs2-reader` | `0.1.0` | 本仓库原创；Rust 前缀参考 binwalk-ng [845806c16374](https://github.com/binwalk-ng/binwalk-ng/blob/845806c163747eb524d66b5fe3de2ed86ea3aaad/src/formats/yaffs.rs) | AGPL-3.0-only；复用的 MIT Rust 结构保留独立版权与许可，补 YAFFS2 page / spare、tags / ECC、版本、shrink / shadow、命名空间和范围读取。YAFFS1 扩展独立实现分散 tags、两位 serial、页正文 ECC 与显式长度策略，共用 Rust 目录接口。没有引入 C 或外部提取入口；原始夹具 CC0，范围见 yaffs2-reader/LOVELYCARVE.md。 |
| `ubidump` | `0.1.0` | [19782c3eabc2](https://github.com/aspnmy/rust_ubidump/commit/19782c3eabc25a1baab7faefe12fe6f4bd362dac) | UBI 层改为有界只读库，修正 VTBL 长度与名称边界，重写逻辑块映射及校验；增补有界 UBIFS v4/v5 已提交索引、主节点副本和按块文件读取，复用 Rust 解码库，不引入 CLI / C LZO。见 LOVELYCARVE.md 与 [UBI](../../docs/formats/UBI.md) / [UBIFS](../../docs/formats/UBIFS.md)。 |
| `ad1` | `0.1.2` | [afc3963659a1](https://github.com/SecurityRonin/ad1-forensic/commit/afc3963659a13acf10082e7d76194d3ab789583e) | 来源记录与上游最新提交一致，保留现有本地补丁。 |
| `apfs` | `0.4.1` | [3a73ee28fdab](https://github.com/Dil4rd/dpp/commit/3a73ee28fdab192fcdfbe8edcad7b520867f2417) | 保留多卷、MACB 和目录遍历；2026-10-13 对照 libfsapfs 补事务 OMAP、目录键、seek/range、inode/xattr/快照元数据及严格来源边界。完整范围与限制见 [APFS](../../docs/formats/APFS.md)。 |
| `sercomm-firmware` | `0.1.0` | 本仓库原创 Rust（AGPL-3.0-only） | 先复用 zip / flate2 / ubidump；验证 Sercomm ZIP32 NAND 布局、长度、CRC、EOF 与取消，宿主通过独立 native view 保留源身份 / 压缩几何；格式依据与边界见 sercomm-firmware/LOVELYCARVE.md。 |
| `backhand` | `0.25.5` | [bf678c7f9237](https://github.com/wcampbell0x2a/backhand/commit/bf678c7f923799a57d972cf695daf69d8c6ddafb) | 升级 0.25.1 → 0.25.5；保留精简特性、std I/O 别名及可读错误信息，修复 Windows 根路径判断；2026-10-21 对照 FMK c72f45d7a062 补 DD-WRT v3 hsqt/tqsh、逐块 LZMA 参数与 v3/v4 精确范围读取；v3 adaptive / 标准 LZMA 复用 lzma-rust2 并去除 adaptive C/C++ 依赖，补终止、EOF 与输出限制，另补 v1/v2 有界大小端读者、尾片段、跨元数据块及有符号 / 缺失时间；无 EOS 块以 Rust lzma-rs 交叉验证最终 range code，另将 XZ / Zstandard 读取改用 lzma-rust2 / ms-compress-ruzstd，补完整输入、校验和、索引和内存上限；C Zstandard 写入仅保留为宿主不启用的独立特性。见 backhand/LOVELYCARVE.md。 |
| `lzma-rs` | `0.3.0` | [da82bd118299](https://github.com/gendx/lzma-rs/commit/da82bd1182993a07db035e285a8f8a87d32a30c2) | 发布包保留原始 MIT 来源；补显式严格 LZMA 完成接口，校验已知输出长度后的 range code 与真实 EOF，供旧 SquashFS 无 EOS 块独立核对；未引入 C 开发依赖，见 LOVELYCARVE.md。 |
| `bitlocker` | `0.3.5` | [e6674dcda241](https://github.com/SecurityRonin/bitlocker-forensic/commit/e6674dcda2413b1a02a915f2255af8fda787a154) | 2026-10-13 对照 libbde 补六种 AES、变长扇区、Vista/EOW、BEK/clear/VMK/FVEK 和 metadata 查询；验证边界见 [BITLOCKER](../../docs/formats/BITLOCKER.md)。 |
| `btrfs-diskformat` | `0.5.1` | [bb84745422ed](https://github.com/GodTamIt/btrfs-diskformat/commit/bb84745422ed2c2aa99b3b151f6247a3b51638f4) | 已最新；保留 zerocopy Immutable 兼容补丁。 |
| `cdfs` | `0.2.3` | [8cc434a31983](https://git.sr.ht/~az1/iso9660-rs/commit/8cc434a319832ae43d1c7685477809d75f313990) | 发布版本及核心解析器已最新；保留现有独立 crate 布局。 |
| `cmpfs` | `0.1.0` | [3a73ee28fdab](https://github.com/Dil4rd/dpp/commit/3a73ee28fdab192fcdfbe8edcad7b520867f2417) | 共享 Apple decmpfs 解码器；libfsapfs 对照补 LZFSE stored marker、RAW 标记与资源偏移表边界，供 APFS/HFS+ 使用。 |
| `ewf` | `0.4.10` | [c456f993f683](https://github.com/SecurityRonin/ewf-forensic/commit/c456f993f68385f3438b3ff16ba5ab6fd0850fb4) | 来源记录与上游最新提交一致，保留现有本地补丁。 |
| `exfat` | `0.1.0` | [b930023e0636](https://github.com/obhq/exfat/commit/b930023e06363371b1f26df1f45e17eaa0c7a290) | 保留删除项修复；本地 VDL 尾部、严格簇链、原始属性/extent、UTF-16 与时间补丁见 [FAT](../../docs/formats/FAT.md)。 |
| `ext4-view` | `1.0.0` | [8facbf4c2986](https://github.com/nicholasbishop/ext4-view-rs/commit/8facbf4c2986b7e2dbdef92514d696f1559a2b90) | 升级 0.9.3 → 1.0.0；保留 inode 索引 API、128 字节 inode 校验和、兼容旧 journal；本地内联数据、EA-inode、meta_bg 等能力补丁见 [EXT](../../docs/formats/EXT.md)。 |
| `fatfs` | `0.4.0` | [2aefc2a027ce](https://github.com/rafalh/rust-fatfs/commit/2aefc2a027ce94ed0671752814dac203f0450e11) | 保留显式 root_dir 生命周期；本地目录项偏移、簇链防环、有限 extent 与大扇区补丁见 [FAT](../../docs/formats/FAT.md)。 |
| `gpt` | `4.1.0` | [73086deff58c](https://github.com/Quyzi/gpt/commit/73086deff58cb3c741b2ba51a53a2a7c6231a3f1) | 同步上游整数边界测试修正；保留精简测试依赖。 |
| `hfsplus` | `0.4.0` | [3a73ee28fdab](https://github.com/Dil4rd/dpp/commit/3a73ee28fdab192fcdfbe8edcad7b520867f2417) | 2026-10-13 对照 libfshfs 95 API：完整 fork 与属性续接、固定名称规则、硬链接、逻辑压缩读取及严格 B-tree / 来源校验；Apple 表 APSL 归属见 LOVELYCARVE.md。 |
| `ios-backup` | `0.1.0` | [8d8cef447ae2](https://github.com/SecurityRonin/ios-backup-forensic/commit/8d8cef447ae212e7dd22dc0425156bf4ddfcfa05) | 来源记录与上游最新提交一致，保留现有本地补丁。 |
| `luks` | `0.1.8` | [284f06487d7a](https://github.com/SecurityRonin/luks-forensic/commit/284f06487d7a66f13ed27c61be5e26f0863c0e4f) | 来源记录与上游最新提交一致，保留现有本地补丁。 |
| `magika` | `1.1.1-dev` | [26b6a9ba7e92](https://github.com/google/magika/commit/26b6a9ba7e92f2b0a3745970a9190ec0dde9bf83) | 同步最后的 ORT 兼容源码（RAR MIME 修正）；上游 2.0.0-dev 已迁至 tract 且未发布，本次不迁移推理运行时，也不降级至 1.1.0。 |
| `notatin` | `1.0.1` | [ae3a70611bc3](https://github.com/strozfriedberg/notatin/commit/ae3a70611bc37b2ca04637671ceb14ccab9d5a87) | 同步默认分支的 nom 8 / thiserror 2 解析器和版权更新；保留独立 crate 布局。 |
| `ntfs` | `0.4.0` | [47ae0168f6c6](https://github.com/ColinFinck/ntfs/commit/47ae0168f6c607208b27c61ed89b4ffac9a3c99a) | 默认分支运行时代码已最新；保留损坏 attribute-list 迭代终止修复；按属性类型在解引用前过滤，避免重解析查询展开无关的过期引用。 |
| `qcow2` | `0.1.3-lovelycarve` | [3ed3ff55b96a](https://github.com/vasi/qcow2-rs/commit/3ed3ff55b96a2f519216ace7b3a02ac29bf8ba65) | 2026-10-13 对照 libqcow 全部 42 API；原创 Rust v1/2/3、DEFLATE、RAW/QCOW 父链、legacy AES、元数据/seek/cancel；边界及许可证见 LOVELYCARVE.md 与 [QCOW](../../docs/formats/QCOW.md)。 |
| `realm-db-reader` | `0.2.1` | [e6c90c65c04b](https://github.com/maartenstaa/realm-db-reader/commit/e6c90c65c04b1e3fcfef796571a664bfa98e2f5b) | 发布版本和运行时代码已最新；保留内存读取及空表补丁。 |
| `refs-reader` | `0.1.0` | [391c1bd56986](https://github.com/b3nguang/refsprogs/commit/391c1bd56986142fc359288b7d2cb41cb3eb3680) | 核对 feature/build-librefs-dll；指定来源分支未变化。Rust 移植不能直接覆盖为 C 源码。 |
| `rsqlite-vfs` | `0.2.0` | [3d8318430bac](https://github.com/Spxg/sqlite-wasm-rs/commit/3d8318430bacd7d6a1aafdf5b5e54fb0fee8d451) | 升级 0.1.1 → 0.2.0；文件表、文件内容和错误槽使用 Arc/Mutex，注册操作串行化；应用改用新导入 API。 |
| `rufs` | `0.6.0` | [36bc35a5fb9e](https://github.com/realchonk/fuse-ufs/commit/36bc35a5fb9e05e214fa3d9619822763d7d8cb9b) | 同步 bincode-next 3.1.1 和 MSRV 1.90；保留 Windows 只读移植和解码器 API。 |
| `sqlite-core` | `0.12.0` | [df511309e086](https://github.com/SecurityRonin/sqlite-forensic/commit/df511309e086825fb194ea8a721e94bbd0033fe0) | 来源记录与上游最新提交一致，保留现有本地补丁。 |
| `udif` | `0.4.2` | [3a73ee28fdab](https://github.com/Dil4rd/dpp/commit/3a73ee28fdab192fcdfbe8edcad7b520867f2417) | 升级 0.3.4 → 0.4.2；采用上游有边界的块解码及纯 Rust XZ。 |
| `vdi` | `0.2.0` | [f1f88806c260](https://github.com/cohaereo/vdi-rs/commit/f1f88806c26042de342d1fbcb7f286590686e2fc) | 发布版本已最新；保留稀疏块、边界检查和 positioned I/O 重写。 |
| `veracrypt` | `0.2.4` | [e3883a5b347f](https://github.com/SecurityRonin/veracrypt-forensic/commit/e3883a5b347f50b51de70ca08655b2fd4b0885b2) | 来源记录与上游最新提交一致，保留现有本地补丁。 |
| `vhd` | `0.1.4` | [2835523fe39c](https://github.com/SecurityRonin/vhd-forensic/commit/2835523fe39c16740133dc8d51932cad46bcc4b6) | 来源记录与上游最新提交一致，保留现有本地补丁。 |
| `vhdx` | `0.4.1` | [b03f1f5c1be9](https://github.com/SecurityRonin/vhdx-forensic/commit/b03f1f5c1be960d79af73f895017169171c00996) | 来源记录与上游最新提交一致，保留现有本地补丁。 |
| `vmdk` | `0.8.4` | [8a9831125c6d](https://github.com/SecurityRonin/vmdk-forensic/commit/8a9831125c6db5cff9a24414481037487a92d410) | 来源记录与上游最新提交一致，保留现有本地补丁。 |
| `vsc` | `0.2.2` | [0af70bed6910](https://github.com/SecurityRonin/vsc-forensic/commit/0af70bed6910fef542224ecd94928b4bc0ae920b) | 2026-10-13 对照 libvshadow 全部 48 个 API；补块/区间枚举、seek/read、严格 catalog/存储头与 NTFS 尺寸校验；真实双副本验证及限制见 [VSS](../../docs/formats/VSS.md)。 |
| `xfs-fuse` | `0.6.0` | [2ea4e250db00](https://github.com/KhaledEmaraDev/xfuse/commit/2ea4e250db00d3f0f845499c8a83f85b9a77b7f8) | 合入上游 v1 inode / real-time 标记；本地 XFS 1–5、目录、属性、checked extent/范围读取、纳秒时间与 meta UUID 补丁见 [XFS](../../docs/formats/XFS.md)。无独立 real-time 设备时拒绝正文。未引入上游 FUSE 服务或 libfsxfs C 实现。 |
| `zfs` | `0.1.0` | [原来源](https://github.com/Hopffenn/zfs-rs) | 原来源返回 Repository not found；保留现有稳定 Rust 移植，不能声称已同步无法访问的上游。 |

`leveldb-rs-binding` 是已移除 LevelDB/MMKV 功能后遗留的空目录，没有被 Git 跟踪的源码或 Cargo 依赖，不属于待更新库。

## 验证入口

- 后端：`just fmt`、`just check`、`just clippy`、`just test --jobs 1`。Windows 页面文件不足时避免并行编译全部示例。
- 独立库：`just test-vendor <目录名>`；已有专用 reader 配方仍优先使用。
- ext4 原始夹具来自上游并校验 SHA-256，详见 `ext4-view/test_data/README.md`；测试通过 Rust zstd 在内存解码，不依赖系统 zstd 程序。
- 可选外部镜像/oracle 未提供时，忽略的测试不代表真实镜像已验证。

| 新增 vendor | 来源 | 本地范围 |
| --- | --- | --- |
| `phdi` | [libyal/libphdi fb132dceb2a1](https://github.com/libyal/libphdi/commit/fb132dceb2a1a43c2597b10bbf9e65757d08cf7c)，能力 / 格式参照；原创 Rust 0.1.0 / AGPL-3.0-only | v2 XML、Plain / Sparse / Split / 差分父链、可取消读取、seek / 元数据 / 限额；只读接入宿主与 CLI，无 C/DLL。56 API 对照、公开单 extent 验证与限制见 [PHDI](../../docs/formats/PHDI.md) 和目录内 ORIGIN.md。 |
| `smraw` | [libyal/libsmraw adfa6b8bc861](https://github.com/libyal/libsmraw/commit/adfa6b8bc8617226805926180244c774366aad30)，公开能力参照；原创 Rust 0.1.0 / AGPL-3.0-only | 单文件/分段 RAW I/O、数字/字母发现、缺段与稳定性检查、取消、句柄上限、新输出写入和调用者元数据；显式强行模式提供连续前缀、短读诊断、越界清零与 Windows 单次临时重试，宿主审计补零。无 C/DLL，不解析 .info。读取接入宿主；72 API 对照见 [SMRAW](../../docs/formats/SMRAW.md) 与目录内 ORIGIN.md。 |
| `gzipf` | [libyal/libgzipf d1b4e6ba5911](https://github.com/libyal/libgzipf/commit/d1b4e6ba5911e1b86a3f9d4ca3fd31f1d69a6b68)，公开能力参照；原创 Rust 0.1.0 / AGPL-3.0-only | GZIP 流与多成员元数据读取、CRC/长度校验、seek、限额和取消；不含上游 C 源码或 DLL，接入 tar.gz / tgz、Docker / OCI GZIP 层与小米备份。42 API 对照与验证边界见 [GZIP](../../docs/formats/GZIP.md)、目录内 ORIGIN.md。 |
| `filevault` (`core` / `forensic`) | [857e04033e09](https://github.com/SecurityRonin/filevault-forensic/commit/857e04033e09c84324948b2cbfdd9e043b24d62b)，0.1.5 / Apache-2.0 | libfvde 对照补受检 metadata copies、多 LV、碎片映射、CF plist、恢复 key、取消与读完整性；实际调用 audit_info。见 [FILEVAULT](../../docs/formats/FILEVAULT.md) 和目录内 LOVELYCARVE.md。 |

## MINIFS 原创读取库

`minifs` 0.1.0 是本仓库原创、AGPL-3.0-only 的独立 Rust 库，没有外部解析器源码上游。
格式参照 [MiniFS 2024 论文](https://arxiv.org/html/2407.05064v1) 并核对官方 Archer C6 v4 固件；
LZMA 复用 Apache-2.0 的 `lzma-rust2`。没有复制无明确代码许可的 H4lo Python 提取器。
库接口、限额、夹具许可与验证来源见 [minifs/README.md](minifs/README.md) 和
[MINIFS](../../docs/formats/MINIFS.md)。

## F2FS / EROFS

- `f2fs-reader`：只读元数据解码改造自 `KarpelesLab/fstool` 0.5.0 / MIT，独立 inode / 目录 / 读路径
  为本仓库 AGPL-3.0-only；固定来源见 [ORIGIN.md](f2fs-reader/ORIGIN.md)，未引入 writer。
- `lamfold-erofs` 0.1.3，`fabaad6af0c28b80535307b339b84110f3a3c12e` / MIT OR Apache-2.0；
  复用已 vendored 的 lamfold，校验和、卷边界、限额和时间补丁见 [LOVELYCARVE.md](lamfold-erofs/LOVELYCARVE.md)。
- mkfs 工具生成的公开样本保留原许可证与哈希；F2FS 的 syzkaller 样本为 Apache-2.0。

## qnx-reader（原创）

纯 Rust QNX4 / QNX6 Power-Safe / MMI 只读库，AGPL-3.0-only；未移植内核 C 驱动。格式参考 Linux v6.12 公开布局及 NFI qnxmount 的 Apache-2.0 Kaitai 结构。真实 QNX6 工具生成夹具与 TAR 来自 NFI qnxmount `11c8a7f9ee9b945d584263743f6ea8524e8776d2`，其许可证和生成脚本保留在 `qnx-reader/tests/fixtures/`，来源与 SHA-256 见 manifest.json。合成数据及生成器为 CC0；能力、拒绝策略与验证边界见 [QNX](../../docs/formats/QNX.md)。

## UDF

`lamfold-udf` 0.1.1 来源于 [lamco-admin/lamfold fabaad6af0c28](https://github.com/lamco-admin/lamfold/tree/fabaad6af0c28b80535307b339b84110f3a3c12e/crates/lamfold-udf)，MIT OR Apache-2.0。复用既有 lamfold；补 descriptor tag / CRC、单物理分区映射与边界检查、512–4096 字节块、FE / EFE、AED 续链、稀疏区、符号链接、时间与资源限额；不含 C/DLL。固定来源、本地改造和上游 mkudffs / Linux 内核夹具见 [LOVELYCARVE.md](lamfold-udf/LOVELYCARVE.md) 与 [UDF](../../docs/formats/UDF.md)。
