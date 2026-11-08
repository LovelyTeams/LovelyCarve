# VxWorks / TP-Link MINIFS

通过「添加检材 → 镜像文件」或 CLI `images open` 打开裸 MINIFS v3、以及带显式
MINIFS 分段描述的 TP-Link legacy v1 更新固件，浏览、预览、哈希、导出和建立索引。
使用独立 Rust 库 [`minifs`](../../vendor/minifs/README.md)，不执行固件或外部提取器。

## 实现与限制

- 16 字节 `MINIFS` + NUL 标记、版本字段为 3 的大端表格式；一个压缩块可以含多个文件。
  根目录和中间目录从文件路径重建，名称区分大小写。文件节点为
  `minifs:part:<分区ID>:<UTF-8路径十六进制>`，避免不同卷之间混用身份。
- 完整读取、流式读取和范围读取共用块解码；第一次访问需解压整个所在块，后续访问复用
  当前会话的一块缓存。限制、LZMA 终态及短读校验见库 README。
- 不虚构时间戳、inode、权限、删除项或压缩文件的物理 data runs。源长度、原始字节、
  镜像哈希和 RAW 导出保持原始固件；文件操作读取解压后的逻辑字节。
- TP-Link 更新文件只按已观测的固定分段描述定位：0x54 的偏移相对外层 20 字节前缀，
  0x58 给出分段长度；检查外层声明总长、头重叠、源边界与全部 MINIFS 表范围。
  不扫描任意魔数猜测文件系统，不认证这个封装的 MD5 / 数字签名。
- 旧式 88 字节记录、其他版本、非 UTF-8 名称、无 EOS / 加密块和其他封装布局未支持。
  不解析未知字段，不宣称通用支持全部 VxWorks 文件系统。原生 GUI / Dokan 挂载需单独验证。

## 公开真实固件

[TP-Link 官方 Archer C6(EU) V4.0 220425 ZIP](https://static.tp-link.com/upload/firmware/2022/202207/20220713/Archer%20C6(EU)_V4.0_220425.zip)
中的 `c6v4-up-noboot_2022-04-25_17.23.00.bin`：

| 项目 | 值 |
| --- | --- |
| 固件长度 | 2,934,140 字节 |
| 固件 SHA-256 | `50ede51b484339ebefe1a7928f2fc760c8015ea4f9a1093b8b6a2f258cd8a262` |
| MINIFS 来源偏移 | `0x157014` |
| MINIFS 精确长度 | 1,525,829 字节（不含封装尾部填充） |
| MINIFS SHA-256 | `717a6a9dc715186ec794e6c3229f1de0093f473b56580c949b67cc393ff2c996` |
| 文件 / 压缩块 | 362 / 44 |
| 许可 | 厂商固件，版权归 TP-Link；仅作为本地外部测试，不提交 / 再分发 |

独立 oracle 直接用 Python `struct` 和标准库 `lzma`（liblzma）解析官方下载字节，
输出每个完整路径的长度与 SHA-256。Rust 库验证全部文件内容、范围读与 EOF；
宿主验证完整固件打开、递归目录、全部文件哈希、完整 / 流式 / 范围读取、无伪造 runs
与原始源字节。CLI 还将全部 362 个文件按目录导出，与独立清单逐个核对长度和哈希，
并核对源模式 RAW 导出与官方固件逐字节相同。
它不是设备上运行文件系统的 oracle，也不证明其他固件布局兼容。

```powershell
# 下载、解压上面的官方 ZIP 后，填写 .bin 的路径。
$firmware = 'C:/samples/c6v4-up-noboot_2022-04-25_17.23.00.bin'
just generate-minifs-fixtures --firmware $firmware --output fixtures/minifs/c6-oracle
$env:MINIFS_IMAGE = (Resolve-Path fixtures/minifs/c6-oracle.minifs).Path
$env:MINIFS_ORACLE = (Resolve-Path fixtures/minifs/c6-oracle.tsv).Path
just test-vendor minifs minifs_external -- --ignored --nocapture
$env:LOVELYCARVE_MINIFS_FIRMWARE = $firmware
just test minifs_external --lib -- --ignored --nocapture
```

外部测试默认 ignored，未提供样本不计为真实固件验证。
常规回归使用 Python 独立生成的最小合成镜像，覆盖共享块、空文件、Unicode、目录、
范围、短读、损坏 / 重名 / 越界 / 限额、取消、LZMA 尾随数据及终态破坏。
源码与其他布局参考见库 README；基本验证入口：

```text
just test-vendor minifs
just clippy-minifs
just test minifs --lib
just test-cli
```
