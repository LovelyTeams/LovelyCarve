# Buffalo / TRX、SafeLoader、uImage、CHK 与 Sercomm 固件

通过「添加检材 → 镜像文件」直接打开 `.enc`、`.trx`、`.chk`、`.uimage`、`.uimg`
或具有对应内容特征的 `.bin` 固件，无需先运行外部解包程序。裸 `.squashfs`、
`.cramfs` 和 `.romfs` 也可直接打开。

## 实现范围

- Buffalo `start` / `asar1` ENC，默认公开密钥 `Buffalo`、256 字节 RC4 状态；
  包括 `bgn` 外层、152 字节加密产品标签、对齐后的 ENC 主体，以及解密后的
  `buffalo.enc\nfilelen=…\n` 前缀。产品标签、载荷均验证 Buffalo 的有符号字节校验和。
- Broadcom TRX v1：验证声明长度、CRC、三个分段槽的顺序与边界，展示其中的
  SquashFS / CramFS / Linux ROMFS / UBIFS / JFFS2 段。内核、填充和其他段保留在原始源中，不伪装成文件系统。
- TP-Link SafeLoader 标准 0x1014 payload 布局：校验声明总长和公开盐化 MD5，
  严格解析 `fwup-ptn` 镜像分段表；验证分段范围、重复名称、重叠、终止符和必要元数据，
  按原始来源偏移展示所有可识别的文件系统段。Flash 分区表中的目标写入地址不冒充镜像位置。
- U-Boot legacy uImage：校验 64 字节头与数据 CRC；支持内核后紧接 rootfs、未压缩
  filesystem 类型，以及未压缩 multi-image 的长度表与 4 字节对齐。压缩内核保持不透明，
  不执行内核，也不把解压出的内核内容误作文件系统。尾部 rootfs 由原生文件系统读取器校验。
- Netgear CHK：校验可变头长、board ID、内核 / rootfs 声明长度、头 / 分段 / 全载荷的
  32 位回绕双累加校验和；支持内核字段包含 uImage + rootfs，或独立 rootfs 字段。
- Netgear Sercomm NAND：校验 512 字节 `sErCoMm` seal 的版本、保留字段和整包字节和；
  复用 Rust ZIP 索引与 DEFLATE 解码，检查单一 ZIP32 `.bin` 成员、本地 / 中央目录一致性、
  精确输入 / 输出长度、CRC 与终止。支持 stored / DEFLATE，校验匹配的 Sercomm 尾部、
  NAND 填充及 2 MiB 处内核 uImage 的两项 CRC，浏览 6 MiB 处的 UBI → SquashFS / UBIFS。
  此布局对应 OpenWrt `netgear_sercomm_nand`，不以扫描任意魔数猜测其他机型布局。
- 标准小端 / 大端 SquashFS 1.0、2.0/2.1、3.0、4.0，以及 DD-WRT 3.0 的 `hsqt` / `tqsh`
  变体；后者按每个块的 pb/lc/lp 参数解压 LZMA，复用文件树、预览、哈希、
  导出、流式与范围读取。
- `hsqs` / `sqsh` SquashFS 3.0 同时读取 gzip 和 LZMA 块：复用 backhand 的
  adaptive 参数选择与镜像内缓存，解码改用纯 Rust `lzma-rust2`。支持 raw LZMA、
  13 字节 LZMA-alone 和 5 字节属性前缀；gzip 优先，LZMA 块可改变参数。
  参数选择限 `lc + lp ≤ 4`、`pb ≤ 4`、前缀 0–10 字节；标准属性头允许 `lc ≤ 8`、
  `lp ≤ 4`，声明字典最多 8 MiB。解码需完整终止标记且消费整个块，声明正文长度必须匹配。
  无终止标记的 legacy 流尚不支持。元数据输出限 8 KiB，文件数据限该卷块大小（最多 1 MiB），
  失败不发布部分解压结果；同时检查 range coder 终态和源 EOF，避免把缺失尾字节补作零。

源镜像大小、源哈希、十六进制视图和源 RAW 导出保留原始容器字节。
固件按字节寻址；加密段的位置指向原始密文对应范围，文件浏览读取经过校验的明文。
裸 SquashFS / UBIFS / JFFS2 也按字节保留完整长度，支持小于一个扇区或末尾不对齐的文件系统镜像。
压缩文件不虚构物理 data runs。明文归属于镜像实例，卸载时释放，不写入案件源文件。
打开时使用现有任务的取消令牌，逐块解密和校验；单个加密载荷限 512 MiB。
嵌套封装最多 4 层、每个 SafeLoader / uImage 分段表最多 32 项，总文件系统数最多 256。

Sercomm 的源分段范围描述 ZIP 成员的实际压缩字节；文件系统会话附加独立的解压 UBI
视图，从逻辑偏移零读取，并沿用源检材 / 分段 ID。目录、预览、范围、哈希、导出及
快照通过该视图读取；快照卷长使用逻辑长度。源镜像长度、源哈希和 RAW 导出仍是原始
封装，不把解压地址映射成物理 data runs，也不标记为已解锁的加密分区。
视图在证据会话与活动读取句柄释放时一并释放，解压数据不写回源文件。
编码源和解压 NAND 各限 512 MiB，校验与解码逐 64 KiB 检查取消；UBI 使用 128 KiB
擦除块、尾部 128 字节对齐。本地 / 中央记录间的空洞、尾随压缩数据和截断明确失败。

不支持自定义 Buffalo 密钥、longstate 模式、其他 TRX 版本、签名验证或任意
厂商固件格式。SafeLoader QNEW、其他 payload 起点、uImage FIT、压缩的 multi-image /
filesystem 和内核 / rootfs 之间的任意填充布局尚未接入。Sercomm 的多成员、ZIP64、
加密 ZIP、data descriptor 和其他 NAND 布局尚未支持。它们不是 Firmware Mod Kit 全部工具的兼容承诺。识别到的
不支持结构、校验失败和截断均明确报错，不以成功的空目录代替。

## 上游依据与许可

格式与兼容行为对照 [Firmware Mod Kit，c72f45d7a062](https://github.com/rampageX/firmware-mod-kit/tree/c72f45d7a062156125ae00cb867d8a614b50b963)：
`src/firmware-tools/buffalo-lib.c`（Gabor Juhos，GPL-2.0-only）、
`src/squashfs-3.0-lzma-damn-small-variant/squashfs_fs.h` 与其
`lzma/C/7zip/Compress/LZMA_Lib/ZLib.cpp`。

本地实现为 Rust 格式读取代码，没有复制或构建这些 C/C++ 源文件，也不调用
Firmware Mod Kit 的 Linux shell、提取器或重打包工具。
SquashFS 复用 `backhand`（MIT OR Apache-2.0），LZMA 使用 `lzma-rust2`；
旧 v1/v2 及无 EOS LZMA 的严格校验与限制见 [SquashFS](SQUASHFS.md)。
具体集成变更见 [backhand/LOVELYCARVE.md](../../src-tauri/vendor/backhand/LOVELYCARVE.md)。

SafeLoader 与 CHK 对照 [OpenWrt firmware-utils b2a93c6f77ba](https://github.com/openwrt/firmware-utils/tree/b2a93c6f77bac3a6525e93b383c987b073b4633b)：
`src/tplink-safeloader.c`（Matthias Schiffer，BSD-2-Clause）、`src/mkchkimg.c`
（Dave C. Reeve，GPL-2.0-or-later）。uImage 对照同一版本的 `src/uimage_padhdr.c`
及 [U-Boot legacy image 头](https://github.com/u-boot/u-boot/blob/master/include/image.h)。
封装解析是独立 Rust 实现，没有引入这些 C 工具；MD5 和 CRC 复用项目已有 Rust 依赖。

Sercomm 对照同一 firmware-utils 的 `src/mksercommfw.c`，以及
[OpenWrt v24.10.1 NAND 构建规则](https://github.com/openwrt/openwrt/blob/v24.10.1/target/linux/ramips/image/mt7621.mk)。
解析库为本仓库的 `sercomm-firmware`，ZIP 复用 `zip`、压缩复用 `flate2`，UBI
复用已加固的 `ubidump`；没有复制或构建外部 C 解包器。字节和 / CRC 不是签名认证。

CramFS / Linux ROMFS 的 Rust 库选择、支持范围和校验要求见 [ROM_IMAGES](ROM_IMAGES.md)；
UBIFS 的支持范围和限制见 [UBIFS](UBIFS.md)，JFFS2 见 [JFFS2](JFFS2.md)；其他公开固件仍不能展开的实测结果见 [兼容性调查](FIRMWARE_COMPATIBILITY.md)。

## 验证

```powershell
just test firmware_
just test-vendor sercomm-firmware
just clippy-sercomm
just test-vendor backhand --features v3_ddwrt
just test-vendor backhand --features v3_ddwrt,v3_lzma
$env:LOVELYCARVE_FIRMWARE_IMAGE = 'C:/path/to/router.enc'
just test firmware_external_image -- --ignored --nocapture
$env:LOVELYCARVE_WRAPPED_FIRMWARE = '<SafeLoader路径>;<uImage路径>;<CHK路径>'
just test firmware_wrappers_external_samples -- --ignored
$env:LOVELYCARVE_SQUASHFS_LZMA = '<Unblob LE路径>;<Unblob BE路径>'
just test squashfs_lzma_external -- --ignored
just run-example probe_squashfs C:/path/to/router.enc
```

合成测试覆盖 TRX 的长度、CRC、分段边界、取消，以及通过正式镜像入口的
目录、文件、流式、范围读取和原始字节保留。外部样本按文件逐一比较完整、
流式和范围读取，不提交真实固件；未提供样本时外部测试保持 ignored。
外部样本一致性测试不等于运行 Firmware Mod Kit 二进制的独立 oracle。
SafeLoader / uImage / CHK 的合成编码器覆盖双 rootfs、multi-image、嵌套 CHK、
有效外层校验和下的内层损坏、长度 / 表重叠 / 重名 / CRC / 终止符与递归限制。
公开 OpenWrt C7 v5 与 EX6150 的下载来源和哈希见 [兼容样本清单](firmware-samples.json)。

Sercomm 的独立 Python 夹具覆盖 stored / DEFLATE、NAND 与 UBI 布局、CRC、长度、
尾部身份与取消。原始字节、源导出、无伪造 data runs、逻辑快照卷长和释放生命周期均有
宿主回归；公开 R6220 / R6260 另比较完整文件、流式、范围和独立 rootfs 对照。
设置 `LOVELYCARVE_SERCOMM_FIRMWARE` 为分号分隔的 factory 镜像路径后，运行
`just test firmware_sercomm_external --lib -- --ignored`。

独立解压对照使用同序的两组路径：`SERCOMM_FIRMWARE_EXTERNAL` 指向 factory 镜像，
`SERCOMM_FIRMWARE_ROOTFS` 指向各型号官方单独发布的 `rootfs.bin`，均以分号分隔。
运行 `just test-vendor sercomm-firmware external_nand -- --ignored`，比较整个解压 UBI
与对应 rootfs 的字节；下载 URL、大小和 SHA-256 见公开样本清单。
