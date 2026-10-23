# 公开固件兼容性调查与 Rust 库选择

公开样本清单记录各下载输入的来源、哈希和原生目录读取结果。能作为 RAW 打开但没有
文件系统目录，不计为解析成功。这些是样本结论，不代表同品牌所有固件；读取器项目的
测试镜像不等于厂商发布的整机固件。仓库只保存清单与独立合成夹具。

## 兼容样本与限制

| 样本 | 现象 | 定位 |
| --- | --- | --- |
| OpenWrt 24.10.1 Archer C7 v5 `factory.bin` | 校验 SafeLoader 后直接浏览 SquashFS，保留来源偏移 2,622,944 | 标准 SafeLoader 已接入；QNEW 等布局仍不支持 |
| 同设备 `sysupgrade.bin` | 校验 uImage 头与内核 CRC，直接浏览偏移 2,614,181 的 SquashFS | 支持紧接内核的数据；任意填充布局仍不支持 |
| OpenWrt 24.10.1 Netgear EX6150 `factory.chk` | 校验 CHK 与内层 uImage 后直接浏览偏移 3,183,392 的 SquashFS | CHK 已接入；不等于所有 Netgear 固件均支持 |
| OpenWrt 24.10.1 Netgear R6220 / R6260 `factory.img` | 校验 Sercomm seal 与 ZIP 后浏览 UBI → SquashFS | 复用 Rust zip / flate2 / ubidump；解压 rootfs 与各自官方 `rootfs.bin` 对照，压缩来源范围与逻辑卷几何分别保留，见 [FIRMWARE](FIRMWARE.md) |
| Unblob UBIFS `banana.zlib.ubifs` / `banana.lzo.ubifs` / `banana.zstd.ubifs` | 读取已提交目录和四个文件，与独立输出核对内容 | 已接入裸卷与 UBI 内嵌卷；前两份输入的数据节点为 stored，压缩路径另有独立夹具，范围见 [UBIFS](UBIFS.md) |
| Unblob JFFS2 新格式 LE/BE zlib（含 64 KiB padded 版本）、RTIME / LZO | 浏览目录并与独立输出比较三个文件内容和哈希 | 已接入新格式大小端与版本片段重建；能力 / 限制见 [JFFS2](JFFS2.md) |
| Binwalk `yaffs2.bin` | 原生 YAFFS2 目录与 13 个普通文件 | 复用并扩展 Rust 头解析；与独立 yaffshiv 的显式几何输出逐字节 / 哈希对照，范围见 [YAFFS2](YAFFS2.md) |
| Unblob YAFFS2 `sample.2048.64.ecc.le` / `sample.4096.128.ecc.be` / `sample.16384.512.le` | 原生浏览目录，读取六个普通文件并保留两条符号链接与特殊节点 | 与独立 yaffshiv 配置输出核对；覆盖标签 ECC、大小端和较大 NAND 页，见 [YAFFS2](YAFFS2.md) |
| Unblob `links.yaffs`、`fruits.dir.le.yffs` / `fruits.dir.be.yffs` | 仍没有原生文件系统目录 | 512 / 16 旧式 YAFFS tags 未接入；不能把 YAFFS2 packed-tags 支持等同于全部 YAFFS 布局 |
| Unblob SquashFS v1.0、v2.0/v2.1 大小端及 Binwalk v2 BE | 原生目录及文件读取 | 旧布局已接入；Binwalk v2 另覆盖属性头无 EOS LZMA，范围见 [SquashFS](SQUASHFS.md) |
| Unblob SquashFS v3 adaptive LZMA LE / BE | 直接浏览两份镜像并读取 `apple.txt` / `orange.txt` | backhand 参数选择配合纯 Rust 解码；独立输出的字节 / SHA256 对照，支持范围见 [FIRMWARE](FIRMWARE.md) |
| Binwalk `cramfs.bin` / `romfs.bin` | 直接浏览、读取和哈希 | 支持边界见 [ROM_IMAGES](ROM_IMAGES.md) |

阳性对照：Binwalk 标准 SquashFS、OpenWrt 19.07.10 ath25 裸 SquashFS，
以及 OpenWrt 24.10.1 R6220 `rootfs.bin` 的 UBI → SquashFS 均能列出文件。
因此不能把“旧 OpenWrt”或“所有 UBI 固件”概括为不支持。

另外，当前 backhand 依赖未启用标准 LZ4 / LZO 与 v4 adaptive LZMA 特性，属于
源码确认的能力边界；公开清单未覆盖对应标准 v4 压缩样本，不计入上述实测结论。

## 先找 Rust 库，再改造

| 文件系统 | 候选 | 源码评估 / 处理 |
| --- | --- | --- |
| CramFS | [lamfold-cramfs](https://github.com/lamco-admin/lamfold/tree/44f0343b2487f65cfb84508bff375fdda18c18a5/crates/lamfold-cramfs) | 已独立编译和读取公开样本；复用 BlockSource / FoldFrontend / zlib，引入本地 CRC、BE、页级范围与目录检查后接入 |
| Linux ROMFS | [lamfold-romfs](https://github.com/lamco-admin/lamfold/tree/fabaad6af0c28b80535307b339b84110f3a3c12e/crates/lamfold-romfs) | 已独立验证并接入；补声明边界、头与超级块校验和、链循环和错误传播 |
| JFFS2 | [jffs2-rs 0.2.1](https://github.com/xmoezzz/jffs2-rs/tree/52e1234ce66b0e099b35afabff3fc3e3078a2c0e) | 复用 Rust 字段和 RTIME 后接入有界只读库；修正固定 LE、CRC、目录硬链接、版本与片段长度，LZO 改用 lzokay；不引入 C Rubin/LZO，支持边界见 [JFFS2](JFFS2.md) |
| UBIFS | [rust_ubidump](https://github.com/aspnmy/rust_ubidump/tree/19782c3eabc25a1baab7faefe12fe6f4bd362dac) | 复用其 UBI 与 UBIFS 节点/键/inode/索引结构并改造为有界库；修正 node_type 偏移、min-I/O 主节点扫描、带符号 R5 名称哈希、文件空洞/裁剪及 raw DEFLATE 语义，使用 Rust 解码依赖。读取干净的已提交 v4/v5 树；日志重放、加密与恢复明确未支持 |
| SquashFS | [backhand](https://github.com/wcampbell0x2a/backhand) | 扩展当前已 vendor 的库。补 v1.0、v2.0/v2.1 大小端、尾片段与范围 / 流式读取，保留旧 inode 缺失时间；无 EOS LZMA 使用 lzma-rust2 配合加固的 lzma-rs 做最终状态核对。其他 Rust 候选 squashfs_reader / squashfs-async 仅支持 v4。支持 v3/v4 标准、DD-WRT v3 与 v3 adaptive / 标准 LZMA；移除 adaptive C/C++ 依赖，用 lzma-rust2 进行有界解码并检查终止与 EOF。XZ / Zstandard 也已切换为 Rust 解码，检查完整流、校验和及资源上限；其他压缩/变体逐项测试后启用 |
| YAFFS2 | crates.io 与公开 GitHub 检索 | 尚无选定的可直接复用 Rust 文件系统读取库；Binwalk 及 [binwalk-ng](https://github.com/binwalk-ng/binwalk-ng/blob/845806c163747eb524d66b5fe3de2ed86ea3aaad/src/formats/yaffs.rs) 的签名 / 部分头解析不能替代读取器，后者的 `yaffs2_extractor` 仍调用外部 `unyaffs`。已基于其 MIT Rust 前缀扩展 `yaffs2-reader`，补几何 / 标签 ECC、版本、shrink / shadow、目录、硬链接、稀疏与范围读取；没有调用外部工具，限制见 [YAFFS2](YAFFS2.md) |

其他候选：Disrobe 的 Rust 清单声明专有 Source-Available 许可，未选用且未引入其 YAFFS 实现；TicKV 是另一种存储格式。

排除误匹配：`neotron-romfs` 是不同磁盘格式；`unearth` 的相关能力是识别，
不能替代文件读取；另一候选 `avalon1610/cramfs` 的仓库清单使用私网 Git 依赖，
且 CRC 条件不正确，未选用。Rust 语言标签也不代表所有解压依赖都是纯 Rust。

## 来源与复现

- 官方固件：[ath79/generic 24.10.1](https://downloads.openwrt.org/releases/24.10.1/targets/ath79/generic/)、
  [ramips/mt7621 24.10.1](https://downloads.openwrt.org/releases/24.10.1/targets/ramips/mt7621/)、
  [ath25/generic 19.07.10](https://downloads.openwrt.org/releases/19.07.10/targets/ath25/generic/)。
- 封装依据：[OpenWrt C7 构建规则](https://github.com/openwrt/openwrt/blob/v24.10.1/target/linux/ath79/image/generic-tp-link.mk)、
  [Netgear EX6150 / R6220 规则](https://github.com/openwrt/openwrt/blob/v24.10.1/target/linux/ramips/image/mt7621.mk)。
- 测试输入：[Binwalk](https://github.com/ReFirmLabs/binwalk/tree/master/tests/inputs)、
  [Unblob filesystem](https://github.com/onekey-sec/unblob/tree/main/tests/integration/filesystem)。
  Unblob 的 Git LFS 对象须下载实际内容并核对指针中的 SHA256 和 size，不可把指针当镜像。

```powershell
@'
{"id":"open","command":"images open","args":{"path":"C:/samples/firmware.bin"}}
{"id":"root","command":"files list","args":{"parentId":"part:0"}}
'@ | just run-cli --quiet session
```

公开输入的完整 URL、长度、SHA256 与读取状态保存在[公开样本清单](firmware-samples.json)。
清单只描述列出的输入，不能用一个样本证明所有型号兼容。永久回归使用独立合成夹具，
不重新分发公开固件；环境变量开启的外部测试逐项验证原始字节与实际目录 / 文件读取。
