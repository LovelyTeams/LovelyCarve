# GFS2

`gfs2-reader` 是纯 Rust、只读的 GFS2 1801 / 1802 卷读取器。适配前检索了
crates.io / GitHub，现有 Rust `unearth` 仅识别超级块；本实现复用其 MIT
前缀布局，并按原生格式定义新增目录、文件、分配和日志校验。
固定来源及许可见 [ORIGIN.md](../../src-tauri/vendor/gfs2-reader/ORIGIN.md)。
产品不链接 C 文件系统库，不调用内核挂载或外部解包程序。

## 能力

| 项目 | 范围 |
| --- | --- |
| 格式 | 大端 GFS2 1801 / 1802、multihost 1900；GFS1（1309）保持独立，不冒充支持 |
| 几何 | 字节 65536 的超级块、块大小 / shift、root / master 双重 inode 身份；卷分配范围由 rindex 决定，源长度只是上限 |
| 分配 | 多资源组、首 RG 页和续 RB 页的两位分配状态，区分 free / used / unlinked / dinode；组重叠、rindex / 组头不一致及组头 CRC 错误明确拒绝 |
| 目录 | stuffed 线性目录、exhash 表、重复桶范围及碰撞链；名称 CRC32、叶深度 / 格式 / 所有者、记录边界、目录计数和 `.` / `..` 身份校验 |
| 文件 | stuffed 内容及完整的定高指针树；格式定义的零指针读零；分配块缺失、越界、循环及重叠报错；内容按逻辑长度裁剪 |
| 名称与链接 | 大小写敏感、UTF-8、证据中的反斜杠保留；硬链接共享 inode 并保留不同路径；符号链接返回源目标，特殊文件不作为普通内容读取 |
| 时间 | UTC atime / mtime / ctime 及独立纳秒，保留负时间和零值；不虚构 creation time |
| 扩展属性 | 单 EA 块及间接 EA 列表、内联值和 ED 外联值；保留 user / system / security / trusted 名称空间和原始字节；trusted 需要 1802 |
| 应用 | 裸卷 / 分区识别、浏览、预览、哈希、导出、范围读取、快照及 CLI 命名流接口 |

范围读取复用已验证的块映射，按逻辑偏移定位，不从文件头流式跳过。
目录逻辑数据块去除原生 JD 头；普通 journaled-data 文件的内容块仍保留完整块正文。
所有主机导出沿用新建文件及不覆盖规则，来源始终只读打开。

## 日志与限制

- 打开卷时检查 master / rindex / jindex，验证每个 journal 的块映射及日志头。
  依据校验和、块身份和序列号选择最新日志头，要求 clean-unmount 标志。
  不能因存在旧的 clean 头就忽略更新的 dirty 头；需要重放明确拒绝，源字节不变。
- 不执行日志重放、在线集群协调、写入、修复、删除恢复或孤儿扫描。
  输入必须是完整且稳定的卷字节视图；不把单个未组装的底层存储成员当成完整卷。
- 保留 native inode 身份，但不声明完整 fsck 或全卷所有权审计。
- 单个文件映射最多 4194304 个逻辑块、131072 个指针元数据块。
  单目录最多 100 万项，exhash 深度最多 17、叶链最多 65536 块。
  rindex 最多 131072 组，inode / 位图缓存分别最多 128 / 8 项。
  单 inode 属性最多 65536 项、解码值合计最多 64 MiB；单值最多 64 KiB。
  达到上限报错，不截断为成功结果。
- 每卷最多 256 个连续编号的日志；每日志遵循原生 8 MiB–1 GiB 大小范围。
  全卷日志合计最多检查 4 GiB，超出工作量上限明确报错。
  打开时读取并检查日志，因此大集群卷的首次打开存在额外 I/O。
- 非 UTF-8 名称 / 字符串目标不能由当前文本契约无损表达时明确失败。
- 超级块接受 512–65536 的二次幂块大小；原生验证覆盖 512 / 1024 / 2048 / 4096，
  更大的原生 Linux page-size 布局尚未完成独立核对。

## 验证

- `just test-gfs2` / `just clippy-gfs2`：独立 Python 自制夹具和损坏场景；
  `just generate-gfs2-fixtures` 重新编码并记录 SHA-256。
  自制数据为 CC0-1.0，不充当 native formatter oracle。
  虚拟发明源另行验证超过 4 GiB 的逻辑与物理偏移、直接范围定位和空洞零读。
- `scripts/gfs2-native-fixtures.py --output <新目录>` 在一次性 Linux 环境中调用
  `mkfs.gfs2`、Linux GFS2 挂载和 `fsck.gfs2 -n`，只格式化脚本新建的普通镜像文件。
  内核生成文件哈希、名称、时间和属性清单，卸载后压缩镜像并记录 SHA-256。
- 实际核对使用 Debian Linux `6.1.0-54-amd64` / `gfs2-utils 3.5.0`，
  512 / 2048 字节块使用 1801，1024 / 4096 字节块使用 1802；每卷两个日志、
  多个资源组、1500 项 exhash 目录、
  12 MiB 间接文件、32 MiB 稀疏文件、硬链接、UTF-8 / 255 字节名称和外联属性。
  设置 `GFS2_ORACLE_DIR` 为解码目录后运行 `just test-gfs2-native`，先核对镜像 SHA-256，再
  逐一核对每卷 1512 个名称及文件完整 / 范围内容、纳秒时间和全部属性。
  原生输入保存在外部测试缓存，不随源码分发。
- 应用及 CLI 测试另行覆盖裸卷 / MBR、来源身份和常规工作流；这些不能替代真实
  Windows Dokan 挂载、在线集群或其他体系结构的验证。

格式参考：[Linux GFS2 on-disk 定义](https://github.com/torvalds/linux/blob/7a78f7ba9f24962c8033e0152bf34a7903ff61ba/include/uapi/linux/gfs2_ondisk.h)。
