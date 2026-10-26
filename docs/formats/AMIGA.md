# Amiga OFS / FFS 与 RDB

使用 Simon Dick 的纯 Rust `amiga-ffs` 0.4.0 和 `amiga-rdb` 0.4.1，
由只读 `amiga-reader` 适配。适配前也检查了 `affs-read` 0.5.1 与 fstool 0.5.0；
前者接口 / 格式范围较窄，后者仅覆盖经典 DOS0–5。来源、固定版本及补丁见
[ORIGIN.md](../../src-tauri/vendor/amiga-reader/ORIGIN.md)。运行时不链接 C 文件系统库，
不调用挂载 / 解包程序；检材适配仅实现 `BlockSource`，不实现写入所需的 `BlockSink`。

## 能力

| 项目 | 范围 |
| --- | --- |
| 格式 | DOS0–7：OFS / FFS、international、directory-cache、long-name；512 B 至 32 KiB 的二次幂块 |
| 识别 | 裸 ADF / HDF、直接包含文件系统的 MBR 分区、原生 RDB 分区表；独立 `amiga` 类型 |
| RDB | RDSK / PART 校验与链边界；保留物理分区范围、DosType、reserved、文件系统块数 / 块大小，支持 SecPerBlock 大于 1 和未使用的轨尾扇区 |
| 目录 | 原生哈希表 / 碰撞链、文件与目录硬链接；名称按 Latin-1 无损映射，保留反斜杠；经典最多 30 字节，LNFS 最多 107 字节 |
| 内容 | 文件头、扩展块、OFS 带头数据 / FFS 原始数据；全读 / 流式 / 范围读一致，FFS 连续物理块合并读取 |
| 链接 | 硬链接共享 inode 身份，路径节点和名称头日期分别保留；软链接显示 Latin-1 目标，内容读返回原始字节，不交给主机跟随 |
| 时间 | 保留从 1978 年起的本地 DateStamp，50 Hz tick 精度；无时区不标 UTC。叶节点仅有修改时间，未知 POSIX mode 保持未知 |
| 工作流 | 浏览、预览、文件哈希、导出、自动目录快照 / 索引和 CLI |

位图检查所访问块的分配状态，校验元数据 checksum、指针边界、单文件重复块及
数据与根 / 位图元数据重叠。OFS 还验证数据块 owner、顺序号、长度与 checksum。
缺失映射、空指针、空闲块、越界、截断和源错误均明确失败，不补零冒充文件内容。
原始检材哈希和 RAW 导出保持源字节。

## 限制

- dirty / bitmap-invalid 卷仍可识别，但浏览明确拒绝；不执行 Amiga 磁盘验证、修复、
  写入、格式化或删除恢复。RDB 中非空 BADB 坏块重映射明确拒绝，不能将未重映射字节当作内容。
- SFS、PFS、MS-DOS FAT 等 RDB DosType 不会冒充 OFS / FFS；已知其他文件系统仍可由各自探针识别。
  不执行 RDB 内附的文件系统驱动，不支持非零 SectorOrigin 或不同于设备块大小的 SizeBlock 布局。
- 目录读取以已校验的哈希表为准，不依赖 directory-cache 的副本；不提供目录缓存一致性审计、
  comment / protection 等扩展属性 UI、完整 fsck 或全卷块所有权审计。
- 元数据每段操作最多 131072 次块读取，目录最多 131072 项、硬链接链最多 256 层；
  根位图最多 200 万页。RDB 最多 4096 次元数据读取、256 个分区 / 驱动。
  达到限制明确失败。条目 / 位图缓存分别 128 / 8 项，仅保留最近的目录和文件映射。
- 共享路径契约将 `.` / `..` 解释为特殊路径成分，不能将它们作为 Amiga 普通名称访问。
- 应用与 CLI 展示顶层平面分区表；MBR 内嵌 RDB 的递归识别是 Rust 内部树接口，
  尚未接到主窗口 / CLI 的分区浏览流程。

## 验证与夹具

`just test-amiga` / `just clippy-amiga` 使用独立 Python amitools 0.8.1 创建并回读的
DOS0–7、512 / 1024 / 4096 字节块样本，覆盖文件扩展、位图扩展、Latin-1 / 长名称、
日期、范围读和损坏拒绝。原生测试覆盖 RDB 多种块几何、MBR 直接卷、硬链接、快照、
导出不覆盖和源字节不变；`just test-cli` 检查读取、SHA-256 和导出。
上游库的独立单元测试分别通过 `just test-amiga-ffs` / `just test-amiga-rdb` 运行。

`just generate-amiga-fixtures` 需要单独提供 amitools 0.8.1 的 Python 包路径。
amitools 是 GPL-2.0-or-later 的外部测试工具，不随运行时链接。其 LNFS 写入器存在
name/comment 编码问题；生成脚本明确记录独立 NaC 字段编辑，并由未修改的 amitools
读取器回读长名称和内容。硬 / 软链接样本是基于 oracle 分配布局的独立字段编辑，
不能称为 oracle 原生创建并验证的链接样本。
样本内容为自制数据（CC0-1.0），哈希见
[manifest.json](../../src-tauri/vendor/amiga-reader/tests/fixtures/manifest.json)。
尚无真实 Amiga 硬件镜像、非 512 字节 RDB 设备扇区、2 / 8 / 16 / 32 KiB 卷及
大型真实硬盘的独立逐文件对照，合成样本不能代替这些验证。

格式参考：[AmigaOS 文件系统](https://wiki.amigaos.net/wiki/UserDoc%3AAmigaOS_File_Systems)、
[DCFS / LNFS 数据结构](https://wiki.amigaos.net/wiki/DCFS_and_LNFS_Low_Level_Data_Structures)、
[Amiga ROM Kernel Reference Manual: DOS](https://developer.amigaos3.net/sites/default/files/downloads/2024-10/Amiga_ROM_Kernel_Reference_Manual_DOS.pdf)。
