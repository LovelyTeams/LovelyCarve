# BeFS（BeOS / Haiku BFS）

纯 Rust `befs-reader` 只读读取 BeOS / Haiku 的 Be File System。适配前检查了
rusty-backup 的 `src/fs/bfs.rs`，复用其布局及读取思路，再按原生 Haiku 定义补齐
分配校验、范围边界、目录交叉引用和扩展属性。固定源码、许可和样本来源见
[ORIGIN.md](../../vendor/befs-reader/ORIGIN.md)。不链接 C 文件系统库，
不依赖内核挂载或外部解包程序。

## 能力

| 项目 | 范围 |
| --- | --- |
| 格式 | 小端 Intel / Haiku 与大端 BeOS/PPC；1024 / 2048 / 4096 / 8192 字节块；超级块在字节 512 或 0 |
| 识别 | 裸卷及 MBR 等分区细化；独立 `befs` 类型；与 UnixWare Boot File System 和旧 BeOS OFS 分开 |
| 几何 | 多 allocation group、位图页、部分最后分配组、绝对日志游标、root / indices inode 引用 |
| 目录 | 按页读取 B+tree，检查层级、分隔键、排序、父子键范围、各层双向兄弟链、空闲节点链和目录 `.` / `..` |
| 名称 | UTF-8、大小写敏感；反斜杠作为证据名称字节保留；同 inode 的不同名称保留独立路径身份 |
| 文件 | 12 个直接 run、间接数组、两级间接数组；校验范围累计值和固定的 double-indirect 单位；按逻辑长度裁剪预分配，不暴露 slack |
| 范围读 | 全读 / 流式 / 范围读共用已验证映射；连续物理 run 合并，二分定位逻辑区间，不从文件头读取并跳过；支持大于 4 GiB 的逻辑及物理偏移 |
| 链接 | 内联及长符号链接保留原目标字节，不交给主机跟随；设备 / FIFO / socket 等特殊 inode 不作为普通文件内容读取 |
| 属性 | small_data 内联属性和属性目录中的外联 inode；保留名称、类型码、resident 标志与原始数据；通过现有 CLI 命名流接口读取 / 导出 |
| 工作流 | 浏览、预览、文件哈希、导出、自动目录快照 / 索引、CLI；沿用只读来源和导出不覆盖规则 |

只有已分配、位于卷内且不与 boot / 位图 / 日志等保留区重叠的块才能作为内容。
检查单文件数据、数组元数据、inode 和已知根 inode 的重叠。缺失逻辑覆盖、零长非法 run、
重复 / 越界块、空闲块、截断或源读取失败均报错，不将缺失数据补零。
位图中 `1` 表示已分配，按卷字节序解析 32 位 word，word 内 LSB 对应较低块号。

属性目录没有普通目录的 `.` / `..` 项。Haiku 通过 `S_ATTR` mode 标识属性 inode，
不要求旧的 `INODE_ATTR_INODE` flag；不能据该 flag 缺失拒绝合法外联属性。
文件内建名称的 `small_data` 项属于目录元数据，不伪装为用户属性。

## 时间证据

BeFS 记录 creation / modification 的原始 64 位时间字；高 48 位是有符号 Unix 秒。
BeOS 的低位可用于唯一性计数，Haiku 又将部分低位用于细分时间。超级块没有
时间编码版本位，卷名也不能可靠证明创建系统。

- 应用默认采用 `TimestampEncoding::Unknown`，呈现可确定的 UTC 秒，保留合法负时间
  和零值；不把计数解释成小数时间，不将原本保留的字段标成 ctime，也不虚构 atime。
- Rust API 的 `recorded_times` 返回完整原始字。来源明确为 Haiku 时，可用
  `set_timestamp_encoding(TimestampEncoding::Haiku)` 或 `RecordedTimes::interpret`
  按原生 mask 解析亚秒和 status-change 字段，排除唯一性位及 `0xf000` 标记区。
  这项解释需要外部来源依据，当前桌面 / CLI 没有单独的编码选择参数。

## 限制

- 只使用干净且日志首尾相同的已提交树。dirty / 非空日志明确拒绝；不执行日志重放、
  写入、修复、删除恢复或孤儿 inode 枚举。
- 不执行原生属性索引查询，也不审计全部 `indices` 子树；检查其根 inode 的基本结构。
  属性类型码在 Rust API 中保留，现有通用 CLI 命名流响应仅携带名称、长度和 resident。
- 非 UTF-8 名称 / 目标不能经当前字符串契约无损显示时会明确失败，不用替换字符掩盖；
  命名流读取仍返回属性的原始二进制字节。
- 单对象映射最多 100 万条分配记录、131072 个数组元数据页；目录最多 100 万项，
  B+tree 可达 / 空闲节点合计最多 65536 个、深度最多 64；单 inode 属性最多 65536 项，
  符号链接目标最多 64 KiB。达到限制报错。inode / 位图缓存为 128 / 8 项，仅保留最近的
  目录、文件映射和属性列表。
- 不宣称完整 fsck、全卷块所有权审计或 BeFS 原生数据库索引功能。

## 验证

- `just test-befs` / `just clippy-befs`：独立 Python 编码的大小端、四种块大小、多组 / 部分末组、
  B+tree 内部页 / 叶链 / 空闲页、两级间接、链接、属性、日期及损坏样本；虚拟发明源验证
  大于 4 GiB 的偏移。`just generate-befs-fixtures` 重新编码并记录哈希，所有内容为自制数据
  （CC0-1.0），不冒充 native formatter / fsck oracle。
- `just test-befs-native`：缓存并校验 Haiku 官方 hrev60295 anyboot ZIP 的发布方 SHA-256，
  解出原生 2048 字节块卷，遍历文件、属性并验证完整 / 范围读取。另从发布方包仓库独立下载
  系统 `.hpkg`，用其 SHA-256 核对原生卷中的完整文件。官方输入不随源码分发。
  用 `--offline` 复用已校验缓存，`--cache <目录>` 更换缓存位置。
- 原生应用测试检查裸卷 / MBR、流式一致、源字节不变、属性及文件导出不覆盖、dirty 拒绝和快照；
  `just test-cli` 覆盖读取 / 哈希 / 属性读取与导出。设置 `BEFS_ORACLE_IMAGE` 为上述已校验
  hrev60295 镜像时，还执行原生镜像的 CLI 系统包哈希对照。
- 真实 BeOS/PPC、大于 4 GiB 的原生文件、真实 double-indirect 和其他原生块大小尚未逐文件对照；
  合成布局测试不能替代这些验证。

格式参考：[Haiku BFS 定义](https://github.com/haiku/haiku/blob/660ea78079ca9ee36623a0562208476bce89b11e/src/add-ons/kernel/file_systems/bfs/bfs.h)、
[原生卷几何](https://github.com/haiku/haiku/blob/660ea78079ca9ee36623a0562208476bce89b11e/src/add-ons/kernel/file_systems/bfs/Volume.cpp)、
[原生数据流寻址](https://github.com/haiku/haiku/blob/660ea78079ca9ee36623a0562208476bce89b11e/src/add-ons/kernel/file_systems/bfs/Inode.cpp)。
