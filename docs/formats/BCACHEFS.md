# bcachefs

适配前先检索 Rust 库，选择 MIT 许可的 `rust-fs-bcachefs` 0.4.2，而不是包装原生
C 工具。固定提交、修改点、许可和镜像哈希见
[ORIGIN.md](../../vendor/bcachefs-reader/ORIGIN.md)。
生产读取不调用挂载、FUSE、外部提取器或 C 文件系统库；证据字节接口没有写入能力。

## 能力

| 项目 | 范围 |
| --- | --- |
| 几何与识别 | 单设备、小端、1.x 元数据至 1.39；支持 inode_v3；校验布局所列超级块、按最大序号选副本，同号使用后一个副本；记录不含校验和时不虚构完整性保证 |
| 地址与树 | packed / unpacked 键、多 bset、内部树、按键定位；验证节点身份、层级、序列、键序、桶代际、设备与文件逻辑范围 |
| 目录 | UTF-8 原始名称、反斜杠、硬链接、符号链接与特殊文件类型；crc32c / SipHash 查找；目录项与 inode 类型不一致时报错 |
| 视图 | 按目录项进入原生子卷和 snapshot；同号 inode 在不同 snapshot 中保持独立内容和缓存，显示原生 inode 编号；应用目录快照保留各路径 |
| 文件内容 | inline / extent、稀疏空洞、unwritten 零区、reflink、LZ4 / Zstandard / raw DEFLATE；完整、流式和范围读取共用逻辑字节 |
| 校验 | 元数据和数据的 CRC32C / CRC64 / XXH64；读取 poison / error extent 明确失败；源读取故障不能被元数据恢复兜底隐藏 |
| 日志 | 上游在内存中重放提交日志，不写原证据；干净根不可读时可使用日志所记根；blacklist / bset 序列参与选择 |
| 时间 | atime、mtime、POSIX ctime、otime 分开；时间基数、精度、带符号纳秒和负时间保留；未观察到的高字时间编码明确拒绝 |
| 应用 | 裸卷 / 分区识别、浏览、预览、范围读、哈希、导出、目录快照、CLI 和挂载适配；不改变源分区身份 |

多设备、条带 / 纠删码、以口令封装的主密钥、旧 inode_v1 / inode_v2、新于 1.39 的
元数据版本及未实现的 extent 类型明确拒绝。上游另有未封装密钥的 Rust 解密与
扩展属性 API；原生加密镜像尚未验证，扩展属性尚未接到应用命名流入口。
删除文件的元数据恢复、损坏扫描、写入和文件系统修复不在此适配范围内。

大小写折叠沿用上游 Unicode 表；与原格式 Unicode 12.1 的变化字符尚未逐项原生核对。
不把这一点视为任意 Unicode 版本的兼容保证。

## 资源与读取边界

节点身份和单 inode 键数量上限为一百万，snapshot 父链最多 1024 层；检测循环、缺失父节点
和资源上限时明确报错。B-tree 节点缓存同时受 512 项和 64 MiB 原始节点字节预算约束。
一段需完整校验 / 解压的数据最多 64 MiB，先检查大小再分配；无校验和与 unwritten
大 extent 的小范围读取仅处理请求窗口，不能为了几字节分配整个 extent。
格式定义的空洞返回零；已分配正文越界、短源、校验不符或来源故障返回错误。

## 验证

- 独立 Python 夹具覆盖子卷 / snapshot 中同号对象的不同内容、硬链接、reflink、5 GiB
  稀疏文件、unwritten、符号链接、负时间及纳秒；损坏用例重新签 CRC 后检验分配代际与循环。
- 虚拟 8 GiB 源把已分配正文放到 6 GiB 处，检验 64 位物理地址和小窗口的有界 I/O；
  该用例独立于原生 512 MiB 镜像，不把它当作真实大盘验证。
- Linux 原生工具生成的 512 字节无校验和、4096 字节 LZ4 / Zstd / gzip 镜像，分别逐项核对
  1514 个目录项的类型、mode、inode、时间戳、全部普通文件 SHA-256 和范围字节。
- 前三组以官方 FUSE 读取器观察；其 gzip 路径崩溃，因此 gzip 改用官方 1.39.7 内核模块
  在 Debian `7.2.9+deb13-amd64` 上以 `loop,ro,nochanges` 挂载核对。四组均通过原生
  fsck，挂载前后整镜像哈希一致；内核模块与原生工具只用于外部验证。
- 原生历史视图、reflink / unwritten、未封装密钥加密、64 KiB 块及 Windows 实际 Dokan
  挂载尚未独立验证；不能把上游能力声明或自制夹具当作这些场景的原生证明。

```powershell
just generate-bcachefs-fixtures
just test-bcachefs
just test-bcachefs-reader
just clippy-bcachefs
just clippy-bcachefs-reader
$env:BCACHEFS_ORACLE_DIR = 'D:/oracle/bcachefs'
just test-bcachefs-native
just test-cli
```

原生测试明确要求完整的外部镜像与清单；缺失输入会失败，不当作通过。读取器测试与
CLI 测试使用同一来源清单；大镜像只放在外部缓存，不随仓库分发。
