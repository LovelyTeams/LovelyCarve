# ext2/3/4

ext2/3/4 由纯 Rust 的 vendored `ext4-view` 加应用层取证适配器读取，没有引入 libfsext 的 C 代码、DLL、运行时依赖或 LGPL 链接。磁盘分区和 LVM 逻辑卷中的 ext 卷使用相同的节点 ID、IPC、预览、导出、搜索与快照流程。

本文以 [libyal/libfsext](https://github.com/libyal/libfsext/tree/48d3d73911a69ef0fc100ed190d8398d1200f08d) 提交 `48d3d73911a69ef0fc100ed190d8398d1200f08d`（2026-10-12 核对）为参照，覆盖 README、公开头文件、超级块、组描述符、inode、目录、块映射、extent、属性和数据流实现及相应测试。这是源码与格式契约的对照，没有编译运行 libfsext，不等于两个实现在真实镜像上做过逐文件比对。

## 能力矩阵

“读者 API”表示后端可以调用，不代表桌面属性面板已经展示该字段。libfsext 一列来自其 README，具体边界以固定提交的实现为准。

| 能力 | libfsext | LovelyCarve |
| --- | --- | --- |
| ext2 / ext3 / ext4 只读访问 | 支持 | 支持；ext2、ext3 与 4 KiB 块样本有回归 |
| 原始 ext2 revision 0 | 支持 | 静态 revision 固定使用 128 字节 inode |
| 没有 FILETYPE 的旧目录格式 | 支持 | 16 位名称长度，从目标 inode 取类型 |
| FILETYPE 为 UNKNOWN 的目录项 | 可按 inode 取类型 | 从目标 inode 取类型；非法类型报错 |
| 根目录、inode 编号、路径查找 | 支持 | 稳定的 inode ID，可按路径或编号访问 |
| 原始名称、UTF-8 / UTF-16 | C/Python API 提供转换 | 保留原始字节名称；Rust 字符串无需 C 的 wide API |
| 硬链接 | 目录项与 inode 关联 | 目录项分别投影；`Metadata::links` 给出链接数 |
| 快速、数据块符号链接 | 支持 | 支持；`read_link_inode` |
| 内联文件的前 60 字节 | 声明支持 | 支持，`File` 保留完整的 resident payload |
| 内联文件的 `system.data` 后续部分 | 数据流有内联分段处理 | 拼接、EOF 校验、跨边界 seek / read |
| 内联目录 | 固定提交读取 `i_block`，最多 60 字节 | 枚举 `i_block[4..60]` 与 xattr 两段并重建 `.` / `..`，每段独立校验记录边界 |
| 内联符号链接 | 按 inode / 流结构处理 | `read_link_inode` 使用完整内联内容 |
| inode 内扩展属性 | 支持 | 带边界检查的读者，保留二进制值并展开命名空间 |
| 外部属性块 | 支持 | 检查 magic、块数、值范围及 metadata CRC32C |
| EA-inode 属性值 | 支持，部分校验字段 TODO | 检查 feature、目标编号、EA 标记及长度，读取普通块 / extent；禁止递归内联 EA 引用 |
| 属性按名称查找、值范围读取 | 独立对象 API | `extended_attributes` 返回属性数组，可按名称和值切片访问；单值上限 64 KiB |
| 属性自身的物理 extent 查询 | 独立对象 API | 未提供；属性 API 返回值字节，不返回 EA 存储布局 |
| direct / single / double / triple indirect | 支持 | 全部层级；合并连续块，空指针跳过整个子树 |
| extent 树、64 位物理块 | 支持 | 支持；校验容量、深度递减、非零长度与偏移乘加 |
| 空洞与尾部空洞 | 支持 | 完整、范围与流式读取共享同一逻辑片计划 |
| unwritten extent | extent 标志参与数据流 | 读零，不列入正文物理 runs |
| 已初始化的 32768-block extent | 有长度处理 | 正确处理 `ee_len == 0x8000`，只有更大的值表示 unwritten |
| 文件 extent / 内容区间 | 公开 extent / seek API | 完整的间接块 runs；范围读取不分配从文件头到 offset 的前缀 |
| 64 KiB 目录记录长度 | 格式处理 | `0` / `0xffff` 解释为整块长度；拒绝超过 64 KiB 的块 |
| `meta_bg` | 支持 | 按 descriptor block 和 `s_first_meta_bg` 定位，覆盖 sparse / sparse_super2 |
| `flex_bg` / 64-bit / checksum seed | 支持 | 支持，使用 descriptor 中真实的 inode table 地址 |
| MMP / large_dir | 固定提交的特性白名单有边界 | 只读打开；MMP 不涉及写入协调，large_dir 沿用现有遍历与 htree |
| casefold 卷的原始名称 | 特性白名单接受 | 可枚举并按原始名称查找；不实现 Unicode casefold 等价查找 |
| atime / mtime / ctime / crtime | 支持 | 带符号秒 + epoch 位 + 纳秒；按 `i_extra_isize` 判断字段是否存在；crtime 投影为 `Mactb.birth` |
| dtime | 支持 | 保留原始值并提供 deletion time；孤儿链复用的值不能直接当作删除事实 |
| mode / UID / GID / links / device | 公开 API | `Metadata` 提供完整字段与 device major/minor；应用层 RawInode 含 UID/GID |
| UUID / label | 支持 | 支持 |
| 版本、feature masks、inode slots、块计数、挂载路径与时间 | 支持 | `VolumeInfo`、`Ext4Volume::volume_info`；非 64 位卷忽略计数高位 |
| 超级块 / inode / extent / 目录 CRC32C | 实现中存在校验与 TODO | 支持，另校验外部 xattr CRC32C 与损坏的 htree count 边界 |
| 旧 GDT CRC16 | 实现有 CRC16 处理 | 支持；跳过 checksum 字段本身，不按 CRC32C 的方式补零 |
| 内部 journal | 与文件结构关联 | JBD / JBD2 只读回放与历史块解析，桌面提供日志查看 |
| 外部 journal 设备 / 跨设备回放 | 接受相关特性，但不等于回放能力 | 不支持外部设备；独立的 journal-device 格式明确拒绝 |
| ext v1、压缩、fscrypt 解密 | README 明确不支持 | 不支持；加密正文读取明确报错，不把密文当明文 |
| 删除项 / 孤儿扫描 | 没有独立完整的恢复契约 | 目录 slack 恢复候选；不提供完整的 undelete 或孤儿扫描 |
| abort / IO 抽象 / mount 工具 | C/BFIO、工具层 | 使用 DiskImage、TaskManager、Dokan 和 CLI，不复制 C 的生命周期与诊断控制 API |

## 校验与边界

缺少分区表的镜像可从[签名恢复](README.md#缺失分区表)进入读者。候选卷复用主超级块解析器确定卷长，目录浏览时继续验证块组和日志。

内联内容必须有 inode 内的 `system.data`，不能把缺失的尾部补零当作完整文件。属性条目和值不能越界或覆盖属性表。EA-inode 单个属性值上限为 Linux 的 64 KiB，保留原始二进制值；名称沿用 lossy 显示语义。inode 与外部属性块的校验和会被验证，但 EA 值 hash、条目 hash 与引用计数的一致性尚未审计。

应用层 extent 和间接块正文使用同一个 `DataPiece` 计划：真实空洞和 unwritten 区间读零，已分配范围越过分区 / LVM 边界或来源短读时失败。目录 slack 扫描使用完整的间接块 runs；内联 inode 没有外部正文 runs，不能把内容字节解释为块指针。

raw inode / extent 取证辅助路径与正常读者的严格 checksum 路径不同，损坏的元数据不会被“修复”成可信证据。UID / GID / device / xattr / 卷历史字段目前只在读者和卷 API 中提供，`FsStat` 与桌面属性面板尚未展示。

## 验证

`test_data/audit_ext.bin` 是可复现的 64 KiB 合成记录镜像，由 `test_data/generate_audit_fixture.py` 生成，不含真实检材；其位图未填充，不能当作可挂载或可通过 e2fsck 的正常卷。SHA-256：`8ab0d910d74afccd1f47f84725a1c39a2d642494646342627d3c43272dc2081a`。

读者回归覆盖内联两段、目录原始名称查找、EA 值与错误引用、外部属性块 checksum、带符号 / epoch / 纳秒时间、旧目录格式、revision 0、`meta_bg` 的三种超级块布局、unwritten、三重间接 seek、64 KiB sentinel、特殊 inode 元数据，并打开上游的 ext2 / ext3 / 4 KiB 镜像。应用层回归比较完整、流式和跨边界范围读取，并验证不会读到相邻分区的字节。

```powershell
just test-vendor ext4-view
just test ext4
```

尚未覆盖：真实 `mke2fs` 生成的 inline_data + EA-inode + meta_bg 组合卷、fscrypt、外部 journal、超大 large_dir / casefold 目录，以及与 libfsext 的逐文件摘要比对。

格式依据：Linux 内核文档的 [inode](https://www.kernel.org/doc/html/latest/filesystems/ext4/inodes.html)、[扩展属性](https://www.kernel.org/doc/html/latest/filesystems/ext4/attributes.html)、[block groups](https://www.kernel.org/doc/html/latest/filesystems/ext4/blockgroup.html)，以及固定提交的 [libfsext 公开 API](https://github.com/libyal/libfsext/blob/48d3d73911a69ef0fc100ed190d8398d1200f08d/include/libfsext.h.in)。
