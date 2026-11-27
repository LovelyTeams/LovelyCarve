# CramFS / Linux ROMFS

使用 Rust 库 [lamfold](https://github.com/lamco-admin/lamfold) 的
`lamfold-cramfs 0.1.0` 和 `lamfold-romfs 0.1.1`，保留其读取契约并补齐
取证读取所需的校验、范围与遍历约束。
未使用宿主挂载、外部提取器或 C 解压库。

## 范围

| 格式 | 已接入 | 明确限制 |
| --- | --- | --- |
| CramFS FSID v1 / v2 | 小端 / 大端位字段、可选 512 字节引导填充、v2 CRC32、4 KiB zlib 页、HOLES 空洞、扩展块指针、XIP 直接页、普通文件、目录和符号链接 | 未知 flags 明确拒绝。v1 没有卷 CRC 或可信的声明长度，读取以镜像或分区边界为上限 |
| Linux ROMFS `-rom1fs-` | 声明长度、前 512 字节或完整短镜像的校验和、每个文件头含名称与填充的校验和；普通文件、目录、符号链接、硬链接和执行权限 | genromfs 扩展、Neotron ROMFS 不在范围内；硬链接最多 8 次跳转 |

裸镜像、MBR / GPT 分区和 HDR1 / TRX 文件系统段使用相同适配器。扩展名
`.cramfs` / `.romfs` 已加入镜像选择器；`.bin` / `.img` 按内容识别。
裸文件按字节寻址，保留不足一个磁盘扇区的尾部；真实磁盘分区保留原始几何。
UBI 内嵌文件系统目前仍只自动展开 SquashFS。

完整、流式、范围读取共用同一内容实现，并检查实际读取长度。设备、FIFO 和
socket 显示在树中但拒绝正文读取。符号链接保留目标且不在适配器内自动跟随；
硬链接保留各自的路径身份并读取同一目标内容。压缩数据不虚构物理 data runs。
两种格式没有 MACB 时间戳，显示未知，不使用镜像创建时间代替。

CramFS 的块指针可以指向相邻页的结尾，也可以直接指向不连续的原始页或带长度前缀的压缩页。同一文件可以混用这些布局。范围读取只解压覆盖请求的页，原始页直接读取请求范围。带引导填充的卷保留原始起点和前 512 字节，CRC32 从超级块开始计算。

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

合成镜像由 `src-tauri/test_data/rom-images/generate.py` 独立编码，覆盖大小端、
压缩页与空洞、跨页范围、空文件、硬链接、符号链接、特殊文件、CRC / 长度 /
解压损坏、目录循环，以及裸镜像、MBR、TRX 的正式 dispatch。

公开对照使用 [Binwalk 测试输入](https://github.com/ReFirmLabs/binwalk/tree/master/tests/inputs)
的 `cramfs.bin`（4096 字节，SHA256 `1e75d47c5c759bfb964011a4530ce353bcf7fe2377eb59b32b773445dbad4216`）
及 `romfs.bin`（1024 字节，SHA256 `31259785aa0c9404e6ae496b27c37d0620e10f783b127848aa3d5cc2a25cb6d3`）。
两者含同一份 104 字节 `/lsb-release`；外部样本不提交仓库。

`native-extended-le.cramfs` 和 `native-extended-be.cramfs` 由上游 `mkcramfs` 使用 `-p -z -X -X -q` 和分别指定小端或大端的 `-L` / `-B` 选项生成。两卷包含普通压缩页、随机字节形成的原始页、空洞、短尾页和 32 位 ELF 的 XIP 页。上游 `cramfsck` 提取了小端卷，并独立验证了把一页移动到非连续位置、改用带长度前缀的直接指针后的卷。测试比较两种字节序的文件哈希、跨页读取、裸卷与 MBR 入口，并拒绝损坏的块指针。 [上游工具与格式说明](https://github.com/npitre/cramfs-tools/tree/a95eecbf899479f8b3d65fb2822c5b7cbcdacfdd)

`native-v1.cramfs` 由 Linux 2.4.0 源码包中的 `scripts/cramfs/mkcramfs.c` 以 `-DDO_HOLES` 编译后生成。工具写入 65,536 字节的占位值，实物只有 8,192 字节。v1 用相邻的相同块指针表示空洞，当时尚无 HOLES 标志。上游 `cramfsck` 提取了同一组源文件，回归测试验证完整读取、跨页读取、空洞和截断。 [Linux 2.4.0 官方源码包](https://www.kernel.org/pub/linux/kernel/v2.4/linux-2.4.0.tar.xz)

| 原生镜像 | SHA-256 |
| --- | --- |
| `native-extended-le.cramfs` | `6ba5717eaf604ff533982dbea48fd980ba066442fbd39766e74c9a8c77235dd7` |
| `native-extended-be.cramfs` | `2bd6d1e0e8168c4ca0f232637b645dd3a821883c1c522983202ef2342a66db3a` |
| `native-v1.cramfs` | `ea888cd6cc0e313123ca5a3697ca79ab50f3931394effb587b62bb463d5a67e7` |

Windows 原生 GUI / Dokan 挂载和其他平台运行仍需单独验证。
