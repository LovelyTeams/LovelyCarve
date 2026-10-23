# YAFFS2 NAND 镜像

LovelyCarve 使用原生 Rust `yaffs2-reader` 读取带 page / spare 的 YAFFS2 镜像，
支持裸卷与受支持的固件分段。原始镜像读取、哈希和 RAW 导出保留包含 OOB 的源字节；
文件读取只组合各数据页声明的正文，不把 spare 或页尾填充拼入文件。

## Rust 库选择与来源

未选到可直接复用的完整开源 Rust YAFFS2 文件读者。
复用并扩展 [binwalk-ng 固定版本的 Rust 头解析](https://github.com/binwalk-ng/binwalk-ng/blob/845806c163747eb524d66b5fe3de2ed86ea3aaad/src/formats/yaffs.rs)
中的前缀结构和字段校验，保留 MIT 许可；其外部 `unyaffs` 提取入口未引入。
本仓库补充几何探测、标签与 ECC 校验、版本重建、目录及随机读取。

另一 Rust 候选 Disrobe 的清单声明专有
[Source-Available 许可证](https://github.com/1-3-7/disrobe/blob/main/LICENSE)，未选用；
仅检查清单与许可，没有引入其 YAFFS 实现。TicKV 是受 YAFFS 概念启发的另一种格式，
不能代替 YAFFS 镜像读者。

格式事实对照 Aleph One YAFFS
[`474b3acb927d`](https://github.com/Aleph-One-Ltd/yaffs2/tree/474b3acb927d27b2305618aaf24456b9d33fe91b/core)
的 `yaffs_guts.h`、`yaffs_packedtags2.*`、`yaffs_yaffs2.c` 与时间/文件长度定义。
没有复制、构建或调用其 C 实现；标签 ECC 使用 Rust 直接计算行列奇偶矩阵。

## 当前能力

- 自动识别大小端、512 / 1024 / 2048 / 4096 / 8192 / 16384 字节页，
  16 / 32 / 64 / 128 / 256 / 512 字节 spare 与 0 / 2 字节标签偏移的已验证组合。
  YAFFS2 没有唯一超级块 magic；以多页标签和对象头的一致性识别，歧义时不猜测。
- 自动探测检查前 2 MiB；库的 `open_with_geometry` 可用于已知几何、长擦除前缀或空卷。
  源长度必须为完整 page + spare 的倍数；所有读取检查范围和真实返回长度。
- 解析普通与 extra packed tags，核对对象类型、父 ID、大小、hardlink、shrink / shadow。
  已存在的 packed-tags ECC 必须正确；未提供 ECC 的标签明确保留为无校验输入。
- 按 sequence 与物理页序重建最新头和数据页，保留当前重命名，处理删除 / unlinked、
  截短、后续扩展、较新数据页带来的长度增长以及 shadow 对象覆盖。
  输入应保留单个 NAND/固件镜像的页位置，不能拼接不同设备或不明擦除块副本。
- 支持目录、普通文件、硬链接、符号链接和特殊节点元数据；硬链接路径独立，
  内容 inode 与目标一致。符号链接目标不跟随到宿主；特殊节点没有可导出的正文。
- 支持 64 位文件长度、格式定义的稀疏空洞、跨页范围与流式读取。
  传统 Unix 秒级时间与一致的扩展 Unix 时间保留角色和宽度；虚拟根 / lost+found
  缺少源头时保持未知权限和时间，不补造文件系统创建时间。
- checkpoint / summary 不投影为用户文件；通过现存日志页重建当前目录，不创建猜测的
  孤儿路径。缺失父链、目录环、重复现存名称、悬空或循环硬链接明确失败。

## 限制与完整性

- YAFFS1、in-band tags、UTF-16 / 非 UTF-8 名称、WinCE 时间、非 POSIX mode、
  标记坏块及其他硬件 OOB/ECC 布局尚未接入。现有标签 ECC 不做自动纠错，错误明确报告；
  NAND 正文硬件 ECC 不校正。库保留扩展时间原始字段。
- 普通数据页没有通用内容 CRC；标签 ECC 不是文件正文认证，也不是整镜像签名。
  完整页的丢失有时无法与合法稀疏 / 擦除布局区分。已分配页的源短读或 I/O 错误不能补零；
  文件逻辑空洞才返回零。源哈希仍读取原始镜像。
- 最多 4,000,000 页，元数据保守预算 256 MiB，父链 / hardlink 深度 256；
  单次范围最多 256 MiB，大文件使用范围或流式接口。库的取消接口直接接收
  `CancellationToken`；宿主沿用现有同步卷打开接口，遍历/导出在现有任务边界取消。
- 不提供写入、删除文件恢复、历史视图、NAND 未分配空间投影或 xattr 浏览。
  不生成把 OOB 页映射误当作连续文件内容的物理 data runs。

## 验证

独立 Python stdlib 夹具覆盖大小端、标签偏移、存在 / 不存在的标签 ECC、UTF-8 名称、
硬链接、符号链接、设备、删除、重命名、shrink 后扩展、shadow、64 位大小 / 时间、
稀疏大范围读取、取消、资源上限、源短读和错误。
生成器与 CC0 哈希见 [夹具清单](../../src-tauri/vendor/yaffs2-reader/test_data/fixtures.json)。

公开 Binwalk `yaffs2.bin` 与独立 Python
[yaffshiv f6f0ef77bf79](https://github.com/devttys0/yaffshiv/blob/f6f0ef77bf79ca46aaa77bc34eda06e7b55da8e8/src/yaffshiv)
以显式 2048 / 64、LE、tags offset 0 配置对照；自动探测不是该 oracle 的验证条件。
公开源 URL / SHA-256 和逐文件内容哈希记录在 [样本清单](firmware-samples.json)。
不提交真实固件或外部提取器。

Unblob 的三份 `sample.*.yaffs2` 另覆盖 2048 / 64 LE（有标签 ECC）、
4096 / 128 BE（有标签 ECC）与 16384 / 512 LE（标签偏移 2）。
使用独立 oracle 核对六个普通文件、两条符号链接、目录与特殊节点；读取器拒绝特殊节点正文，
链接仅保留目标字符串。`fruits.dir.*.yffs` 和 `links.yaffs` 的 512 / 16 旧式 tags
布局不属于已接入的 packed-tags 路径。

```powershell
just test-vendor yaffs2-reader
just clippy-yaffs2
just test yaffs2_host --lib
just test-cli
$env:YAFFS2_EXTERNAL = '<公开镜像路径>'
$env:YAFFS2_ORACLE = '<独立提取目录>'
just test-vendor yaffs2-reader external_all -- --ignored
$env:LOVELYCARVE_YAFFS2_IMAGE = $env:YAFFS2_EXTERNAL
just test yaffs2_host_external --lib -- --ignored
$env:YAFFS2_MATRIX_DIR = '<上述六份 Unblob 原文件所在目录>'
just test-vendor yaffs2-reader external_unblob -- --ignored
```
