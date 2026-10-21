# Buffalo ENC / Broadcom TRX 固件

通过「添加检材 → 镜像文件」直接打开 `.enc`、`.trx` 或具有相同魔数的
`.bin` 固件，无需先运行外部解包程序。裸 `.squashfs` 也可直接打开。

## 实现范围

- Buffalo `start` / `asar1` ENC，默认公开密钥 `Buffalo`、256 字节 RC4 状态；
  包括 `bgn` 外层、152 字节加密产品标签、对齐后的 ENC 主体，以及解密后的
  `buffalo.enc\nfilelen=…\n` 前缀。产品标签、载荷均验证 Buffalo 的有符号字节校验和。
- Broadcom TRX v1：验证声明长度、CRC、三个分段槽的顺序与边界，展示其中的
  SquashFS / CramFS / Linux ROMFS 段。内核、填充和其他段保留在原始源中，不伪装成文件系统。
- 标准小端 / 大端 SquashFS 3.0、4.0，以及 DD-WRT 3.0 的 `hsqt` / `tqsh`
  变体；后者按每个块的 pb/lc/lp 参数解压 LZMA，复用文件树、预览、哈希、
  导出、流式与范围读取。

源镜像大小、源哈希、十六进制视图和源 RAW 导出保留原始容器字节。
固件按字节寻址；加密段的位置指向原始密文对应范围，文件浏览读取经过校验的明文。
压缩文件不虚构物理 data runs。明文归属于镜像实例，卸载时释放，不写入案件源文件。
打开时使用现有任务的取消令牌，逐块解密和校验；单个加密载荷限 512 MiB。

不支持自定义 Buffalo 密钥、longstate 模式、其他 TRX 版本、签名验证或任意
厂商固件格式。它们不是 Firmware Mod Kit 全部工具的兼容承诺。识别到的
不支持结构、校验失败和截断均明确报错，不以成功的空目录代替。

## 上游依据与许可

格式与兼容行为对照 [Firmware Mod Kit，c72f45d7a062](https://github.com/rampageX/firmware-mod-kit/tree/c72f45d7a062156125ae00cb867d8a614b50b963)：
`src/firmware-tools/buffalo-lib.c`（Gabor Juhos，GPL-2.0-only）、
`src/squashfs-3.0-lzma-damn-small-variant/squashfs_fs.h` 与其
`lzma/C/7zip/Compress/LZMA_Lib/ZLib.cpp`。

本地实现为 Rust 格式读取代码，没有复制或构建这些 C/C++ 源文件，也不调用
Firmware Mod Kit 的 Linux shell、提取器或重打包工具。
SquashFS 复用 `backhand`（MIT OR Apache-2.0），LZMA 使用 `lzma-rust2`；
具体集成变更见 [backhand/LOVELYCARVE.md](../../src-tauri/vendor/backhand/LOVELYCARVE.md)。

CramFS / Linux ROMFS 的 Rust 库选择、支持范围和校验要求见 [ROM_IMAGES](ROM_IMAGES.md)；
其他公开固件仍不能展开的实测结果见 [兼容性调查](FIRMWARE_COMPATIBILITY.md)。

## 验证

```powershell
just test firmware_
just test-vendor backhand --features v3_ddwrt
$env:LOVELYCARVE_FIRMWARE_IMAGE = 'C:/path/to/router.enc'
just test firmware_external_image -- --ignored --nocapture
just run-example probe_squashfs C:/path/to/router.enc
```

合成测试覆盖 TRX 的长度、CRC、分段边界、取消，以及通过正式镜像入口的
目录、文件、流式、范围读取和原始字节保留。外部样本按文件逐一比较完整、
流式和范围读取，不提交真实固件；未提供样本时外部测试保持 ignored。
外部样本一致性测试不等于运行 Firmware Mod Kit 二进制的独立 oracle。
