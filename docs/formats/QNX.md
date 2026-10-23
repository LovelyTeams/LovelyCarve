# QNX4 / QNX6 Power-Safe

裸 `.qnx4` / `.qnx6` 镜像和磁盘分区可直接浏览、预览、哈希、导出、建立快照与索引。
使用原创独立纯 Rust `qnx-reader`，共享 lamfold 的只读字节源与路径接口，不挂载镜像，
不依赖 QNX、Linux、Python 或外部提取器。MBR 的原始类型字节保留，文件系统按实际结构识别。

## 支持范围

| 布局 | 实现 |
| --- | --- |
| QNX4 | 小端、512 字节块、64 字节内联 inode，16 字节短名与最长 48 字节 link 名称；根 inode 和 `.bitmap` 验证 |
| QNX4 extents | 单 extent、多 `IamXblk` 链，检查计数、签名、前向环、反向指针、extent 重叠、来源范围与逻辑总长；范围读取二分定位 |
| QNX6 | 标准 v4.3；主超级块位于 8192 或 0，MMI 布局位于 0；512 / 1024 / 2048 / 4096 字节块，大小端解析 |
| QNX6 事务代际 | 两份 512 字节超级块各自校验 CRC32、几何、卷身份；在有效且一致的候选中选 serial 最大者，不拼接不同代际的根节点 |
| QNX6 tree | inode、长名与文件的 16 根指针、0～5 层间接树；检查深度、容量、inode 范围、指针边界及间接环；普通文件的未映射指针读零，元数据缺块报错 |
| QNX6 名称 | 27 字节短名与最长 510 字节长名，普通布局校验长名 checksum；MMI 按其格式不要求该 checksum |
| 共有能力 | 普通文件、目录、硬链接、符号链接；完整、流式与范围读共用同一映射。符号链接保留原始目标，不在宿主跟随；路径保持分区与目录身份 |
| 时间 | 保存 ftime / mtime / atime / ctime 的创建、修改、访问、状态变更角色，按 POSIX UTC 秒解释，不虚构纳秒或其他时间角色 |

分配数据截断或越界明确报错，不补零掩盖来源缺失。源哈希、原始字节和 RAW 导出不变，
文件不生成未经验证的物理 data runs。特殊 inode 只供元数据浏览，内容读取拒绝。

## 限制

- QNX6 当前保守接受标准 v4.3 和已验证的超级块标志集合 `0x101`；其他版本 / 标志明确拒绝。
  inode 的低三位扩展模式与零保留字段是当前验证范围；未知扩展 / 加密内容拒绝读取。
  不提供 fsencrypt 密钥解锁、解密或完整性认证。MMI 仅有合成样本验证。
- 读取当前有效已提交代际，不恢复历史快照、删除项或未提交事务，也不执行 bitmap 分配一致性审计。
  主副本损坏时可通过仍合理的几何或精确卷镜像末尾定位备份；不扫描任意尾部垃圾推测副本。
- 每个读者最多缓存 100,000 个 inode；QNX4 最多累计 1,000,000 个 extent；目录限 32 MiB，
  符号链接限 64 KiB，宿主路径限 256 层。达到上限返回错误。
- 非 UTF-8、反斜线或带 NUL 的名称拒绝，避免有损解码合并不同目录身份。
  Unicode 短名有合成验证，非 ASCII 长名 checksum 的厂商差异尚未真实验证。
- 本次不包含 QNX IFS 启动镜像、ETFS、EFS、QTD 或其他 QNX 文件系统；它们不能当作 QNX4 / QNX6 读取。
  原生 GUI / Dokan 驱动行为需另行验证。

## 来源与验证

格式参考 [Linux QNX6 文档](https://docs.kernel.org/filesystems/qnx6.html)、v6.12 的
`include/linux/qnx6_fs.h` / `include/uapi/linux/qnx4_fs.h`，以及
[NFI qnxmount 的 QNX6 结构](https://github.com/NetherlandsForensicInstitute/qnxmount/blob/11c8a7f9ee9b945d584263743f6ea8524e8776d2/qnxmount/qnx6/parser.ksy)。
Rust 实现为原创 AGPL-3.0-only 代码，没有移植或调用内核 C 驱动。

QNX6 兼容样本来自 NFI qnxmount 固定提交 `11c8a7f9ee9b945d584263743f6ea8524e8776d2`：
其脚本在 QNX 上执行 `mkqnx6fs`、挂载、创建文件并生成独立 TAR。
测试逐一比对 TAR 中 32 个条目的文件内容、权限、修改时间、符号链接与目录类型；
源镜像另有 `.boot`，TAR 的 `*` 未包含它。不是用户车机检材。
Apache-2.0 许可证、原生成脚本、TAR、源哈希与压缩镜像一起保留在库的 `tests/fixtures/`。

独立 Python 字段夹具覆盖 QNX4 extent 链 / 长名链接与 QNX6 大小端、四种块长、
无 bootblock / MMI、长名 / 硬链接 / 符号链接 / 空洞；Rust 另验证全部间接层级、
坏指针、环、长名 checksum、超级块回退与截断。字段夹具不宣称是完整 mkfs 输出。
QNX4 尚无由原系统工具生成的外部样本；已用独立 MIT [qnxprobe](https://github.com/abrignoni/qnxprobe/tree/dffd27ab242c16100b40114f248e816fb302cc52) 比对合成 QNX4 的 8 个条目和 NFI QNX6 的 32 个 TAR 条目。复现时取得该提交的 `qnxprobe.py`，运行 `just test-qnx-reference <脚本路径>`；脚本 SHA-256 为 `6feb99d15cd6841d77a150719ce8335859f450a566aefd46352e51c9fd6f93ed`。这不是对真实 QNX4 检材的验证。

```text
just generate-qnx-fixtures
just test-qnx
just clippy-qnx
just test qnx --lib
just test-cli
```
