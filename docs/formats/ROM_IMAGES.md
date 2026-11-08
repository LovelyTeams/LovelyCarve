# CramFS / Linux ROMFS

使用 Rust 库 [lamfold](https://github.com/lamco-admin/lamfold) 的
`lamfold-cramfs 0.1.0` 和 `lamfold-romfs 0.1.1`，保留其读取契约并补齐
取证读取所需的校验、范围与遍历约束。
未使用宿主挂载、外部提取器或 C 解压库。

## 范围

| 格式 | 已接入 | 明确限制 |
| --- | --- | --- |
| CramFS FSID v2 | 小端 / 大端 bitfield、完整文件系统 CRC32、zlib 4 KiB 页、HOLES 空洞、普通文件、目录和符号链接；范围读取只解压涉及的页 | v1 无 CRC、EXT_BLOCK_POINTERS / XIP、未知 flags、前置 512 字节引导填充尚未接入 |
| Linux ROMFS `-rom1fs-` | 声明长度、前 512 字节或完整短镜像的校验和、每个文件头含名称与填充的校验和；普通文件、目录、符号链接、硬链接和执行权限 | genromfs 扩展、Neotron ROMFS 不在范围内；硬链接最多 8 次跳转 |

裸镜像、MBR / GPT 分区和 HDR1 / TRX 文件系统段使用相同适配器。扩展名
`.cramfs` / `.romfs` 已加入镜像选择器；`.bin` / `.img` 按内容识别。
裸文件按字节寻址，保留不足一个磁盘扇区的尾部；真实磁盘分区保留原始几何。
UBI 内嵌文件系统目前仍只自动展开 SquashFS。

完整、流式、范围读取共用同一内容实现，并检查实际读取长度。设备、FIFO 和
socket 显示在树中但拒绝正文读取。符号链接保留目标且不在适配器内自动跟随；
硬链接保留各自的路径身份并读取同一目标内容。压缩数据不虚构物理 data runs。
两种格式没有 MACB 时间戳，显示未知，不使用镜像创建时间代替。

ROMFS 的格式校验和只覆盖规定的超级块与文件头区域，不能校验所有文件正文。
单次返回缓冲区最多 512 MiB；较大文件可分段或流式读取。

目录重复名称、链循环、祖先目录复用、越界、短解压、非法路径和非 UTF-8 名称
明确报错。每目录最多 100,000 项，宿主路径深度最多 256 层，不返回成功的截断目录。
源字节、源哈希和 RAW 导出仍读原容器；加密固件中的文件系统视图复用已验证明文。

## 来源与评估

- `lamfold 0.1.5` / `lamfold-romfs 0.1.1`：上游
  [fabaad6af0c2](https://github.com/lamco-admin/lamfold/commit/fabaad6af0c28b80535307b339b84110f3a3c12e)。
- `lamfold-cramfs 0.1.0`：发布包记录
  [44f0343b2487](https://github.com/lamco-admin/lamfold/commit/44f0343b2487f65cfb84508bff375fdda18c18a5)。
- MIT OR Apache-2.0；许可证与 NOTICE 保留在三个 vendor 目录。本地补丁见各自
  `LOVELYCARVE.md`，依赖仅启用 std 和纯 Rust DEFLATE。
- 另一个候选 [avalon1610/cramfs](https://github.com/avalon1610/cramfs/blob/517a4d37cb02c63e59c822d133e0f00be7525a48/Cargo.toml)
  的仓库依赖私网 Git 地址，且 CRC 判断存在问题，未选用。
- 格式参照 [Linux ROMFS 文档](https://docs.kernel.org/filesystems/romfs.html)、
  [CramFS 公共磁盘结构](https://github.com/torvalds/linux/blob/master/include/uapi/linux/cramfs_fs.h)。
  没有复制 Linux 内核文件系统实现。

## 验证

```powershell
just test rom_images
just test-vendor lamfold --no-default-features --features std,codec-deflate
just test-cli
$env:LOVELYCARVE_ROM_IMAGES = 'C:/samples/cramfs.bin;C:/samples/romfs.bin'
just test rom_images_external_samples -- --ignored
```

合成镜像由 `crates/core/test_data/rom-images/generate.py` 独立编码，覆盖大小端、
压缩页与空洞、跨页范围、空文件、硬链接、符号链接、特殊文件、CRC / 长度 /
解压损坏、目录循环，以及裸镜像、MBR、TRX 的正式 dispatch。

公开对照使用 [Binwalk 测试输入](https://github.com/ReFirmLabs/binwalk/tree/master/tests/inputs)
的 `cramfs.bin`（4096 字节，SHA256 `1e75d47c5c759bfb964011a4530ce353bcf7fe2377eb59b32b773445dbad4216`）
及 `romfs.bin`（1024 字节，SHA256 `31259785aa0c9404e6ae496b27c37d0620e10f783b127848aa3d5cc2a25cb6d3`）。
两者含同一份 104 字节 `/lsb-release`；外部样本不提交仓库。
Windows 原生 GUI / Dokan 挂载和其他平台运行仍需单独验证。
