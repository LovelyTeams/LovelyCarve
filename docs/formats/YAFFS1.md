# YAFFS1 NAND 镜像

LovelyCarve 扩展现有原生 Rust `yaffs2-reader`，读取 512 字节页、16 字节 spare 的
YAFFS1 镜像。它与 YAFFS2 共用目录、硬链接与逻辑范围接口，识别结果为独立的
`yaffs1` 分区类型，文件身份使用 `yaffs1:part:`；已有 YAFFS2 身份保持有效。
原始镜像读取、哈希和 RAW 导出始终保留页正文、spare 与固件源字节。

## Rust 库评估与来源

未选到可直接复用的完整开源 Rust YAFFS1 镜像读取器。Binwalk 的 Rust 部分仅有
部分头识别，TicKV 是不同格式；FsRover 使用 C，crush-forensics 使用 Python，
Disrobe 为专有 Source-Available 许可，没有引入其 YAFFS 实现。
本次在已采用的 Rust 库内增加独立 `legacy_tags` / `legacy_scan` 模块。

格式事实对照 Aleph One
[`474b3acb927d`](https://github.com/Aleph-One-Ltd/yaffs2/tree/474b3acb927d27b2305618aaf24456b9d33fe91b/core)
的 `yaffs_guts.h`、`yaffs_tagscompat.c`、`yaffs_yaffs1.c` 与 ECC 定义。
没有复制、构建或调用其 C 实现；标签 ECC 直接计算位位置校验，页正文 ECC 使用
Rust 行列奇偶矩阵，不导入 C 查找表或自动修改证据位。
公共头结构沿用已有 MIT 来源，许可证记录见 [YAFFS2](YAFFS2.md)。

## 能力与文件长度

- 识别大小端 C bitfield 布局及分散在 spare 中的八字节 tags，校验 12 位标签 ECC。
  按物理页向前扫描；对象头和数据页重复时，只有两位 serial 的直接后继替换已有页，
  包括 `3 → 0` 回绕。擦除、已删除页不投影为现存文件。
- 支持目录、普通文件、硬链接、符号链接、特殊节点元数据、当前重命名与 shadow。
  名称为有界 UTF-8，不跟随符号链接到宿主。硬链接路径独立，内容 inode 与目标一致。
  特殊节点没有文件正文；不猜测没有对象头的数据页路径。
- 对象头在打开时验证页正文 ECC；已分配文件数据在读取时验证完整 512 字节页，
  范围请求即使避开损坏位也会失败。格式定义的逻辑空洞返回零，源错误不得补零。
  spare 中为全 `FF` 的 ECC 无法与缺省 / 未提供的 ECC 区分，不宣称这些正文已认证。
- 完整、流式与范围读取共用页图。传统 Unix 秒与扩展 Unix 时间保留角色；虚拟根
  缺少真实对象头时不补造权限或时间。未知 / 不支持的时间与名称配置明确拒绝。

YAFFS1 的文件长度策略是挂载配置，镜像没有标志可可靠推断。宿主和默认打开接口
采用头声明的文件长度：读取裁剪到声明 EOF，声明内没有已分配页的区域返回零。
库的 `YaffsImage::open_yaffs1(..., FileSizeSource::DataPages, ...)` 可在独立掌握源配置时
使用数据页末尾推导长度，对应原始扫描器的另一配置。不能把这种选择描述为自动识别。
合成夹具覆盖两个策略得出不同长度的情况。

## 限制与完整性

- 当前支持 512 / 16、每块 32 页的旧式分散 tags；其他 spare 排列、连续 packed-tags1、
  1024 字节扩展页、未知硬件配置、Unicode / WinCE 布局尚未接入。
- 标记坏块、非确定的删除状态、块内擦除页后出现已编程页均明确失败。
  不做 ECC 纠错，不修改原始字节，不以纠正后的页替代来源。普通 ECC 不是签名或
  文件内容的密码学认证，无法排除完整页丢失 / 重排、合法空洞与未提供 ECC 的歧义。
- 最多 4,000,000 页、元数据保守预算 256 MiB，父链 / 硬链接深度 256；单次读取最多
  256 MiB，较大内容使用范围 / 流式接口。20 位 chunk 地址限制单文件约 512 MiB。
  库直接接收 `CancellationToken`；宿主沿用现有同步卷打开和任务取消边界。
- 不提供写入、删除文件恢复、历史视图、未分配空间投影或 xattr 浏览；缺失父链、
  目录环、重复现存名称、悬空硬链接均不猜测修复。页映射不伪装为连续物理 data runs。

## 验证

独立 Python stdlib 编码器生成 CC0 夹具，覆盖大小端、分散 tags、标签 / 正文 ECC、
UTF-8、跨页、两位 serial 回绕、已删除页、shadow、两种长度策略、空洞及源完整性。
夹具清单见 [legacy-fixtures.json](../../vendor/yaffs2-reader/test_data/legacy-fixtures.json)。
固件夹具另覆盖大端 YAFFS1 经 Buffalo ENC / TRX 后仍保持源身份与 RAW 字节。

Unblob 的 `fruits.dir.le.yffs` / `fruits.dir.be.yffs` 与 `links.yaffs` 用作公开输入。
普通文件与其独立预期输出的实际 Git LFS 对象对照，长度与 SHA256 必须匹配；符号链接
目标取 Git mode `120000` 的原始 blob，避免 GitHub 内容接口跟随链接后返回目标正文。
来源、哈希和当前读取结果见 [公开样本清单](firmware-samples.json)，不提交真实固件。

```powershell
just test-vendor yaffs2-reader
just clippy-yaffs2
just test yaffs1_host --lib
just test-cli
$env:YAFFS2_MATRIX_DIR = '<三份公开原文件所在目录>'
just test-vendor yaffs2-reader legacy_external -- --ignored
just test yaffs1_host_external --lib -- --ignored
```
