# 公开固件兼容性调查与 Rust 库选择

核对日期：2026-10-21。用 Windows 独立 CLI 实际打开公开固件及读取器项目的
测试镜像，并列根目录。能作为 RAW 打开但没有文件系统目录，不计为解析成功。
这些是样本结论，不代表同品牌所有固件；测试镜像不等于厂商发布的整机固件。
真实下载文件只放在本机临时审计目录，未提交到仓库。

## 实测缺口

| 样本 | 现象 | 定位 |
| --- | --- | --- |
| OpenWrt 24.10.1 Archer C7 v5 `factory.bin` | RAW，无可浏览分区；从 2,622,944 截出 SquashFS 后能列目录 | TP-Link SafeLoader 外层封装未展开 |
| 同设备 `sysupgrade.bin` | RAW，无可浏览分区；从 2,614,181 截出 SquashFS 后能列目录 | uImage + rootfs 组合未展开 |
| OpenWrt 24.10.1 Netgear EX6150 `factory.chk` | RAW，无可浏览分区；从 3,183,392 截出 SquashFS 后能列目录 | Netgear CHK 外层未展开 |
| OpenWrt 24.10.1 Netgear R6220 `factory.img` | `sErCoMm` 头，RAW 无可浏览分区 | Sercomm 打包；构建规则为 ZIP + sercom-seal，尚未接入，未凭熵猜测加密算法 |
| Unblob UBIFS `banana.zlib.ubifs` / `banana.lzo.ubifs` | 无可浏览分区 | UBIFS 文件系统尚未接入；已有 UBI 层不等于 UBIFS 读取器 |
| Unblob JFFS2 新格式 LE/BE zlib（含 64 KiB padded 版本） | 无可浏览分区 | JFFS2 尚未接入；并非仅短镜像问题 |
| Binwalk `yaffs2.bin` | 无可浏览分区 | YAFFS2 尚未接入，需 NAND page / spare 几何和版本重建 |
| Unblob SquashFS v1 LE、Binwalk SquashFS v2 BE | 识别 SquashFS，展开时明确报不支持版本 | v1 / v2 尚未接入 |
| Unblob SquashFS v3 adaptive LZMA | 识别 SquashFS，按标准 gzip 读时报解压错误 | adaptive LZMA 变体未接入；与已支持的 DD-WRT `hsqt` 变体不同 |
| Binwalk `cramfs.bin` / `romfs.bin` | 调查前无文件树；本次基于 Rust 库补齐 | 支持边界见 [ROM_IMAGES](ROM_IMAGES.md) |

阳性对照：Binwalk 标准 SquashFS、OpenWrt 19.07.10 ath25 裸 SquashFS，
以及 OpenWrt 24.10.1 R6220 `rootfs.bin` 的 UBI → SquashFS 均能列出文件。
因此不能把“旧 OpenWrt”或“所有 UBI 固件”概括为不支持。

另外，当前 backhand 依赖未启用标准 LZ4 / LZO / adaptive LZMA 特性，属于
源码确认的能力边界；本轮没有对应标准 v4 压缩样本实测，不计入上述实测结论。

## 先找 Rust 库，再改造

| 文件系统 | 候选 | 源码评估 / 处理 |
| --- | --- | --- |
| CramFS | [lamfold-cramfs](https://github.com/lamco-admin/lamfold/tree/44f0343b2487f65cfb84508bff375fdda18c18a5/crates/lamfold-cramfs) | 已独立编译和读取公开样本；复用 BlockSource / FoldFrontend / zlib，引入本地 CRC、BE、页级范围与目录检查后接入 |
| Linux ROMFS | [lamfold-romfs](https://github.com/lamco-admin/lamfold/tree/fabaad6af0c28b80535307b339b84110f3a3c12e/crates/lamfold-romfs) | 已独立验证并接入；补声明边界、头与超级块校验和、链循环和错误传播 |
| JFFS2 | [jffs2-rs 0.2.1](https://github.com/xmoezzz/jffs2-rs/tree/52e1234ce66b0e099b35afabff3fc3e3078a2c0e) | 有可复用的结构基础，但 build.rs 编译 C Rubin/LZO；扫描器忽略 CRC，inode/dirent 固定 LE，文件长度按节点大小累加。需要审计与改造，不能直接宣称支持 |
| UBIFS | [rust_ubidump](https://github.com/aspnmy/rust_ubidump/tree/19782c3eabc25a1baab7faefe12fe6f4bd362dac) | 已复用其 UBI 层；上游还有 UBIFS 源码。正文导出简单串接 data 节点，未按块号补空洞/裁 inode 长度，主节点搜索固定步长；应先改造为有界库、审核解压链和恢复语义 |
| SquashFS | [backhand](https://github.com/wcampbell0x2a/backhand) | 优先扩展当前已 vendor 的库。已支持 v3/v4 标准及 DD-WRT v3；其他压缩/变体逐项测试后启用，避免替换整套读取器 |
| YAFFS2 | crates.io 与公开 GitHub 检索 | 本轮未找到可直接复用的 Rust 文件系统读取库；Binwalk 的 Rust 签名识别不是文件系统读取实现。保持明确未支持 |

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

调查共 19 个公开下载输入（6 个官方固件/裸 rootfs、13 个测试镜像），另做了
3 个内部 SquashFS 截取对照。接入 CramFS / ROMFS 后重新实测：5 个输入可浏览，
14 个仍有上述缺口。完整 URL、长度、SHA256 与结果保存在[公开样本清单](firmware-samples.json)。
CLI 原始 JSON 输出另保留在本机审计目录的 `results.json`、`extended-results.json`
和 `post-integration-results.json`，不把成功的 RAW 打开误计为文件系统读取成功。
本次 CramFS / ROMFS 支持的永久回归只使用独立合成夹具，不重新分发公开固件。
